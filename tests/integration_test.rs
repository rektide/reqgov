use reqgov::HttpApiRateLimiter;

#[test]
fn test_http_api_rate_limiter_basic() {
    let config = reqgov::SmootherConfig::default();
    let _limiter = HttpApiRateLimiter::new(config);
}
