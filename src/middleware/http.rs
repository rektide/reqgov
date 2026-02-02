use crate::registry::origin::OriginRegistry;
use crate::smoothing::smoother::SmootherConfig;
use std::sync::Arc;

pub struct HttpApiRateLimiterBuilder {
    registry: Option<Arc<OriginRegistry>>,
    smoother_config: Option<SmootherConfig>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl Default for HttpApiRateLimiterBuilder {
    fn default() -> Self {
        Self {
            registry: None,
            smoother_config: None,
            max_concurrent_global: None,
            max_concurrent_per_domain: None,
        }
    }
}

impl HttpApiRateLimiterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(mut self, registry: Arc<OriginRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn smoother(mut self, config: SmootherConfig) -> Self {
        self.smoother_config = Some(config);
        self
    }

    pub fn max_concurrent_global(mut self, max: usize) -> Self {
        self.max_concurrent_global = Some(max);
        self
    }

    pub fn max_concurrent_per_domain(mut self, max: usize) -> Self {
        self.max_concurrent_per_domain = Some(max);
        self
    }

    pub fn build(self) -> HttpApiRateLimiter {
        let registry = self.registry.unwrap_or_else(|| {
            let mut builder = OriginRegistry::builder()
                .smoother(self.smoother_config.unwrap_or_default());
            if let Some(max) = self.max_concurrent_global {
                builder = builder.max_concurrent_global(max);
            }
            if let Some(max) = self.max_concurrent_per_domain {
                builder = builder.max_concurrent_per_domain(max);
            }
            Arc::new(builder.build())
        });

        HttpApiRateLimiter {
            registry,
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
}

pub struct HttpApiRateLimiter {
    registry: Arc<OriginRegistry>,
    current_url: Arc<tokio::sync::RwLock<Option<url::Url>>>,
}

impl HttpApiRateLimiter {
    pub fn builder() -> HttpApiRateLimiterBuilder {
        HttpApiRateLimiterBuilder::new()
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
        let _limiter = HttpApiRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .build();
    }

    #[test]
    fn test_limiter_with_concurrency_limits() {
        let limiter = HttpApiRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .max_concurrent_global(100)
            .max_concurrent_per_domain(10)
            .build();
        assert_eq!(limiter.registry().max_concurrent_global(), Some(100));
        assert_eq!(limiter.registry().max_concurrent_per_domain(), Some(10));
    }

    #[tokio::test]
    async fn test_limiter_acquire_permit() {
        let limiter = Arc::new(
            HttpApiRateLimiter::builder()
                .smoother(SmootherConfig::default())
                .build()
        );

        let url = url::Url::parse("https://api.example.com/test").unwrap();
        limiter.set_url(url).await;

        limiter.acquire_permit().await;
    }

    #[tokio::test]
    async fn test_limiter_registry_access() {
        let limiter = HttpApiRateLimiter::builder()
            .smoother(SmootherConfig::default())
            .build();
        let _registry = limiter.registry();
    }
}
