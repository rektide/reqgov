use crate::middleware::http::HttpApiRateLimiter;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;
use std::time::Instant;

pub struct ConcurrencyTracing;

#[async_trait::async_trait]
impl Middleware for ConcurrencyTracing {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        if let Some(rate_limiter) = extensions.get::<Arc<HttpApiRateLimiter>>() {
            let url = req.url().clone();
            let registry = rate_limiter.registry();

            let concurrency_wait_start = Instant::now();
            let global_semaphore = registry.get_global_semaphore().await;
            let domain_semaphore = registry.get_domain_semaphore(&url).await;

            let _global_permit = global_semaphore.acquire().await.unwrap();
            let concurrency_wait = concurrency_wait_start.elapsed();

            let domain_wait_start = Instant::now();
            let _domain_permit = domain_semaphore.acquire().await.unwrap();
            let domain_wait = domain_wait_start.elapsed();

            let span = tracing::Span::current();
            if let Some(max) = registry.max_concurrent_global() {
                span.record("rate_limit.concurrent.global.max", max);
            }
            if let Some(max) = registry.max_concurrent_per_domain() {
                span.record("rate_limit.concurrent.domain.max", max);
            }

            let total_wait = concurrency_wait + domain_wait;
            if total_wait.as_millis() > 0 {
                span.record("rate_limit.concurrent.wait_ms", total_wait.as_millis() as u64);
            }
        }
        next.run(req, extensions).await
    }
}
