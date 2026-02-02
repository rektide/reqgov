use crate::limiter::origin::OriginRateLimiter;
use crate::middleware::http::HttpApiRateLimiter;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

/// Rate limiting tracing middleware that registers the limiter in extensions.
///
/// This middleware makes the rate limiter available to subsequent tracing
/// middleware (PolicyTracing, SmootherTracing, StatusTracing) via extensions.
///
/// # Example
///
/// ```ignore
/// use reqwest_middleware::ClientBuilder;
/// use reqgov::{RateLimitTracing, PolicyTracing, SmootherTracing, StatusTracing};
/// use std::sync::Arc;
///
/// let limiter = Arc::new(OriginRateLimiter::new());
///
/// let client = ClientBuilder::new(reqwest::Client::new())
///     .with(RateLimitTracing::new(limiter.clone()))
///     .with(PolicyTracing)
///     .with(SmootherTracing)
///     .with(StatusTracing)
///     .build();
/// ```
pub struct RateLimitTracing {
    rate_limiter: Arc<OriginRateLimiter>,
}

impl RateLimitTracing {
    pub fn new(rate_limiter: Arc<OriginRateLimiter>) -> Self {
        Self { rate_limiter }
    }
}

impl Clone for RateLimitTracing {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for RateLimitTracing {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        extensions.insert(Arc::clone(&self.rate_limiter));
        next.run(req, extensions).await
    }
}

/// Concurrency tracing middleware that registers HttpApiRateLimiter in extensions.
///
/// This middleware makes the HTTP API rate limiter available to ConcurrencyTracing
/// middleware via extensions.
pub struct ConcurrencyTracing {
    rate_limiter: Arc<HttpApiRateLimiter>,
}

impl ConcurrencyTracing {
    pub fn new(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self { rate_limiter }
    }
}

impl Clone for ConcurrencyTracing {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ConcurrencyTracing {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        extensions.insert(Arc::clone(&self.rate_limiter));
        next.run(req, extensions).await
    }
}
