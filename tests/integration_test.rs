// Include stub module
pub mod stub {
    pub mod limiter;
}

use reqwest_ratelimit::RateLimiter;
use stub::limiter::StubRateLimiter;

#[tokio::test]
async fn simple_ratelimit_test() {
    let limiter = StubRateLimiter;
    RateLimiter::acquire_permit(&limiter).await;
    println!("Permit acquired after 100ms");
}
