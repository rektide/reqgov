use super::concurrency::ConcurrencySpanEnricher;
use super::detailed::DetailedSpanEnricher;
use super::enricher_trait::SpanEnricher;
use super::minimal::MinimalSpanEnricher;
use super::smoother::SmootherEnricher;
use super::standard::StandardSpanEnricher;
use crate::limiter::context::SpanContext;
use std::sync::Arc;

pub struct ChainedEnricher {
    enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>,
}

impl SpanEnricher for ChainedEnricher {
    fn enrich(&self, span: &tracing::Span, context: &SpanContext) {
        for enricher in &self.enrichers {
            if enricher.is_enabled() {
                enricher.enrich(span, context);
            }
        }
    }
}

impl ChainedEnricher {
    pub fn new() -> Self {
        Self {
            enrichers: Vec::new(),
        }
    }

    pub fn with_enricher<E: SpanEnricher + 'static>(mut self, enricher: E) -> Self {
        self.enrichers.push(Arc::new(enricher));
        self
    }

    pub fn with_enrichers<E: SpanEnricher + 'static>(mut self, enrichers: Vec<E>) -> Self {
        for enricher in enrichers {
            self.enrichers.push(Arc::new(enricher));
        }
        self
    }

    pub fn with_dyn_enrichers(
        mut self,
        enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>,
    ) -> Self {
        for enricher in enrichers {
            self.enrichers.push(enricher);
        }
        self
    }

    pub fn build(self) -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(self)
    }
}

pub struct EnricherPresets;

impl EnricherPresets {
    pub fn minimal() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(MinimalSpanEnricher)
    }

    pub fn standard() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(StandardSpanEnricher)
    }

    pub fn detailed() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(DetailedSpanEnricher)
    }

    pub fn production() -> Box<dyn SpanEnricher + Send + Sync> {
        ChainedEnricher::new()
            .with_enricher(SmootherEnricher)
            .with_enricher(StandardSpanEnricher)
            .build()
    }

    pub fn debug() -> Box<dyn SpanEnricher + Send + Sync> {
        ChainedEnricher::new()
            .with_enricher(SmootherEnricher)
            .with_enricher(DetailedSpanEnricher)
            .build()
    }

    pub fn custom(
        enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>,
    ) -> Box<dyn SpanEnricher + Send + Sync> {
        ChainedEnricher::new().with_dyn_enrichers(enrichers).build()
    }

    pub fn concurrency() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(ConcurrencySpanEnricher)
    }
}
