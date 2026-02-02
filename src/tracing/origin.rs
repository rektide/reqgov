use crate::origin::check_result::RateLimitCheckResult;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};

pub struct StatusTracer;

#[async_trait::async_trait]
impl Middleware for StatusTracer {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(result) = extensions.get::<RateLimitCheckResult>() {
            let span = tracing::Span::current();

            span.record("rate_limit.allowed", result.allowed);
            span.record("rate_limit.blocked", !result.allowed);

            if let Some(wait_duration) = result.wait_duration {
                span.record("rate_limit.wait_ms", wait_duration.as_millis() as u64);
            }
        }
        next.run(req, extensions).await
    }
}
