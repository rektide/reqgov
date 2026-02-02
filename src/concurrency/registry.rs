use crate::url::origin_key;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use url::Url;

#[derive(Default)]
pub struct ConcurrencyRegistryBuilder {
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl ConcurrencyRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_concurrent_global(mut self, max: usize) -> Self {
        self.max_concurrent_global = Some(max);
        self
    }

    pub fn max_concurrent_per_domain(mut self, max: usize) -> Self {
        self.max_concurrent_per_domain = Some(max);
        self
    }

    pub fn build(self) -> ConcurrencyRegistry {
        let global_permits = self.max_concurrent_global.unwrap_or(i32::MAX as usize);
        ConcurrencyRegistry {
            global_semaphore: Arc::new(Semaphore::new(global_permits)),
            per_domain_semaphores: DashMap::new(),
            max_concurrent_global: self.max_concurrent_global,
            max_concurrent_per_domain: self.max_concurrent_per_domain,
        }
    }
}

pub struct ConcurrencyRegistry {
    global_semaphore: Arc<Semaphore>,
    per_domain_semaphores: DashMap<String, Arc<Semaphore>>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_domain: Option<usize>,
}

impl ConcurrencyRegistry {
    pub fn builder() -> ConcurrencyRegistryBuilder {
        ConcurrencyRegistryBuilder::new()
    }

    pub fn get_global_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.global_semaphore)
    }

    pub fn get_domain_semaphore(&self, url: &Url) -> Arc<Semaphore> {
        let key = origin_key(url);
        self._get_domain_semaphore(&key)
    }

    fn _get_domain_semaphore(&self, key: &str) -> Arc<Semaphore> {
        let permits = self.max_concurrent_per_domain.unwrap_or(i32::MAX as usize);

        self.per_domain_semaphores
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(permits)))
            .clone()
    }

    pub fn max_concurrent_global(&self) -> Option<usize> {
        self.max_concurrent_global
    }

    pub fn max_concurrent_per_domain(&self) -> Option<usize> {
        self.max_concurrent_per_domain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_key() {
        let url = Url::parse("https://api.github.com/repos").unwrap();
        let key = origin_key(&url);
        assert_eq!(key, "https://api.github.com");
    }

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = ConcurrencyRegistry::builder()
            .max_concurrent_global(100)
            .max_concurrent_per_domain(10)
            .build();
        assert_eq!(registry.max_concurrent_global(), Some(100));
        assert_eq!(registry.max_concurrent_per_domain(), Some(10));
    }

    #[tokio::test]
    async fn test_get_global_semaphore() {
        let registry = ConcurrencyRegistry::builder()
            .max_concurrent_global(5)
            .build();
        let semaphore = registry.get_global_semaphore();

        let _permit1 = semaphore.acquire().await.unwrap();
        let _permit2 = semaphore.acquire().await.unwrap();
        let _permit3 = semaphore.acquire().await.unwrap();

        let _permit4 = semaphore.acquire().await.unwrap();
        let _permit5 = semaphore.acquire().await.unwrap();
    }

    #[tokio::test]
    async fn test_get_domain_semaphore() {
        let registry = ConcurrencyRegistry::builder()
            .max_concurrent_per_domain(3)
            .build();
        let url = Url::parse("https://api.example.com/test").unwrap();
        let semaphore = registry.get_domain_semaphore(&url);

        let _permit1 = semaphore.acquire().await.unwrap();
        let _permit2 = semaphore.acquire().await.unwrap();
        let _permit3 = semaphore.acquire().await.unwrap();
    }
}
