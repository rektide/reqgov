use crate::limiter::context::SpanContext;
use tracing::Span;

pub trait SpanEnricher: Send + Sync {
    fn enrich(&self, span: &Span, context: &SpanContext);
    fn is_enabled(&self) -> bool {
        true
    }
}
