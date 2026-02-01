use crate::origin_limiter::OriginRateLimiter;
use crate::parser::{parse_limit_header, parse_policy_header};
use crate::smoother::SmootherConfig;
use http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

pub struct OriginRegistry {
    limiters: Arc<RwLock<HashMap<String, Arc<RwLock<OriginRateLimiter>>>>>,
    smoother_config: SmootherConfig,
}

impl OriginRegistry {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            limiters: Arc::new(RwLock::new(HashMap::new())),
            smoother_config,
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

    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) {
        let limiter = self.get_limiter(url).await;
        let mut limiter = limiter.write().await;

        if let Some(policies) = parse_policy_header(headers) {
            limiter.update_policies(policies);
        }

        if let Some(limits) = parse_limit_header(headers) {
            limiter.update_limits(limits);
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
}
