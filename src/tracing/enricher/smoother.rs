use super::enricher_trait::SpanEnricher;
use crate::limiter::context::SpanContext;
use tracing::Span;

#[derive(Debug, Clone, Copy, Default)]
pub struct SmootherEnricher;

impl SpanEnricher for SmootherEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
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
