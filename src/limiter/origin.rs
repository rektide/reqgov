use crate::limiter::context::{CheckMetrics, SpanContext, SpanExtensions, StateMode};
use crate::limiter::state::RateLimitViolation;
use crate::policies::policy::Policy;
use crate::policies::slot::PolicySlot;
use crate::smoothing::smoother::Smoother;
use crate::tracing::enricher::enricher_trait::SpanEnricher;
use governor::clock::Clock;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Option<Smoother>,
    fastest_policy: Option<String>,
    span_context: Arc<RwLock<Option<SpanContext>>>,
    span_enricher: Arc<dyn SpanEnricher + Send + Sync>,
}

impl OriginRateLimiter {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            smoother: None,
            fastest_policy: None,
            span_context: Arc::new(RwLock::new(None)),
            span_enricher: Arc::new(crate::tracing::enricher::standard::StandardSpanEnricher),
        }
    }

    pub fn with_smoother(smoother_config: crate::smoothing::smoother::SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Some(Smoother::new(smoother_config)),
            fastest_policy: None,
            span_context: Arc::new(RwLock::new(None)),
            span_enricher: Arc::new(crate::tracing::enricher::standard::StandardSpanEnricher),
        }
    }

    pub fn with_span_enricher(enricher: Arc<dyn SpanEnricher + Send + Sync>) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: None,
            fastest_policy: None,
            span_context: Arc::new(RwLock::new(None)),
            span_enricher: enricher,
        }
    }

    pub fn with_smoother_and_span_enricher(smoother_config: crate::smoothing::smoother::SmootherConfig, enricher: Arc<dyn SpanEnricher + Send + Sync>) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Some(Smoother::new(smoother_config)),
            fastest_policy: None,
            span_context: Arc::new(RwLock::new(None)),
            span_enricher: enricher,
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

    pub fn check(&self) -> Result<(), RateLimitViolation> {
        let start = Instant::now();

        let (smoother_result, smoother_state, smoother_metrics) = if let Some(ref smoother) = self.smoother {
            let smoother_start = Instant::now();
            let result = smoother.check();
            let state = smoother.state();
            let duration = smoother_start.elapsed();
            let metrics = CheckMetrics {
                duration,
                passed: result.is_ok(),
                wait_duration: result.as_ref().err().map(|not_until| {
                    not_until.wait_time_from(smoother.clock().now())
                }),
            };
            (Some(result), Some(state), Some(metrics))
        } else {
            (None, None, None)
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
        let smoother_passed = smoother_result.as_ref().map(|r| r.is_ok()).unwrap_or(true);
        let all_passed = smoother_passed && policy_results.iter().all(|(_, r)| r.passed);
        let limiting_policy = if !all_passed {
            if !smoother_passed {
                None
            } else {
                policy_results.iter().find(|(_, r)| !r.passed).map(|(name, _)| name.clone())
            }
        } else {
            None
        };

        let failed_policy = if all_passed || !smoother_passed {
            None
        } else {
            policy_results.iter().find(|(_, r)| !r.passed).map(|(name, result)| {
                (name.clone(), result.wait_duration.unwrap())
            })
        };

        let extensions = SpanExtensions {
            smoother_state,
            policy_states,
            attributes: HashMap::new(),
            concurrency: crate::limiter::context::ConcurrencyMetrics::default(),
        };

        let span_context = SpanContext {
            all_passed,
            limiting_policy,
            total_duration,
            smoother_metrics,
            policy_metrics: policy_results,
            extensions,
            metadata: crate::limiter::context::SpanMetadata {
                timestamp: Instant::now(),
                duration: total_duration,
                mode: StateMode::Historical,
            },
        };

        *self.span_context.write().unwrap() = Some(span_context);

        if let Some(Err(not_until)) = smoother_result {
            return Err(RateLimitViolation::Smoothed {
                wait_duration: not_until.wait_time_from(self.smoother.as_ref().unwrap().clock().now()),
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
        if let Some(ref smoother) = self.smoother {
            smoother.wait().await;
        }

        for slot in self.slots.values() {
            slot.wait().await;
        }
    }
    
    pub fn state(&self) -> crate::limiter::state::OriginRateLimiterState {
        self.historical_state()
            .unwrap_or_else(|| self.fresh_state())
    }

    pub fn fresh_state(&self) -> crate::limiter::state::OriginRateLimiterState {
        let _ = self.check();
        self.state()
    }

    pub fn historical_state(&self) -> Option<crate::limiter::state::OriginRateLimiterState> {
        let span_context = self.span_context.read().unwrap().clone()?;

        Some(crate::limiter::state::OriginRateLimiterState {
            smoother: span_context.extensions.smoother_state.clone(),
            policies: span_context.extensions.policy_states.iter().map(|(_, s)| s.clone()).collect(),
            will_throttle: !span_context.all_passed,
            throttle_wait_duration: if !span_context.all_passed {
                span_context.smoother_metrics.as_ref().and_then(|s| s.wait_duration)
                    .or_else(|| span_context.policy_metrics.iter().find_map(|(_, p)| p.wait_duration))
            } else {
                None
            },
            span_context: Some(span_context.clone()),
            mode: StateMode::Historical,
        })
    }

    pub fn span_context(&self) -> Option<SpanContext> {
        self.span_context.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_rate_limiter_creation() {
        let limiter = OriginRateLimiter::new();
        assert_eq!(limiter.slots.len(), 0);
    }

    #[test]
    fn test_update_policies() {
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert_eq!(limiter.slots.len(), 1);
    }

    #[test]
    fn test_update_multiple_policies() {
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![
            crate::policies::policy::Policy {
                name: "burst".to_string(),
                quota: 100,
                window_secs: Some(60),
                quota_unit: crate::policies::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            crate::policies::policy::Policy {
                name: "daily".to_string(),
                quota: 10000,
                window_secs: Some(86400),
                quota_unit: crate::policies::policy::QuotaUnit::Requests,
                partition_key: None,
            },
        ];

        limiter.update_policies(policies);
        assert_eq!(limiter.slots.len(), 2);
    }

    #[test]
    fn test_update_existing_policy() {
        let mut limiter = OriginRateLimiter::new();

        let policies1 = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies1);
        assert_eq!(limiter.slots["burst"].policy.quota, 100);

        let policies2 = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 200,
            window_secs: Some(120),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies2);
        assert_eq!(limiter.slots["burst"].policy.quota, 200);
        assert_eq!(limiter.slots.len(), 1);
    }

    #[test]
    fn test_fastest_policy_detection() {
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![
            crate::policies::policy::Policy {
                name: "burst".to_string(),
                quota: 100,
                window_secs: Some(60),
                quota_unit: crate::policies::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            crate::policies::policy::Policy {
                name: "hourly".to_string(),
                quota: 5000,
                window_secs: Some(3600),
                quota_unit: crate::policies::policy::QuotaUnit::Requests,
                partition_key: None,
            },
            crate::policies::policy::Policy {
                name: "daily".to_string(),
                quota: 100000,
                window_secs: Some(86400),
                quota_unit: crate::policies::policy::QuotaUnit::Requests,
                partition_key: None,
            },
        ];

        limiter.update_policies(policies);
        assert_eq!(limiter.fastest_policy, Some("burst".to_string()));
    }

    #[test]
    fn test_check_allows_when_quotas_available() {
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert!(limiter.check().is_ok());
    }

    #[test]
    fn test_update_limits() {
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);

        let limits = vec![crate::policies::policy::ServiceLimit {
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
        let mut limiter = OriginRateLimiter::new();

        let policies = vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }];

        limiter.update_policies(policies);
        assert!(limiter.check().is_ok());
    }

    #[test]
    fn test_state_capture_during_check() {
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state = limiter.historical_state().unwrap();

        assert!(state.smoother.is_some());
        assert_eq!(state.policies.len(), 1);
        assert_eq!(state.mode, StateMode::Historical);
        assert!(state.span_context.is_some());
        assert_eq!(state.span_context.as_ref().unwrap().all_passed, true);
    }

    #[test]
    fn test_fresh_state_fetches_current() {
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
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
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state = limiter.state();

        assert_eq!(state.mode, StateMode::Historical);
        assert!(state.span_context.is_some());
    }

    #[test]
    fn test_check_captures_metrics_with_timing() {
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let span_context = limiter.span_context().unwrap();

        assert!(span_context.smoother_metrics.is_some());
        assert_eq!(span_context.policy_metrics.len(), 1);
        assert!(span_context.total_duration.as_nanos() > 0);
    }

    #[test]
    fn test_span_enricher_standard() {
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let span_context = limiter.span_context().unwrap();
        let enricher = crate::tracing::enricher::standard::StandardSpanEnricher;

        assert!(enricher.is_enabled());
    }

    #[test]
    fn test_span_extensions_custom_attributes() {
        let mut extensions = SpanExtensions::new();
        extensions.set("custom.org_id", crate::limiter::context::AttributeValue::from("org-123"));
        extensions.set("custom.region", crate::limiter::context::AttributeValue::from("us-east-1"));

        assert_eq!(extensions.attributes.len(), 2);
    }

    #[test]
    fn test_optional_smoother_new_constructor() {
        let limiter = OriginRateLimiter::new();
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_none());
    }

    #[test]
    fn test_optional_smoother_with_smoother_constructor() {
        let limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_some());
    }

    #[test]
    fn test_check_without_smoother() {
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
    fn test_state_without_smoother() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let state = limiter.historical_state().unwrap();

        assert!(state.smoother.is_none());
        assert_eq!(state.policies.len(), 1);
        assert_eq!(state.mode, StateMode::Historical);
        assert!(state.span_context.is_some());
        assert_eq!(state.span_context.as_ref().unwrap().all_passed, true);
        assert!(state.span_context.as_ref().unwrap().smoother_metrics.is_none());
    }

    #[test]
    fn test_span_context_without_smoother() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let span_context = limiter.span_context().unwrap();

        assert!(span_context.smoother_metrics.is_none());
        assert_eq!(span_context.policy_metrics.len(), 1);
        assert!(span_context.total_duration.as_nanos() > 0);
    }

    #[tokio::test]
    async fn test_wait_without_smoother() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        limiter.wait().await;
    }

    #[test]
    fn test_reconfigure_smoother_without_smoother() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        let limits = vec![crate::policies::policy::ServiceLimit {
            name: "burst".to_string(),
            remaining: 75,
            reset_secs: Some(30),
            partition_key: None,
        }];

        limiter.update_limits(limits);
        assert_eq!(limiter.slots["burst"].remaining, 75);
    }

    #[test]
    fn test_with_span_enricher_constructor() {
        let limiter = OriginRateLimiter::with_span_enricher(Arc::new(crate::tracing::enricher::minimal::MinimalSpanEnricher));
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_none());
    }

    #[test]
    fn test_with_smoother_and_span_enricher_constructor() {
        let limiter = OriginRateLimiter::with_smoother_and_span_enricher(
            crate::smoothing::smoother::SmootherConfig::default(),
            Arc::new(crate::tracing::enricher::detailed::DetailedSpanEnricher)
        );
        assert_eq!(limiter.slots.len(), 0);
        assert!(limiter.smoother.is_some());
    }

    #[test]
    fn test_limiting_policy_without_smoother() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 1,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
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
    fn test_concurrency_span_enricher() {
        let mut limiter = OriginRateLimiter::with_smoother(crate::smoothing::smoother::SmootherConfig::default());
        limiter.update_policies(vec![crate::policies::policy::Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policies::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let span_context = limiter.span_context().unwrap();
        let enricher = crate::tracing::enricher::concurrency::ConcurrencySpanEnricher;

        assert!(enricher.is_enabled());
    }

    #[test]
    fn test_enricher_presets_concurrency() {
        let enricher = crate::tracing::enricher::chain::EnricherPresets::concurrency();
        assert!(enricher.is_enabled());
    }
}
