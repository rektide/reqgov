use crate::policy::{Policy, ServiceLimit};
use crate::policy_slot::PolicySlot;
use crate::smoother::{Smoother, SmootherConfig};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum RateLimitViolation {
    Smoothed { wait_duration: Duration },
    PolicyExceeded { policy_name: String, wait_duration: Duration },
}

pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Smoother,
    fastest_policy: Option<String>,
}

impl OriginRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(smoother_config),
            fastest_policy: None,
        }
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

    pub fn update_limits(&mut self, limits: Vec<ServiceLimit>) {
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
        if let Some(ref name) = self.fastest_policy {
            if let Some(slot) = self.slots.get(name) {
                let window = slot.policy.window_secs.unwrap_or(60);
                self.smoother.configure(slot.remaining, window);
            }
        }
    }

    pub fn check(&self) -> Result<(), RateLimitViolation> {
        self.smoother
            .check()
            .map_err(|wait| RateLimitViolation::Smoothed { wait_duration: wait })?;

        for (name, slot) in &self.slots {
            slot.check().map_err(|wait| RateLimitViolation::PolicyExceeded {
                policy_name: name.clone(),
                wait_duration: wait,
            })?;
        }

        Ok(())
    }

    pub async fn wait(&self) {
        self.smoother.wait().await;

        for slot in self.slots.values() {
            slot.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_rate_limiter_creation() {
        let config = SmootherConfig::default();
        let limiter = OriginRateLimiter::new(config);
        assert_eq!(limiter.slots.len(), 0);
    }

    #[test]
    fn test_update_policies() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert_eq!(limiter.slots.len(), 1);
    }

    #[test]
    fn test_update_multiple_policies() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![
            Policy {
                name: "burst".to_string(),
                quota: 100,
                window_secs: Some(60),
                quota_unit: crate::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            Policy {
                name: "daily".to_string(),
                quota: 10000,
                window_secs: Some(86400),
                quota_unit: crate::policy::QuotaUnit::Requests,
                partition_key: None,
            },
        ];

        limiter.update_policies(policies);
        assert_eq!(limiter.slots.len(), 2);
    }

    #[test]
    fn test_update_existing_policy() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies1 = vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies1);
        assert_eq!(limiter.slots["burst"].policy.quota, 100);

        let policies2 = vec![Policy {
            name: "burst".to_string(),
            quota: 200,
            window_secs: Some(120),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies2);
        assert_eq!(limiter.slots["burst"].policy.quota, 200);
        assert_eq!(limiter.slots.len(), 1);
    }

    #[test]
    fn test_fastest_policy_detection() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![
            Policy {
                name: "burst".to_string(),
                quota: 100,
                window_secs: Some(60),
                quota_unit: crate::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            Policy {
                name: "hourly".to_string(),
                quota: 5000,
                window_secs: Some(3600),
                quota_unit: crate::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            Policy {
                name: "daily".to_string(),
                quota: 100000,
                window_secs: Some(86400),
                quota_unit: crate::policy::QuotaUnit::Requests,
                partition_key: None,
            },
        ];

        limiter.update_policies(policies);
        assert_eq!(limiter.fastest_policy, Some("burst".to_string()));
    }

    #[test]
    fn test_check_allows_when_quotas_available() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert!(limiter.check().is_ok());
    }

    #[test]
    fn test_update_limits() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);

        let limits = vec![ServiceLimit {
            name: "burst".to_string(),
            remaining: 75,
            reset_secs: Some(30),
            partition_key: None,
        }];

        limiter.update_limits(limits);
        assert_eq!(limiter.slots["burst"].remaining, 75);
    }

    #[test]
    fn test_check_allows_immediately_when_quotas_available() {
        let config = SmootherConfig::default();
        let mut limiter = OriginRateLimiter::new(config);

        let policies = vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert!(limiter.check().is_ok());
    }
}
