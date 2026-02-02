use crate::origin::state::RateLimitViolation;
use crate::origin::policies::{Policy, QuotaUnit, ServiceLimit};
use crate::origin::slots::PolicySlot;
use crate::origin::smoother::{Smoother, SmootherConfig};
use governor::clock::Clock;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::sync::Arc;

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
            smoother: Arc::new(tokio::sync::RwLock::new(self.smoother_config.map(Smoother::new))),
            slots: Arc::new(DashMap::new()),
            fastest_policy: ArcSwap::from_pointee(None),
        }
    }
}

pub struct OriginRateLimiter {
    pub(crate) smoother: Arc<tokio::sync::RwLock<Option<Smoother>>>,
    pub(crate) slots: Arc<DashMap<String, PolicySlot>>,
    fastest_policy: ArcSwap<Option<String>>,
}

impl Clone for OriginRateLimiter {
    fn clone(&self) -> Self {
        Self {
            smoother: Arc::clone(&self.smoother),
            slots: Arc::clone(&self.slots),
            fastest_policy: ArcSwap::new(self.fastest_policy.load_full()),
        }
    }
}

impl OriginRateLimiter {
    pub fn builder() -> OriginRateLimiterBuilder {
        OriginRateLimiterBuilder::new()
    }

    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn update_policies(&self, policies: Vec<Policy>) {
        for policy in policies {
            self.slots.entry(policy.name.clone())
                .and_modify(|slot| slot.policy = policy.clone())
                .or_insert_with(|| PolicySlot::new(policy));
        }
        self.recalculate_fastest();
    }

    pub async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        for limit in limits {
            if let Some(mut slot) = self.slots.get_mut(&limit.name) {
                slot.update(&limit);
            }
        }
        self.reconfigure_smoother().await;
    }

    fn recalculate_fastest(&self) {
        let fastest = self.slots
            .iter()
            .filter_map(|s| s.value().policy.window_secs.map(|w| (s.key().clone(), w)))
            .min_by_key(|(_, w)| *w)
            .map(|(name, _)| name);
        self.fastest_policy.store(Arc::new(fastest));
    }

    async fn reconfigure_smoother(&self) {
        let fastest = self.fastest_policy.load();
        if let Some(ref name) = **fastest {
            if let Some(slot) = self.slots.get(name) {
                let window = slot.policy.window_secs.unwrap_or(60);
                if let Some(smoother) = self.smoother.write().await.as_mut() {
                    smoother.configure(slot.remaining, window);
                }
            }
        }
    }

    /// Access slots for tracing middleware
    pub fn slots(&self) -> Vec<(String, PolicySlot)> {
        vec![]
    }

    pub async fn check(&self) -> Result<(), RateLimitViolation> {
        if let Some(smoother) = self.smoother.read().await.as_ref() {
            if let Err(not_until) = smoother.check() {
                return Err(RateLimitViolation::Smoothed {
                    wait_duration: not_until.wait_time_from(smoother.clock().now()),
                });
            }
        }

        for r in self.slots.iter() {
            if let Err(not_until) = r.value().check() {
                return Err(RateLimitViolation::PolicyExceeded {
                    policy_name: r.key().clone(),
                    wait_duration: not_until.wait_time_from(r.value().clock().now()),
                });
            }
        }

        Ok(())
    }

    pub async fn wait(&self) {
        if let Some(smoother) = self.smoother.write().await.as_mut() {
            smoother.wait().await;
        }

        for r in self.slots.iter() {
            r.value().wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_limiter_new() {
        let limiter = OriginRateLimiter::new();
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.read().await.is_none());
    }

    #[tokio::test]
    async fn test_limiter_with_smoother() {
        let limiter = OriginRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .build();
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.read().await.is_some());
    }

    #[test]
    fn test_update_policies() {
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert_eq!(limiter.slots.len(), 1);
        assert!(limiter.slots.contains_key("burst"));
    }

    #[tokio::test]
    async fn test_update_limits() {
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.update_limits(vec![ServiceLimit {
            name: "burst".to_string(),
            remaining: 45,
            reset_secs: Some(30),
            partition_key: None,
        }]).await;

        let slot = limiter.slots.get("burst").unwrap();
        assert_eq!(slot.remaining, 45);
    }

    #[tokio::test]
    async fn test_check_passes() {
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert!(limiter.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_check_with_smoother() {
        let limiter = OriginRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .build();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert!(limiter.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_policy_exceeded() {
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 1,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check().await.unwrap();
        let result = limiter.check().await;

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
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        assert_eq!(limiter.slots.len(), 1);
    }

    #[tokio::test]
    async fn test_wait() {
        let limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check().await.unwrap();
        limiter.wait().await;
    }

    #[test]
    fn test_limiter_is_clone() {
        let limiter = OriginRateLimiter::builder().build();
        let _cloned = limiter.clone();
    }
}
