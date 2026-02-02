use crate::origin::smoother_limiter::SmootherLimiter;
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
        if let Some(limiter) = extensions.get::<Arc<SmootherLimiter>>() {
            let smoother_read = limiter.smoother.read().await;
            if let Some(ref smoother) = *smoother_read {
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
