use super::enricher_trait::SpanEnricher;
use crate::limiter::context::SpanContext;
use tracing::Span;

#[derive(Debug, Clone, Copy, Default)]
pub struct ConcurrencySpanEnricher;

impl SpanEnricher for ConcurrencySpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        let concurrency = &context.extensions.concurrency;

        if let Some(global_max) = concurrency.global_max {
            span.record("rate_limit.concurrent.global.max", global_max);
        }

        if let Some(domain_max) = concurrency.domain_max {
            span.record("rate_limit.concurrent.domain.max", domain_max);
        }

        if let Some(wait_duration) = concurrency.wait_duration {
            span.record("rate_limit.concurrent.wait_ms", wait_duration.as_millis());
        }
    }
}
