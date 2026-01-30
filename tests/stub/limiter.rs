use async_trait::async_trait;
use reqwest_ratelimit::RateLimiter;
use std::time::Duration;

pub struct StubRateLimiter;

#[async_trait]
impl RateLimiter for StubRateLimiter {
    async fn acquire_permit(&self) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
