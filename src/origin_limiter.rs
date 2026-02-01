use crate::policy::{Policy, ServiceLimit};
use crate::policy_slot::{PolicySlot, PolicySlotState};
use crate::smoother::{Smoother, SmootherConfig, SmootherState};
use governor::clock::Clock;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::Span;

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
pub struct SpanMetadata {
    pub timestamp: Instant,
    pub duration: Duration,
    pub mode: StateMode,
}

#[derive(Debug, Clone)]
pub struct SpanExtensions {
    pub smoother_state: Option<SmootherState>,
    pub policy_states: Vec<(String, PolicySlotState)>,
    pub attributes: HashMap<&'static str, AttributeValue>,
}

impl SpanExtensions {
    pub fn new() -> Self {
        Self {
            smoother_state: None,
            policy_states: Vec::new(),
            attributes: HashMap::new(),
        }
    }

    pub fn set(&mut self, key: &'static str, value: AttributeValue) {
        self.attributes.insert(key, value);
    }
}

#[derive(Debug, Clone)]
pub enum AttributeValue {
    Bool(bool),
    U64(u64),
    I64(i64),
    Str(String),
    Float(f64),
    Duration(Duration),
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        AttributeValue::Bool(value)
    }
}

impl From<u64> for AttributeValue {
    fn from(value: u64) -> Self {
        AttributeValue::U64(value)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        AttributeValue::I64(value)
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        AttributeValue::Str(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        AttributeValue::Str(value.to_string())
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        AttributeValue::Float(value)
    }
}

impl From<Duration> for AttributeValue {
    fn from(value: Duration) -> Self {
        AttributeValue::Duration(value)
    }
}

#[derive(Debug, Clone)]
pub struct SpanContext {
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
    pub total_duration: Duration,
    pub smoother_metrics: Option<CheckMetrics>,
    pub policy_metrics: Vec<(String, CheckMetrics)>,
    pub extensions: SpanExtensions,
    pub metadata: SpanMetadata,
}

pub trait SpanEnricher: Send + Sync {
    fn enrich(&self, span: &Span, context: &SpanContext);
    fn is_enabled(&self) -> bool {
        true
    }
}

pub struct MinimalSpanEnricher;

impl SpanEnricher for MinimalSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        if context.all_passed {
            span.record("rate_limit", "allowed");
        } else {
            span.record("rate_limit", "blocked");
        }
    }
}

pub struct StandardSpanEnricher;

impl SpanEnricher for StandardSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.allowed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy);
        }
    }
}

pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        let mode_str = format!("{:?}", context.metadata.mode);
        span.record("rate_limit.mode", mode_str);
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        span.record("rate_limit.timestamp_ms", context.metadata.timestamp.elapsed().as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        if let Some(ref smoother_state) = context.extensions.smoother_state {
            span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
        }
        
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name);
            span.record(remaining_key.as_str(), state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name);
            span.record(quota_key.as_str(), state.quota);
        }
    }
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
    pub span_context: Option<SpanContext>,
    pub mode: StateMode,
}

impl OriginRateLimiterState {
    pub fn limiting_policy(&self) -> Option<&str> {
        self.span_context.as_ref().and_then(|r| r.limiting_policy.as_deref())
    }
}

pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Smoother,
    fastest_policy: Option<String>,
    span_context: Arc<RwLock<Option<SpanContext>>>,
    span_enricher: Arc<dyn SpanEnricher + Send + Sync>,
}

impl OriginRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(smoother_config),
            fastest_policy: None,
            span_context: Arc::new(RwLock::new(None)),
            span_enricher: Arc::new(StandardSpanEnricher),
        }
    }

    pub fn with_span_enricher(smoother_config: SmootherConfig, enricher: Arc<dyn SpanEnricher + Send + Sync>) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(smoother_config),
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

        let extensions = SpanExtensions {
            smoother_state: Some(smoother_state),
            policy_states,
            attributes: HashMap::new(),
        };

        let span_context = SpanContext {
            all_passed,
            limiting_policy,
            total_duration,
            smoother_metrics: Some(smoother_metrics),
            policy_metrics: policy_results,
            extensions,
            metadata: SpanMetadata {
                timestamp: Instant::now(),
                duration: total_duration,
                mode: StateMode::Historical,
            },
        };

        *self.span_context.write().unwrap() = Some(span_context);

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
        let span_context = self.span_context.read().unwrap().clone()?;

        Some(OriginRateLimiterState {
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
        assert!(state.span_context.is_some());
        assert_eq!(state.span_context.as_ref().unwrap().all_passed, true);
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
        assert!(state.span_context.is_some());
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
        let span_context = limiter.span_context().unwrap();

        assert!(span_context.smoother_metrics.is_some());
        assert_eq!(span_context.policy_metrics.len(), 1);
        assert!(span_context.total_duration.as_nanos() > 0);
    }

    #[test]
    fn test_span_enricher_standard() {
        let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        }]);

        limiter.check();
        let span_context = limiter.span_context().unwrap();
        let enricher = StandardSpanEnricher;

        assert!(enricher.is_enabled());
    }

    #[test]
    fn test_span_extensions_custom_attributes() {
        let mut extensions = SpanExtensions::new();
        extensions.set("custom.org_id", "org-123".into());
        extensions.set("custom.region", "us-east-1".into());

        assert_eq!(extensions.attributes.len(), 2);
    }
}
