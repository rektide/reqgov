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
            limiter.for_each_slot(|slot| {
                span.record(
                    format!("rate_limit.policy.{}.remaining", slot.policy.name).as_str(),
                    slot.governor_remaining(),
                );
                span.record(
                    format!("rate_limit.policy.{}.quota", slot.policy.name).as_str(),
                    slot.policy.quota,
                );
                span.record(
                    format!("rate_limit.policy.{}.window_secs", slot.policy.name).as_str(),
                    slot.policy.window_secs.unwrap_or(60),
                );
                std::future::ready(())
            }).await;
        }
        next.run(req, extensions).await
    }
}
