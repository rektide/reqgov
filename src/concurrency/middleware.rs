use crate::concurrency::limiter::ConcurrencyRateLimiter;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct ConcurrencyLimiterMiddleware {
    rate_limiter: Arc<ConcurrencyRateLimiter>,
}

impl ConcurrencyLimiterMiddleware {
    pub fn new(rate_limiter: Arc<ConcurrencyRateLimiter>) -> Self {
        Self { rate_limiter }
    }
}

impl Clone for ConcurrencyLimiterMiddleware {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ConcurrencyLimiterMiddleware {
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
