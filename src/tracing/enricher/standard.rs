use super::enricher_trait::SpanEnricher;
use crate::limiter::context::SpanContext;
use tracing::Span;

#[derive(Debug, Clone, Copy, Default)]
pub struct StandardSpanEnricher;

impl SpanEnricher for StandardSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.allowed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());

        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy);
        }

        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name);
            span.record(remaining_key.as_str(), state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name);
            span.record(quota_key.as_str(), state.quota);
        }
    }
}
