use crate::concurrency::registry::ConcurrencyRegistry;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

#[derive(Default)]
pub struct ConcurrencyRateLimiterBuilder {
    registry: Option<Arc<ConcurrencyRegistry>>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl ConcurrencyRateLimiterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(mut self, registry: Arc<ConcurrencyRegistry>) -> Self {
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
            let mut builder = ConcurrencyRegistry::builder();
            if let Some(max) = self.max_concurrent_global {
                builder = builder.max_concurrent_global(max);
            }
            if let Some(max) = self.max_concurrent_per_domain {
                builder = builder.max_concurrent_per_domain(max);
            }
            Arc::new(builder.build())
        });

        ConcurrencyRateLimiter {
            inner: Arc::new(ConcurrencyRateLimiterInner {
                registry,
                current_url: tokio::sync::RwLock::new(None),
            }),
        }
    }
}

struct ConcurrencyRateLimiterInner {
    registry: Arc<ConcurrencyRegistry>,
    current_url: tokio::sync::RwLock<Option<url::Url>>,
}

/// Concurrency rate limiter with internal Arc for cheap cloning.
/// Can be used directly as reqwest middleware.
#[derive(Clone)]
pub struct ConcurrencyRateLimiter {
    inner: Arc<ConcurrencyRateLimiterInner>,
}

impl ConcurrencyRateLimiter {
    pub fn builder() -> ConcurrencyRateLimiterBuilder {
        ConcurrencyRateLimiterBuilder::new()
    }

    pub fn registry(&self) -> &Arc<ConcurrencyRegistry> {
        &self.inner.registry
    }

    pub async fn set_url(&self, url: url::Url) {
        *self.inner.current_url.write().await = Some(url);
    }

    pub fn get_global_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.inner.registry.get_global_semaphore()
    }

    pub fn get_domain_semaphore(&self, url: &url::Url) -> Arc<tokio::sync::Semaphore> {
        self.inner.registry.get_domain_semaphore(url)
    }

    pub fn max_concurrent_global(&self) -> Option<usize> {
        self.inner.registry.max_concurrent_global()
    }

    pub fn max_concurrent_per_domain(&self) -> Option<usize> {
        self.inner.registry.max_concurrent_per_domain()
    }
}

impl reqwest_ratelimit::RateLimiter for ConcurrencyRateLimiter {
    fn acquire_permit(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            if let Some(ref url) = *self.inner.current_url.read().await {
                let global_semaphore = self.inner.registry.get_global_semaphore();
                let domain_semaphore = self.inner.registry.get_domain_semaphore(url);

                let _global_permit = global_semaphore.acquire().await.unwrap();
                let _domain_permit = domain_semaphore.acquire().await.unwrap();
            }
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ConcurrencyRateLimiter {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        extensions.insert(self.clone());
        next.run(req, extensions).await
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

        let _global = limiter.get_global_semaphore();
        let _domain = limiter.get_domain_semaphore(&url);
    }

    #[test]
    fn test_limiter_is_clone() {
        let limiter = ConcurrencyRateLimiter::builder().build();
        let _cloned = limiter.clone();
    }
}
