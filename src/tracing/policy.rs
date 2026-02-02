use crate::origin::origin_limiter::OriginLimiter;
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
        if let Some(limiter) = extensions.get::<Arc<OriginLimiter>>() {
            let span = tracing::Span::current();
            let slots = limiter.slots().await;
            for (name, remaining, quota, window_secs) in slots {
                span.record(
                    format!("rate_limit.policy.{}.remaining", name).as_str(),
                    remaining,
                );
                span.record(
                    format!("rate_limit.policy.{}.quota", name).as_str(),
                    quota,
                );
                span.record(
                    format!("rate_limit.policy.{}.window_secs", name).as_str(),
                    window_secs,
                );
            }
        }
        next.run(req, extensions).await
    }
}
