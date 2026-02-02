use crate::origin::origin::OriginRateLimiter;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct SmootherTracer;

#[async_trait::async_trait]
impl Middleware for SmootherTracer {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(limiter) = extensions.get::<Arc<OriginRateLimiter>>() {
            if let Some(smoother) = limiter.smoother() {
                let span = tracing::Span::current();
                span.record("rate_limit.smoother.remaining", smoother.remaining());
                span.record("rate_limit.smoother.velocity", smoother.velocity);
                span.record("rate_limit.smoother.micro_interval_secs", smoother.micro_interval_secs);
                span.record("rate_limit.smoother.base_window_secs", smoother.base_window_secs);
            }
        }
        next.run(req, extensions).await
    }
}
