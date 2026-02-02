use reqgov::{HttpApiRateLimiter, OriginRegistry, SmootherConfig};
use reqwest_ratelimit::RateLimiter;
use std::sync::Arc;

#[tokio::test]
async fn test_http_api_limiter_full_flow() {
    let limiter = HttpApiRateLimiter::builder()
        .smoother(SmootherConfig::default())
        .build();

    let url = url::Url::parse("https://api.example.com/test").unwrap();
    limiter.set_url(url.clone()).await;

    limiter.acquire_permit().await;
}

#[tokio::test]
async fn test_registry_multiple_origins() {
    let registry = Arc::new(
        OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build()
    );

    let url1 = url::Url::parse("https://api.github.com/repos").unwrap();
    let url2 = url::Url::parse("https://api.gitlab.com/projects").unwrap();
    let url3 = url::Url::parse("https://api.custom.com/data").unwrap();

    let limiter1 = registry.get_limiter(&url1).await;
    let limiter2 = registry.get_limiter(&url2).await;
    let limiter3 = registry.get_limiter(&url3).await;

    limiter1.read().await.wait().await;
    limiter2.read().await.wait().await;
    limiter3.read().await.wait().await;
}

#[tokio::test]
async fn test_registry_same_origin_same_limiter() {
    let registry = Arc::new(
        OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build()
    );

    let url1 = url::Url::parse("https://api.example.com/endpoint1").unwrap();
    let url2 = url::Url::parse("https://api.example.com/endpoint2").unwrap();

    let limiter1 = registry.get_limiter(&url1).await;
    let limiter2 = registry.get_limiter(&url2).await;

    assert!(Arc::ptr_eq(&limiter1, &limiter2));
}

#[tokio::test]
async fn test_registry_update_from_response() {
    let registry = Arc::new(
        OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build()
    );

    let url = url::Url::parse("https://api.example.com/test").unwrap();

    let mut headers = http::HeaderMap::new();
    headers.insert(
        "ratelimit-policy",
        r#""burst";q=100;w=60, "daily";q=1000;w=86400"#
            .parse()
            .unwrap(),
    );
    headers.insert(
        "ratelimit",
        r#""burst";r=45;t=30, "daily";r=850"#
            .parse()
            .unwrap(),
    );

    registry.update_from_response(&url, &headers).await;

    let limiter = registry.get_limiter(&url).await;
    limiter.read().await.check().unwrap();
}

#[tokio::test]
async fn test_registry_concurrent_access() {
    let registry = Arc::new(
        OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build()
    );

    let url = url::Url::parse("https://api.example.com/test").unwrap();

    let mut handles = vec![];
    for _ in 0..10 {
        let registry = Arc::clone(&registry);
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            let limiter = registry.get_limiter(&url).await;
            limiter.read().await.wait().await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_http_api_limiter_concurrent_requests() {
    let limiter = Arc::new(
        HttpApiRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .build()
    );

    let url = url::Url::parse("https://api.example.com/test").unwrap();
    limiter.set_url(url.clone()).await;

    let mut handles = vec![];
    for _ in 0..5 {
        let limiter = Arc::clone(&limiter);
        handles.push(tokio::spawn(async move {
            limiter.acquire_permit().await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
