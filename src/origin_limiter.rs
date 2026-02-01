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

#[derive(Debug, Clone, PartialEq)]
pub enum StateMode {
    Historical,
    Fresh,
}

#[derive(Debug, Clone)]
pub struct LastCheckResult {
    pub timestamp: Instant,
    pub smoother_state: Option<SmootherState>,
    pub policy_states: Vec<(String, PolicySlotState)>,
    pub smoother_metrics: Option<CheckMetrics>,
    pub policy_metrics: Vec<(String, CheckMetrics)>,
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
    pub mode: StateMode,
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
        let smoother_state = self.smoother.state();
        let smoother_metrics = CheckMetrics {
            duration: smoother_duration,
            passed: smoother_result.is_ok(),
            wait_duration: smoother_result.as_ref().err().map(|not_until| {
                not_until.wait_time_from(self.smoother.clock().now())
            }),
        };

        let mut policy_results = Vec::new();
        let mut policy_states = Vec::new();
        for (name, slot) in &self.slots {
            let policy_start = Instant::now();
            let policy_result = slot.check();
            let policy_duration = policy_start.elapsed();
            let policy_state = slot.state();

            policy_states.push((name.clone(), policy_state));
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
            smoother_state: Some(smoother_state),
            policy_states,
            smoother_metrics: Some(smoother_metrics),
            policy_metrics: policy_results,
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
        self.historical_state()
            .unwrap_or_else(|| self.fresh_state())
    }

    /// Get fresh current state (always calls governor, no caching)
    pub fn fresh_state(&self) -> OriginRateLimiterState {
        let _ = self.check();
        self.state()
    }

    /// Get historical state from last check (no governor calls)
    pub fn historical_state(&self) -> Option<OriginRateLimiterState> {
        let last_check = self.last_check_result.read().unwrap().clone()?;

        Some(OriginRateLimiterState {
            smoother: last_check.smoother_state.clone(),
            policies: last_check.policy_states.iter().map(|(_, s)| s.clone()).collect(),
            will_throttle: !last_check.all_passed,
            throttle_wait_duration: if !last_check.all_passed {
                last_check.smoother_metrics.as_ref().and_then(|s| s.wait_duration)
                    .or_else(|| last_check.policy_metrics.iter().find_map(|(_, p)| p.wait_duration))
            } else {
                None
            },
            last_check: Some(last_check.clone()),
            mode: StateMode::Historical,
        })
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

    #[test]
    fn test_state_capture_during_check() {
        let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state = limiter.historical_state().unwrap();

        assert!(state.smoother.is_some());
        assert_eq!(state.policies.len(), 1);
        assert_eq!(state.mode, StateMode::Historical);
        assert!(state.last_check.is_some());
        assert_eq!(state.last_check.as_ref().unwrap().all_passed, true);
    }

    #[test]
    fn test_fresh_state_fetches_current() {
        let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state1 = limiter.historical_state().unwrap();
        let remaining1 = state1.policies[0].remaining;

        limiter.check();
        let state2 = limiter.historical_state().unwrap();
        let remaining2 = state2.policies[0].remaining;

        assert!(remaining2 < remaining1);
    }

    #[test]
    fn test_state_uses_historical_by_default() {
        let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state = limiter.state();

        assert_eq!(state.mode, StateMode::Historical);
        assert!(state.last_check.is_some());
    }

    #[test]
    fn test_check_captures_metrics_with_timing() {
        let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let last_check = limiter.last_check_result().unwrap();

        assert!(last_check.smoother_metrics.is_some());
        assert_eq!(last_check.policy_metrics.len(), 1);
        assert!(last_check.total_duration.as_nanos() > 0);
    }
}
