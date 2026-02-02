use crate::origin::origin::OriginRateLimiter;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct PolicyTracer;

#[async_trait::async_trait]
impl Middleware for PolicyTracer {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(limiter) = extensions.get::<Arc<OriginRateLimiter>>() {
            let span = tracing::Span::current();
            for (name, slot) in limiter.slots() {
                span.record(
                    format!("rate_limit.policy.{}.remaining", name).as_str(),
                    slot.governor_remaining(),
                );
                span.record(
                    format!("rate_limit.policy.{}.quota", name).as_str(),
                    slot.policy.quota,
                );
                span.record(
                    format!("rate_limit.policy.{}.window_secs", name).as_str(),
                    slot.policy.window_secs.unwrap_or(60),
                );
            }
        }
        next.run(req, extensions).await
    }
}
