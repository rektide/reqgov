use crate::limiter::origin::OriginRateLimiter;
use crate::limiter::state::RateLimitViolation;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct StatusTracing;

#[async_trait::async_trait]
impl Middleware for StatusTracing {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(limiter) = extensions.get::<Arc<OriginRateLimiter>>() {
            let span = tracing::Span::current();
            match limiter.check() {
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
