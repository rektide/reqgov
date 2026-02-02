use crate::limiter::state::RateLimitViolation;
use crate::policies::policy::Policy;
use crate::policies::slot::PolicySlot;
use crate::smoothing::smoother::{Smoother, SmootherConfig};
use governor::clock::Clock;
use std::collections::HashMap;

#[derive(Default)]
pub struct OriginRateLimiterBuilder {
    smoother_config: Option<SmootherConfig>,
}

impl OriginRateLimiterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn smoother(mut self, config: SmootherConfig) -> Self {
        self.smoother_config = Some(config);
        self
    }

    pub fn build(self) -> OriginRateLimiter {
        OriginRateLimiter {
            smoother: self.smoother_config.map(Smoother::new),
            slots: HashMap::new(),
            fastest_policy: None,
        }
    }
}

pub struct OriginRateLimiter {
    pub(crate) slots: HashMap<String, PolicySlot>,
    pub(crate) smoother: Option<Smoother>,
    fastest_policy: Option<String>,
}

impl OriginRateLimiter {
    pub fn builder() -> OriginRateLimiterBuilder {
        OriginRateLimiterBuilder::new()
    }

    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn update_policies(&mut self, policies: Vec<Policy>) {
        for policy in policies {
            self.slots
                .entry(policy.name.clone())
                .and_modify(|slot| slot.policy = policy.clone())
                .or_insert_with(|| PolicySlot::new(policy));
        }

        self.recalculate_fastest();
    }

    pub fn update_limits(&mut self, limits: Vec<crate::policies::policy::ServiceLimit>) {
        for limit in limits {
            if let Some(slot) = self.slots.get_mut(&limit.name) {
                slot.update(&limit);
            }
        }

        self.reconfigure_smoother();
    }

    fn recalculate_fastest(&mut self) {
        self.fastest_policy = self
            .slots
            .values()
            .filter_map(|s| s.policy.window_secs.map(|w| (s.policy.name.clone(), w)))
            .min_by_key(|(_, w)| *w)
            .map(|(name, _)| name);
    }

    fn reconfigure_smoother(&mut self) {
        if let Some(ref mut smoother) = self.smoother {
            if let Some(ref name) = self.fastest_policy {
                if let Some(slot) = self.slots.get(name) {
                    let window = slot.policy.window_secs.unwrap_or(60);
                    smoother.configure(slot.remaining, window);
                }
            }
        }
    }

    /// Access slots for tracing middleware
    pub fn slots(&self) -> impl Iterator<Item = (&String, &PolicySlot)> {
        self.slots.iter()
    }

    /// Access smoother for tracing middleware
    pub fn smoother(&self) -> Option<&Smoother> {
        self.smoother.as_ref()
    }

    pub fn check(&self) -> Result<(), RateLimitViolation> {
        if let Some(ref smoother) = self.smoother {
            if let Err(not_until) = smoother.check() {
                return Err(RateLimitViolation::Smoothed {
                    wait_duration: not_until.wait_time_from(smoother.clock().now()),
                });
            }
        }

        for (name, slot) in &self.slots {
            if let Err(not_until) = slot.check() {
                return Err(RateLimitViolation::PolicyExceeded {
                    policy_name: name.clone(),
                    wait_duration: not_until.wait_time_from(slot.clock().now()),
                });
            }
        }

        Ok(())
    }

    pub async fn wait(&self) {
        if let Some(ref smoother) = self.smoother {
            smoother.wait().await;
        }

        for slot in self.slots.values() {
            slot.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_new() {
        let limiter = OriginRateLimiter::new();
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_none());
    }

    #[test]
    fn test_limiter_with_smoother() {
        let limiter = OriginRateLimiter::builder()
            .smoother(crate::smoothing::smoother::SmootherConfig::default())
            .build();
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_some());
    }

    #[test]
    fn test_update_policies() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert_eq!(limiter.slots.len(), 1);
        assert!(limiter.slots.contains_key("burst"));
    }

    #[test]
    fn test_update_limits() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.update_limits(vec![crate::policies::policy::ServiceLimit {
            name: "burst".to_string(),
            remaining: 45,
            reset_secs: Some(30),
            partition_key: None,
        }]);

        assert_eq!(limiter.slots["burst"].remaining, 45);
    }

    #[test]
    fn test_check_passes() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert!(limiter.check().is_ok());
    }

    #[test]
    fn test_check_with_smoother() {
        let mut limiter = OriginRateLimiter::builder()
            .smoother(crate::smoothing::smoother::SmootherConfig::default())
            .build();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert!(limiter.check().is_ok());
    }

    #[test]
    fn test_policy_exceeded() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 1,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check().unwrap();
        let result = limiter.check();

        assert!(result.is_err());
        match result {
            Err(RateLimitViolation::PolicyExceeded { policy_name, .. }) => {
                assert_eq!(policy_name, "burst");
            }
            _ => panic!("Expected PolicyExceeded error"),
        }
    }

    #[test]
    fn test_slots_accessor() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        let slots: Vec<_> = limiter.slots().collect();
        assert_eq!(slots.len(), 1);
    }

    #[test]
    fn test_smoother_accessor() {
        let limiter = OriginRateLimiter::new();
        assert!(limiter.smoother().is_none());

        let limiter_with = OriginRateLimiter::builder()
            .smoother(crate::smoothing::smoother::SmootherConfig::default())
            .build();
        assert!(limiter_with.smoother().is_some());
    }

    #[tokio::test]
    async fn test_wait() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check().unwrap();
        limiter.wait().await;
    }
}
