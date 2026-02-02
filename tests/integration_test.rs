use reqgov::HttpApiRateLimiter;

#[test]
fn test_http_api_rate_limiter_basic() {
    let _limiter = HttpApiRateLimiter::builder()
        .smoother(reqgov::SmootherConfig::default())
        .build();
}
