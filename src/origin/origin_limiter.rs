use crate::origin::state::RateLimitViolation;
use crate::origin::policies::{Policy, ServiceLimit};
use crate::origin::slots::PolicySlot;
use governor::clock::Clock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Default)]
pub struct OriginLimiterBuilder;

impl OriginLimiterBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(self) -> OriginLimiter {
        OriginLimiter {
            slots: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

pub struct OriginLimiter {
    pub(crate) slots: Arc<Mutex<Vec<PolicySlot>>>,
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

    pub async fn update_policies(&self, policies: Vec<Policy>) {
        let mut slots = self.slots.lock().await;

        for policy in policies {
            let existing = slots.iter_mut()
                .find(|slot| slot.policy.name == policy.name);

            if let Some(slot) = existing {
                slot.policy = policy;
            } else {
                slots.push(PolicySlot::new(policy));
            }
        }

        slots.sort_by(|a, b| {
            a.policy.window_secs.unwrap_or(60)
                .cmp(&b.policy.window_secs.unwrap_or(60))
        });
    }

    pub async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        let mut slots = self.slots.lock().await;

        for limit in limits {
            if let Some(slot) = slots.iter_mut().find(|s| s.policy.name == limit.name) {
                slot.update(&limit);
            }
        }
    }

    /// Access slots for tracing middleware
    pub async fn slots(&self) -> Vec<(String, u32, u32, u32)> {
        let slots = self.slots.lock().await;
        slots.iter()
            .map(|slot| (
                slot.policy.name.clone(),
                slot.governor_remaining(),
                slot.policy.quota,
                slot.policy.window_secs.unwrap_or(60)
            ))
            .collect()
    }

    pub async fn check(&self) -> Result<(), RateLimitViolation> {
        let slots = self.slots.lock().await;

        for slot in slots.iter() {
            if let Err(not_until) = slot.check() {
                return Err(RateLimitViolation::PolicyExceeded {
                    policy_name: slot.policy.name.clone(),
                    wait_duration: not_until.wait_time_from(slot.clock().now()),
                });
            }
        }
        Ok(())
    }

    pub async fn wait(&self) -> Duration {
        let slots = self.slots.lock().await;
        let mut total_wait = Duration::ZERO;

        for slot in slots.iter() {
            total_wait += slot.wait().await;
        }
        total_wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::policies::QuotaUnit;

    #[tokio::test]
    async fn test_limiter_new() {
        let limiter = OriginLimiter::new();
        assert_eq!(limiter.slots.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn test_update_policies() {
        let limiter = OriginLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]).await;

        assert_eq!(limiter.slots.lock().await.len(), 1);
        assert_eq!(limiter.slots.lock().await[0].policy.name, "burst");
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
        }]).await;

        limiter.update_limits(vec![ServiceLimit {
            name: "burst".to_string(),
            remaining: 45,
            reset_secs: Some(30),
            partition_key: None,
        }]).await;

        let slot = &limiter.slots.lock().await[0];
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
        }]).await;

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
        }]).await;

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

    #[tokio::test]
    async fn test_slots_accessor() {
        let limiter = OriginLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }]).await;

        assert_eq!(limiter.slots.lock().await.len(), 1);
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
        }]).await;

        limiter.check().await.unwrap();
        limiter.wait().await;
    }

    #[test]
    fn test_limiter_is_clone() {
        let limiter = OriginLimiter::builder().build();
        let _cloned = limiter.clone();
    }
}
