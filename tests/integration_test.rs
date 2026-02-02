use reqgov::{ConcurrencyRateLimiter, OriginRateLimiter, SmootherConfig};

#[test]
fn test_concurrency_rate_limiter_basic() {
    let _limiter = ConcurrencyRateLimiter::builder().build();
}

#[test]
fn test_origin_rate_limiter_basic() {
    let _limiter = OriginRateLimiter::builder()
        .smoother(SmootherConfig::default())
        .build();
}
