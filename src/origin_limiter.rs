use crate::policy::{Policy, ServiceLimit};
use crate::policy_slot::{PolicySlot, PolicySlotState};
use crate::smoother::{Smoother, SmootherConfig, SmootherState};
use governor::clock::Clock;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CheckMetrics {
    pub duration: Duration,
    pub passed: bool,
    pub wait_duration: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct LastCheckResult {
    pub timestamp: Instant,
    pub smoother: Option<CheckMetrics>,
    pub policies: Vec<(String, CheckMetrics)>,
    pub total_duration: Duration,
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RateLimitViolation {
    Smoothed { wait_duration: Duration },
    PolicyExceeded { policy_name: String, wait_duration: Duration },
}

/// State snapshot for telemetry
#[derive(Debug, Clone)]
pub struct OriginRateLimiterState {
    pub smoother: Option<SmootherState>,
    pub policies: Vec<PolicySlotState>,
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,
    pub last_check: Option<LastCheckResult>,
}

impl OriginRateLimiterState {
    pub fn limiting_policy(&self) -> Option<&str> {
        self.last_check.as_ref().and_then(|r| r.limiting_policy.as_deref())
    }
}

pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Smoother,
    fastest_policy: Option<String>,
    last_check_result: Arc<RwLock<Option<LastCheckResult>>>,
}

impl OriginRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(smoother_config),
            fastest_policy: None,
            last_check_result: Arc::new(RwLock::new(None)),
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
        let start = Instant::now();

        let smoother_start = Instant::now();
        let smoother_result = self.smoother.check();
        let smoother_duration = smoother_start.elapsed();
        let smoother_check = CheckMetrics {
            duration: smoother_duration,
            passed: smoother_result.is_ok(),
            wait_duration: smoother_result.as_ref().err().map(|not_until| {
                not_until.wait_time_from(self.smoother.clock().now())
            }),
        };

        let mut policy_results = Vec::new();
        for (name, slot) in &self.slots {
            let policy_start = Instant::now();
            let policy_result = slot.check();
            let policy_duration = policy_start.elapsed();

            policy_results.push((
                name.clone(),
                CheckMetrics {
                    duration: policy_duration,
                    passed: policy_result.is_ok(),
                    wait_duration: policy_result.as_ref().err().map(|not_until| {
                        not_until.wait_time_from(slot.clock().now())
                    }),
                },
            ));
        }

        let total_duration = start.elapsed();
        let all_passed = smoother_result.is_ok() && policy_results.iter().all(|(_, r)| r.passed);
        let limiting_policy = if !all_passed {
            if smoother_result.is_err() {
                None
            } else {
                policy_results.iter().find(|(_, r)| !r.passed).map(|(name, _)| name.clone())
            }
        } else {
            None
        };

        let failed_policy = if all_passed || smoother_result.is_err() {
            None
        } else {
            policy_results.iter().find(|(_, r)| !r.passed).map(|(name, result)| {
                (name.clone(), result.wait_duration.unwrap())
            })
        };

        let last_result = LastCheckResult {
            timestamp: Instant::now(),
            smoother: Some(smoother_check),
            policies: policy_results,
            total_duration,
            all_passed,
            limiting_policy,
        };

        *self.last_check_result.write().unwrap() = Some(last_result);

        if let Err(not_until) = smoother_result {
            return Err(RateLimitViolation::Smoothed {
                wait_duration: not_until.wait_time_from(self.smoother.clock().now()),
            });
        }

        if let Some((name, wait_duration)) = failed_policy {
            return Err(RateLimitViolation::PolicyExceeded {
                policy_name: name,
                wait_duration,
            });
        }

        Ok(())
    }

    pub async fn wait(&self) {
        self.smoother.wait().await;

        for slot in self.slots.values() {
            slot.wait().await;
        }
    }
    
    /// Get state snapshot for telemetry
    pub fn state(&self) -> OriginRateLimiterState {
        let smoother_state = self.smoother.state();

        let policy_states: Vec<PolicySlotState> = self.slots
            .values()
            .map(|slot| slot.state())
            .collect();

        let last_check = self.last_check_result.read().unwrap().clone();
        let (will_throttle, _limiting_policy) = match &last_check {
            Some(result) => (
                !result.all_passed,
                result.limiting_policy.clone(),
            ),
            None => {
                let check_result = self.check();
                (
                    check_result.is_err(),
                    check_result.err().and_then(|violation| match violation {
                        RateLimitViolation::Smoothed { .. } => None,
                        RateLimitViolation::PolicyExceeded { policy_name, .. } => Some(policy_name),
                    }),
                )
            }
        };

        OriginRateLimiterState {
            smoother: Some(smoother_state),
            policies: policy_states,
            will_throttle,
            throttle_wait_duration: last_check.as_ref().and_then(|r| {
                if !r.all_passed {
                    r.smoother.as_ref().and_then(|s| s.wait_duration)
                        .or_else(|| r.policies.iter().find_map(|(_, p)| p.wait_duration))
                } else {
                    None
                }
            }),
            last_check,
        }
    }

    pub fn last_check_result(&self) -> Option<LastCheckResult> {
        self.last_check_result.read().unwrap().clone()
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
