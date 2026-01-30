// Include stub module
#[path = "../test/stub/limiter.rs"]
mod stub_limiter;

use reqwest_ratelimit::RateLimiter;
use stub_limiter::StubRateLimiter;

#[tokio::test]
async fn simple_ratelimit_test() {
    let limiter = StubRateLimiter;
    RateLimiter::acquire_permit(&limiter).await;
    println!("Permit acquired after 100ms");
}
