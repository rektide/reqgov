use crate::origin::origin_limiter::OriginLimiter;
use crate::origin::state::RateLimitViolation;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct StatusTracer;

#[async_trait::async_trait]
impl Middleware for StatusTracer {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(limiter) = extensions.get::<Arc<OriginLimiter>>() {
            let span = tracing::Span::current();
            match limiter.check().await {
                Ok(()) => {
                    span.record("rate_limit.allowed", true);
                    span.record("rate_limit.blocked", false);
                }
                Err(violation) => {
                    span.record("rate_limit.allowed", false);
                    span.record("rate_limit.blocked", true);
                    match violation {
                        RateLimitViolation::Smoothed { wait_duration } => {
                            span.record("rate_limit.blocked_by", "smoother");
                            span.record("rate_limit.wait_ms", wait_duration.as_millis() as u64);
                        }
                        RateLimitViolation::PolicyExceeded { policy_name, wait_duration } => {
                            span.record("rate_limit.blocked_by", policy_name.as_str());
                            span.record("rate_limit.wait_ms", wait_duration.as_millis() as u64);
                        }
                    }
                }
            }
        }
        next.run(req, extensions).await
    }
}
