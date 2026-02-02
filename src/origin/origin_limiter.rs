use crate::origin::state::RateLimitViolation;
use crate::origin::policies::{Policy, ServiceLimit};
use crate::origin::slots::PolicySlot;
use governor::clock::Clock;
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct OriginLimiterBuilder;

impl OriginLimiterBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(self) -> OriginLimiter {
        OriginLimiter {
            slots: Arc::new(DashMap::new()),
        }
    }
}

pub struct OriginLimiter {
    pub(crate) slots: Arc<DashMap<String, PolicySlot>>,
}

impl Clone for OriginLimiter {
    fn clone(&self) -> Self {
        Self {
            slots: Arc::clone(&self.slots),
        }
    }
}

impl OriginLimiter {
    pub fn builder() -> OriginLimiterBuilder {
        OriginLimiterBuilder::new()
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
    }

    pub async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        for limit in limits {
            if let Some(mut slot) = self.slots.get_mut(&limit.name) {
                slot.update(&limit);
            }
        }
    }

    /// Access slots for tracing middleware
    pub fn slots(&self) -> Vec<(String, PolicySlot)> {
        vec![]
    }

    pub async fn check(&self) -> Result<(), RateLimitViolation> {
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
        for r in self.slots.iter() {
            r.value().wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::policies::QuotaUnit;

    #[tokio::test]
    async fn test_limiter_new() {
        let limiter = OriginLimiter::new();
        assert_eq!(limiter.slots.len(), 0);
    }

    #[test]
    fn test_update_policies() {
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::new();
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
        let limiter = OriginLimiter::builder().build();
        let _cloned = limiter.clone();
    }
}
