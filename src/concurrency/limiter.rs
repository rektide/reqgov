use crate::registry::origin::OriginRegistry;
use std::sync::Arc;

pub struct ConcurrencyRateLimiterBuilder {
    registry: Option<Arc<OriginRegistry>>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl Default for ConcurrencyRateLimiterBuilder {
    fn default() -> Self {
        Self {
            registry: None,
            max_concurrent_global: None,
            max_concurrent_per_domain: None,
        }
    }
}

impl ConcurrencyRateLimiterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(mut self, registry: Arc<OriginRegistry>) -> Self {
        self.registry = Some(registry);
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

    pub fn build(self) -> ConcurrencyRateLimiter {
        let registry = self.registry.unwrap_or_else(|| {
            let mut builder = OriginRegistry::builder();
            if let Some(max) = self.max_concurrent_global {
                builder = builder.max_concurrent_global(max);
            }
            if let Some(max) = self.max_concurrent_per_domain {
                builder = builder.max_concurrent_per_domain(max);
            }
            Arc::new(builder.build())
        });

        ConcurrencyRateLimiter {
            registry,
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
}

pub struct ConcurrencyRateLimiter {
    registry: Arc<OriginRegistry>,
    current_url: Arc<tokio::sync::RwLock<Option<url::Url>>>,
}

impl ConcurrencyRateLimiter {
    pub fn builder() -> ConcurrencyRateLimiterBuilder {
        ConcurrencyRateLimiterBuilder::new()
    }

    pub fn registry(&self) -> &Arc<OriginRegistry> {
        &self.registry
    }

    pub async fn set_url(&self, url: url::Url) {
        *self.current_url.write().await = Some(url);
    }

    pub async fn get_global_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.registry.get_global_semaphore().await
    }

    pub async fn get_domain_semaphore(&self, url: &url::Url) -> Arc<tokio::sync::Semaphore> {
        self.registry.get_domain_semaphore(url).await
    }

    pub fn max_concurrent_global(&self) -> Option<usize> {
        self.registry.max_concurrent_global()
    }

    pub fn max_concurrent_per_domain(&self) -> Option<usize> {
        self.registry.max_concurrent_per_domain()
    }
}

impl reqwest_ratelimit::RateLimiter for ConcurrencyRateLimiter {
    fn acquire_permit(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            if let Some(ref url) = *self.current_url.read().await {
                let global_semaphore = self.registry.get_global_semaphore().await;
                let domain_semaphore = self.registry.get_domain_semaphore(url).await;

                let _global_permit = global_semaphore.acquire().await.unwrap();
                let _domain_permit = domain_semaphore.acquire().await.unwrap();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_creation() {
        let _limiter = ConcurrencyRateLimiter::builder().build();
    }

    #[test]
    fn test_limiter_with_concurrency_limits() {
        let limiter = ConcurrencyRateLimiter::builder()
            .max_concurrent_global(100)
            .max_concurrent_per_domain(10)
            .build();
        assert_eq!(limiter.max_concurrent_global(), Some(100));
        assert_eq!(limiter.max_concurrent_per_domain(), Some(10));
    }

    #[tokio::test]
    async fn test_limiter_set_url() {
        let limiter = ConcurrencyRateLimiter::builder().build();
        let url = url::Url::parse("https://api.example.com/test").unwrap();
        limiter.set_url(url).await;
    }

    #[tokio::test]
    async fn test_limiter_registry_access() {
        let limiter = ConcurrencyRateLimiter::builder().build();
        let _registry = limiter.registry();
    }

    #[tokio::test]
    async fn test_limiter_get_semaphores() {
        let limiter = ConcurrencyRateLimiter::builder()
            .max_concurrent_global(100)
            .max_concurrent_per_domain(10)
            .build();
        let url = url::Url::parse("https://api.example.com/test").unwrap();
        limiter.set_url(url.clone()).await;

        let _global = limiter.get_global_semaphore().await;
        let _domain = limiter.get_domain_semaphore(&url).await;
    }
}
