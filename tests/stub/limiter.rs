use reqwest_ratelimit::RateLimiter;
use std::time::Duration;

pub struct StubRateLimiter;

impl RateLimiter for StubRateLimiter {
    fn acquire_permit(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
