use crate::limiter::origin::OriginRateLimiter;
use crate::parsing::headers::{parse_limit_header, parse_policy_header};
use crate::smoothing::smoother::SmootherConfig;
use http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use url::Url;

pub struct OriginRegistry {
    limiters: Arc<RwLock<HashMap<String, Arc<RwLock<OriginRateLimiter>>>>>,
    smoother_config: SmootherConfig,
    global_semaphore: Arc<Semaphore>,
    per_domain_semaphores: Arc<RwLock<HashMap<String, Arc<Semaphore>>>>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl OriginRegistry {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            limiters: Arc::new(RwLock::new(HashMap::new())),
            smoother_config,
            global_semaphore: Arc::new(Semaphore::new(i32::MAX as usize)),
            per_domain_semaphores: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent_global: None,
            max_concurrent_per_domain: None,
        }
    }

    pub fn with_concurrency_limits(
        smoother_config: SmootherConfig,
        max_concurrent_global: Option<usize>,
        max_concurrent_per_domain: Option<usize>,
    ) -> Self {
        let global_permits = max_concurrent_global.unwrap_or(i32::MAX as usize);
        Self {
            limiters: Arc::new(RwLock::new(HashMap::new())),
            smoother_config,
            global_semaphore: Arc::new(Semaphore::new(global_permits)),
            per_domain_semaphores: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent_global,
            max_concurrent_per_domain,
        }
    }

    fn origin_key(url: &Url) -> String {
        format!(
            "{}://{}",
            url.scheme(),
            url.host_str().unwrap_or("unknown")
        )
    }

    pub async fn get_limiter(&self, url: &Url) -> Arc<RwLock<OriginRateLimiter>> {
        let key = Self::origin_key(url);

        {
            let limiters = self.limiters.read().await;
            if let Some(limiter) = limiters.get(&key) {
                return Arc::clone(limiter);
            }
        }

        let mut limiters = self.limiters.write().await;
        limiters
            .entry(key)
            .or_insert_with(|| {
                Arc::new(RwLock::new(OriginRateLimiter::with_smoother(self.smoother_config.clone())))
            })
            .clone()
    }

    pub async fn get_global_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.global_semaphore)
    }

    pub async fn get_domain_semaphore(&self, url: &Url) -> Arc<Semaphore> {
        let key = Self::origin_key(url);

        {
            let semaphores = self.per_domain_semaphores.read().await;
            if let Some(semaphore) = semaphores.get(&key) {
                return Arc::clone(semaphore);
            }
        }

        let mut semaphores = self.per_domain_semaphores.write().await;
        let permits = self.max_concurrent_per_domain.unwrap_or(i32::MAX as usize);
        semaphores
            .entry(key)
            .or_insert_with(|| Arc::new(Semaphore::new(permits)))
            .clone()
    }

    pub fn max_concurrent_global(&self) -> Option<usize> {
        self.max_concurrent_global
    }

    pub fn max_concurrent_per_domain(&self) -> Option<usize> {
        self.max_concurrent_per_domain
    }

    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) {
        let limiter = self.get_limiter(url).await;

        if let Some(policies) = parse_policy_header(headers) {
            limiter.write().await.update_policies(policies);
        }

        if let Some(limits) = parse_limit_header(headers) {
            limiter.write().await.update_limits(limits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_key() {
        let url = Url::parse("https://api.github.com/repos").unwrap();
        let key = OriginRegistry::origin_key(&url);
        assert_eq!(key, "https://api.github.com");
    }

    #[tokio::test]
    async fn test_registry_creation() {
        let config = SmootherConfig::default();
        let registry = OriginRegistry::new(config);
        let url = Url::parse("https://api.example.com/test").unwrap();
        let _limiter = registry.get_limiter(&url).await;
    }

    #[tokio::test]
    async fn test_registry_with_concurrency_limits() {
        let config = SmootherConfig::default();
        let registry = OriginRegistry::with_concurrency_limits(config, Some(100), Some(10));
        assert_eq!(registry.max_concurrent_global(), Some(100));
        assert_eq!(registry.max_concurrent_per_domain(), Some(10));
    }

    #[tokio::test]
    async fn test_get_global_semaphore() {
        let config = SmootherConfig::default();
        let registry = OriginRegistry::with_concurrency_limits(config, Some(5), None);
        let semaphore = registry.get_global_semaphore().await;

        let _permit1 = semaphore.acquire().await.unwrap();
        let _permit2 = semaphore.acquire().await.unwrap();
        let _permit3 = semaphore.acquire().await.unwrap();

        // Should be able to acquire 5 permits total
        let _permit4 = semaphore.acquire().await.unwrap();
        let _permit5 = semaphore.acquire().await.unwrap();
    }

    #[tokio::test]
    async fn test_get_domain_semaphore() {
        let config = SmootherConfig::default();
        let registry = OriginRegistry::with_concurrency_limits(config, None, Some(3));
        let url = Url::parse("https://api.example.com/test").unwrap();
        let semaphore = registry.get_domain_semaphore(&url).await;

        let _permit1 = semaphore.acquire().await.unwrap();
        let _permit2 = semaphore.acquire().await.unwrap();
        let _permit3 = semaphore.acquire().await.unwrap();

        // Should be able to acquire 3 permits for this domain
    }
}
