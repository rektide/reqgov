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
///
/// # Example Usage with HttpApiRateLimiter (with concurrency limiting)
///
/// ```ignore
/// use reqwest_middleware::ClientBuilder;
/// use reqwest_tracing::TracingMiddleware;
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
///     .with(TracingMiddleware::default())
///     .with(reqwest_ratelimit::all(rate_limiter))
///     .with(telemetry)
///     .build();
/// ```
///
/// # Note
///
/// To use the doctest example, you need the following dependencies in Cargo.toml:
/// - `reqwest` - for reqwest::Client
/// - `reqwest-middleware` - for ClientBuilder
/// - `reqwest-tracing` - for TracingMiddleware
/// - `reqwest-ratelimit` - for reqwest_ratelimit::all
/// ```

use crate::middleware::HttpApiRateLimiter;
use crate::origin_limiter::OriginRateLimiter;
use crate::tracing::{RateLimitSpanBackend, RateLimitState};
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;
/// Usage with reqwest-tracing:
/// ```ignore
/// use reqwest_middleware::ClientBuilder;
/// use reqgov::{HttpApiRateLimiter, RateLimitTelemetry, SmootherConfig};
/// use std::sync::Arc;
/// 
/// let rate_limiter = Arc::new(HttpApiRateLimiter::new(SmootherConfig::default()));
/// let telemetry = RateLimitTelemetry::new(rate_limiter.clone());
/// 
/// // Note: This example assumes you have reqwest-tracing and reqwest-ratelimit configured
/// let client = ClientBuilder::new(/* reqwest::Client::new() */)
///     .with(/* TracingMiddleware::default() */)
///     .with(/* reqwest_ratelimit::all(rate_limiter) */)
///     .with(telemetry)
///     .build();
/// ```
pub struct RateLimitTelemetry<S: RateLimitSpanBackend> {
    rate_limiter: Arc<OriginRateLimiter>,
    span_backend: S,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: RateLimitSpanBackend + Default> RateLimitTelemetry<S> {
    pub fn new(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: S::default(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: RateLimitSpanBackend> RateLimitTelemetry<S> {
    pub fn with_backend(rate_limiter: Arc<OriginRateLimiter>, span_backend: S) -> Self {
        Self {
            rate_limiter,
            span_backend,
            _phantom: std::marker::PhantomData,
        }
    }
}

// Convenience constructors for common span backends
impl RateLimitTelemetry<crate::tracing::MinimalSpanBackend> {
    pub fn new_minimal(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl RateLimitTelemetry<crate::tracing::StandardSpanBackend> {
    pub fn new_standard(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl RateLimitTelemetry<crate::tracing::DetailedSpanBackend> {
    pub fn new_detailed(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self::new(rate_limiter)
    }
}

impl<S: RateLimitSpanBackend> Clone for RateLimitTelemetry<S> {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
            span_backend: self.span_backend.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<S: RateLimitSpanBackend + Send + Sync + 'static> Middleware for RateLimitTelemetry<S> {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        // Get current limiter state before request
        let limiter_state = self.rate_limiter.state();
        
        // Store URL for enrichment
        let url = req.url().as_str().to_string();
        let rate_limit_state = RateLimitState {
            origin: Some(url),
            smoother: limiter_state.smoother,
            policies: limiter_state.policies,
            will_throttle: limiter_state.will_throttle,
            throttle_wait_duration: limiter_state.throttle_wait_duration,
            concurrency: None,
        };
        
        // Store state in extensions for use after request
        extensions.insert(rate_limit_state);
        
        // Execute request (rate limiting handled by reqwest_ratelimit middleware)
        let result = next.run(req, extensions).await;
        
        // Enrich the span created by reqwest-tracing with rate limit state
        if let Some(state) = extensions.get::<crate::tracing::RateLimitState>() {
            self.span_backend.enrich_span(state);
        }
        
        result
    }
}

/// Telemetry middleware that includes concurrency limiting metrics
///
/// This middleware works with `HttpApiRateLimiter` to capture both
/// rate limiting and concurrency limiting telemetry.
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
pub struct ConcurrencyTelemetry<S: crate::tracing::RateLimitSpanBackend> {
    rate_limiter: Arc<HttpApiRateLimiter>,
    span_backend: S,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: crate::tracing::RateLimitSpanBackend + Default> ConcurrencyTelemetry<S> {
    pub fn new(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: S::default(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: crate::tracing::RateLimitSpanBackend> ConcurrencyTelemetry<S> {
    pub fn with_backend(rate_limiter: Arc<HttpApiRateLimiter>, span_backend: S) -> Self {
        Self {
            rate_limiter,
            span_backend,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: crate::tracing::RateLimitSpanBackend> Clone for ConcurrencyTelemetry<S> {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
            span_backend: self.span_backend.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<S: crate::tracing::RateLimitSpanBackend + Send + Sync + 'static> Middleware
    for ConcurrencyTelemetry<S>
{
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
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

        // Get origin key for rate limiter lookup
        let origin_key = format!("{}://{}", url.scheme(), url.host_str().unwrap_or("unknown"));

        // Get rate limiter state
        let limiter = registry.get_limiter(&url).await;
        let limiter_state = limiter.read().await.state();

        // Calculate active concurrent requests (approximate)
        let global_max = registry.max_concurrent_global();
        let domain_max = registry.max_concurrent_per_domain();

        let rate_limit_state = crate::tracing::RateLimitState {
            origin: Some(origin_key),
            smoother: limiter_state.smoother,
            policies: limiter_state.policies,
            will_throttle: limiter_state.will_throttle,
            throttle_wait_duration: limiter_state.throttle_wait_duration,
            concurrency: Some(crate::tracing::ConcurrencyState {
                global_active: None, // Would need to track this separately
                global_max,
                domain_active: None, // Would need to track this separately
                domain_max,
                wait_duration: Some(concurrency_wait + domain_wait),
            }),
        };

        // Store state in extensions
        extensions.insert(rate_limit_state);

        // Execute request
        let result = next.run(req, extensions).await;

        // Enrich span
        if let Some(state) = extensions.get::<crate::tracing::RateLimitState>() {
            self.span_backend.enrich_span(state);
        }

        result
    }
}
