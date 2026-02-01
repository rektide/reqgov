use std::time::Duration;
/// Span enrichment for rate limiting telemetry
///
/// This module enriches spans created by reqwest-tracing with governor state,
/// not HTTP headers. The reqwest-tracing middleware creates HTTP request spans,
/// and we add rate limiting telemetry to those existing spans.
use tracing::Span;

/// Trait for enriching rate limit telemetry on existing spans
pub trait RateLimitSpanBackend: Send + Sync + Clone {
    /// Enrich current span with rate limit state information
    ///
    /// This is called during request processing to add rate limiting
    /// telemetry to the HTTP request span created by reqwest-tracing.
    fn enrich_span(&self, state: &RateLimitState);
}

/// Snapshot of rate limiter state for telemetry
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// Origin being rate limited (e.g., "api.github.com")
    pub origin: Option<String>,

    /// Smoother state (micro-interval pacing)
    pub smoother: Option<crate::smoothing::smoother::SmootherState>,

    pub policies: Vec<crate::policies::slot::PolicySlotState>,

    /// Overall whether rate limiting will cause a delay
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,
}

/// Minimal backend - only adds whether rate limiting occurred
#[derive(Debug, Clone, Copy, Default)]
pub struct MinimalSpanBackend;

impl RateLimitSpanBackend for MinimalSpanBackend {
    fn enrich_span(&self, state: &RateLimitState) {
        let span = Span::current();
        span.record("rate_limit.enabled", true);
        span.record("rate_limit.will_throttle", state.will_throttle);
    }
}

/// Standard backend - adds basic rate limit state
#[derive(Debug, Clone, Copy, Default)]
pub struct StandardSpanBackend;

impl RateLimitSpanBackend for StandardSpanBackend {
    fn enrich_span(&self, state: &RateLimitState) {
        let span = Span::current();

        span.record("rate_limit.enabled", true);
        span.record("rate_limit.will_throttle", state.will_throttle);

        if let Some(ref origin) = state.origin {
            span.record("rate_limit.origin", origin.as_str());
        }

        if let Some(ref smoother) = state.smoother {
            span.record("rate_limit.smoother.velocity", smoother.velocity);
            span.record(
                "rate_limit.smoother.micro_interval_secs",
                smoother.micro_interval_secs,
            );
        }

        // Add policy info
        for policy in &state.policies {
            let quota_attr = format!("rate_limit.policy.{}.quota", policy.name);
            let remaining_attr = format!("rate_limit.policy.{}.remaining", policy.name);
            span.record(quota_attr.as_str(), policy.quota);
            span.record(remaining_attr.as_str(), policy.remaining);
        }
    }
}

/// Detailed backend - adds full rate limit state including timing
#[derive(Debug, Clone, Copy, Default)]
pub struct DetailedSpanBackend;

impl RateLimitSpanBackend for DetailedSpanBackend {
    fn enrich_span(&self, state: &RateLimitState) {
        let span = Span::current();

        span.record("rate_limit.enabled", true);
        span.record("rate_limit.will_throttle", state.will_throttle);

        if let Some(ref origin) = state.origin {
            span.record("rate_limit.origin", origin.as_str());
        }

        // Smoother details
        if let Some(ref smoother) = state.smoother {
            span.record("rate_limit.smoother.velocity", smoother.velocity);
            span.record(
                "rate_limit.smoother.micro_interval_secs",
                smoother.micro_interval_secs,
            );
            span.record(
                "rate_limit.smoother.base_window_secs",
                smoother.base_window_secs,
            );
            span.record(
                "rate_limit.smoother.remaining_per_interval",
                smoother.remaining_per_interval,
            );
        }

        // All policy details
        for policy in &state.policies {
            let name_attr = format!("rate_limit.policy.{}.name", policy.name);
            let quota_attr = format!("rate_limit.policy.{}.quota", policy.name);
            let remaining_attr = format!("rate_limit.policy.{}.remaining", policy.name);
            let window_attr = format!("rate_limit.policy.{}.window_secs", policy.name);

            span.record(name_attr.as_str(), policy.name.as_str());
            span.record(quota_attr.as_str(), policy.quota);
            span.record(remaining_attr.as_str(), policy.remaining);
            span.record(window_attr.as_str(), policy.window_secs);

            if let Some(ref reset_at) = policy.reset_at {
                let now = std::time::Instant::now();
                let reset_in_secs = reset_at.saturating_duration_since(now).as_secs();
                let reset_attr = format!("rate_limit.policy.{}.reset_in_secs", policy.name);
                span.record(reset_attr.as_str(), reset_in_secs);
            }
        }

        // Throttle duration
        if let Some(ref duration) = state.throttle_wait_duration {
            span.record("rate_limit.throttle_wait_ms", duration.as_millis());
        }
    }
}

/// No-op backend - no tracing overhead
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpSpanBackend;

impl RateLimitSpanBackend for NoOpSpanBackend {
    fn enrich_span(&self, _state: &RateLimitState) {}
}
