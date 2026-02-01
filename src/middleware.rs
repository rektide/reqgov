use crate::origin_registry::OriginRegistry;
use crate::smoother::SmootherConfig;
use std::sync::Arc;

pub struct HttpApiRateLimiter {
    registry: Arc<OriginRegistry>,
    current_url: Arc<tokio::sync::RwLock<Option<url::Url>>>,
}

impl HttpApiRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            registry: Arc::new(OriginRegistry::new(smoother_config)),
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn with_concurrency_limits(
        smoother_config: SmootherConfig,
        max_concurrent_global: Option<usize>,
        max_concurrent_per_domain: Option<usize>,
    ) -> Self {
        Self {
            registry: Arc::new(OriginRegistry::with_concurrency_limits(
                smoother_config,
                max_concurrent_global,
                max_concurrent_per_domain,
            )),
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn with_registry(registry: Arc<OriginRegistry>) -> Self {
        Self {
            registry,
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn registry(&self) -> &Arc<OriginRegistry> {
        &self.registry
    }

    pub async fn set_url(&self, url: url::Url) {
        *self.current_url.write().await = Some(url);
    }
}

impl reqwest_ratelimit::RateLimiter for HttpApiRateLimiter {
    fn acquire_permit(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            if let Some(ref url) = *self.current_url.read().await {
                let global_semaphore = self.registry.get_global_semaphore().await;
                let domain_semaphore = self.registry.get_domain_semaphore(url).await;
                let limiter = self.registry.get_limiter(url).await;

                let _global_permit = global_semaphore.acquire().await.unwrap();
                let _domain_permit = domain_semaphore.acquire().await.unwrap();
                limiter.read().await.wait().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest_ratelimit::RateLimiter;

    #[test]
    fn test_limiter_creation() {
        let config = SmootherConfig::default();
        let _limiter = HttpApiRateLimiter::new(config);
    }

    #[test]
    fn test_limiter_with_concurrency_limits() {
        let config = SmootherConfig::default();
        let limiter = HttpApiRateLimiter::with_concurrency_limits(config, Some(100), Some(10));
        assert_eq!(limiter.registry().max_concurrent_global(), Some(100));
        assert_eq!(limiter.registry().max_concurrent_per_domain(), Some(10));
    }

    #[tokio::test]
    async fn test_limiter_acquire_permit() {
        let config = SmootherConfig::default();
        let limiter = Arc::new(HttpApiRateLimiter::new(config));

        let url = url::Url::parse("https://api.example.com/test").unwrap();
        limiter.set_url(url).await;

        limiter.acquire_permit().await;
    }

    #[tokio::test]
    async fn test_limiter_registry_access() {
        let config = SmootherConfig::default();
        let limiter = HttpApiRateLimiter::new(config);
        let _registry = limiter.registry();
    }
}
