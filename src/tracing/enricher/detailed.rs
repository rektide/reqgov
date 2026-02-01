use super::enricher_trait::SpanEnricher;
use crate::limiter::context::SpanContext;
use tracing::Span;

pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        let mode_str = format!("{:?}", context.metadata.mode);
        span.record("rate_limit.mode", mode_str);
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        span.record(
            "rate_limit.timestamp_ms",
            context.metadata.timestamp.elapsed().as_millis(),
        );

        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }

        if let Some(ref smoother_state) = context.extensions.smoother_state {
            span.record(
                "rate_limit.smoother.remaining_per_interval",
                smoother_state.remaining_per_interval,
            );
            span.record(
                "rate_limit.smoother.micro_interval_secs",
                smoother_state.micro_interval_secs,
            );
            span.record("rate_limit.smoother.velocity", smoother_state.velocity);
        }
    }
}
