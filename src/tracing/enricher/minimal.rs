use super::enricher_trait::SpanEnricher;
use crate::limiter::context::SpanContext;
use tracing::Span;

#[derive(Debug, Clone, Copy, Default)]
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
