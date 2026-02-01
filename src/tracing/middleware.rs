/// Rate limiting telemetry middleware for reqwest-tracing spans
///
/// This middleware integrates with reqwest-tracing to enrich HTTP request spans
/// with rate limiting telemetry from governor state. It does NOT create new spans,
/// but adds attributes to spans created by reqwest-tracing.
///
/// # Example Usage with OriginRateLimiter
///
/// ```ignore
/// use reqwest_middleware::ClientBuilder;
/// use reqwest_tracing::TracingMiddleware;
/// use reqgov::{HttpApiRateLimiter, RateLimitTelemetry, SmootherConfig};
/// use std::sync::Arc;
///
/// let rate_limiter = Arc::new(HttpApiRateLimiter::new(SmootherConfig::default()));
/// let telemetry = RateLimitTelemetry::new(rate_limiter.clone());
///
/// let client = ClientBuilder::new(reqwest::Client::new())
///     .with(TracingMiddleware::default())
///     .with(reqwest_ratelimit::all(rate_limiter))
///     .with(telemetry)
///     .build();
/// ```

use crate::middleware::http::HttpApiRateLimiter;
use crate::limiter::origin::OriginRateLimiter;
use crate::tracing::enricher::enricher_trait::SpanEnricher;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;
use tracing::Span;

pub struct RateLimitTelemetry<S: SpanEnricher> {
    rate_limiter: Arc<OriginRateLimiter>,
    span_enricher: S,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: SpanEnricher + Default> RateLimitTelemetry<S> {
    pub fn new(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_enricher: S::default(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: SpanEnricher> RateLimitTelemetry<S> {
    pub fn with_backend(rate_limiter: Arc<OriginRateLimiter>, span_enricher: S) -> Self {
        Self {
            rate_limiter,
            span_enricher,
            _phantom: std::marker::PhantomData,
        }
    }
}

// Convenience constructors for common span enrichers
impl RateLimitTelemetry<crate::tracing::enricher::minimal::MinimalSpanEnricher> {
    pub fn new_minimal(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl RateLimitTelemetry<crate::tracing::enricher::standard::StandardSpanEnricher> {
    pub fn new_standard(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl RateLimitTelemetry<crate::tracing::enricher::detailed::DetailedSpanEnricher> {
    pub fn new_detailed(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl<S: SpanEnricher + Clone> Clone for RateLimitTelemetry<S> {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
            span_enricher: self.span_enricher.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<S: SpanEnricher + Send + Sync + 'static> Middleware for RateLimitTelemetry<S> {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        // Get current limiter state (this triggers check() if using fresh state)
        let limiter_state = self.rate_limiter.state();
        
        // Get the rich SpanContext that was captured during check()
        if let Some(span_context) = limiter_state.span_context.as_ref() {
            // Enrich the current span with rate limit telemetry
            let span = Span::current();
            self.span_enricher.enrich(&span, span_context);
        }
        
        // Execute request (rate limiting handled by reqwest_ratelimit middleware)
        next.run(req, extensions).await
    }
}

/// Telemetry middleware that includes concurrency limiting metrics
///
/// This middleware works with `HttpApiRateLimiter` to capture concurrency
/// limiting telemetry (semaphore wait times) and enriches tracing spans.
///
/// # Example
///
/// ```ignore
/// use reqwest_middleware::ClientBuilder;
/// use reqgov::{HttpApiRateLimiter, ConcurrencyTelemetry, SmootherConfig};
/// use std::sync::Arc;
///
/// let rate_limiter = Arc::new(HttpApiRateLimiter::with_concurrency_limits(
///     SmootherConfig::default(),
///     Some(100),  // max 100 global concurrent requests
///     Some(10),   // max 10 per domain
/// ));
/// let telemetry = ConcurrencyTelemetry::new(rate_limiter.clone());
///
/// let client = ClientBuilder::new(reqwest::Client::new())
///     .with(telemetry)
///     .build();
/// ```
pub struct ConcurrencyTelemetry {
    rate_limiter: Arc<HttpApiRateLimiter>,
}

impl ConcurrencyTelemetry {
    pub fn new(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self { rate_limiter }
    }
}

impl Clone for ConcurrencyTelemetry {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ConcurrencyTelemetry {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        _extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        use std::time::Instant;

        let url = req.url().clone();
        let registry = self.rate_limiter.registry();
        
        // Track semaphore wait times
        let concurrency_wait_start = Instant::now();
        let global_semaphore = registry.get_global_semaphore().await;
        let domain_semaphore = registry.get_domain_semaphore(&url).await;
        
        let _global_permit = global_semaphore.acquire().await.unwrap();
        let concurrency_wait = concurrency_wait_start.elapsed();
        
        let domain_wait_start = Instant::now();
        let _domain_permit = domain_semaphore.acquire().await.unwrap();
        let domain_wait = domain_wait_start.elapsed();
        
        // Enrich span with concurrency telemetry
        let global_max = registry.max_concurrent_global();
        let domain_max = registry.max_concurrent_per_domain();
        let total_wait = concurrency_wait + domain_wait;
        
        if let Some(max) = global_max {
            tracing::Span::current().record("rate_limit.concurrent.global.max", max);
        }
        if let Some(max) = domain_max {
            tracing::Span::current().record("rate_limit.concurrent.domain.max", max);
        }
        if total_wait.as_millis() > 0 {
            tracing::Span::current().record("rate_limit.concurrent.wait_ms", total_wait.as_millis());
        }
        
        // Execute request
        next.run(req, _extensions).await
    }
}
