use crate::origin::origin::OriginRateLimiter;
use crate::origin::parsing::{parse_limit_header, parse_policy_header};
use crate::origin::smoother::SmootherConfig;
use http::{Extensions, HeaderMap};
use reqwest_middleware::{Middleware, Next, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

#[derive(Default)]
pub struct OriginRegistryBuilder {
    smoother_config: Option<SmootherConfig>,
}

impl OriginRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn smoother(mut self, config: SmootherConfig) -> Self {
        self.smoother_config = Some(config);
        self
    }

    pub fn build(self) -> OriginRegistry {
        let smoother_config = self.smoother_config.unwrap_or_default();
        OriginRegistry {
            limiters: Arc::new(RwLock::new(HashMap::new())),
            smoother_config,
        }
    }
}

pub struct OriginRegistry {
    limiters: Arc<RwLock<HashMap<String, Arc<OriginRateLimiter>>>>,
    smoother_config: SmootherConfig,
}

impl OriginRegistry {
    pub fn builder() -> OriginRegistryBuilder {
        OriginRegistryBuilder::new()
    }

    fn origin_key(url: &Url) -> String {
        format!(
            "{}://{}",
            url.scheme(),
            url.host_str().unwrap_or("unknown")
        )
    }

    pub async fn get_limiter(&self, url: &Url) -> Arc<OriginRateLimiter> {
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
                Arc::new(
                    OriginRateLimiter::builder()
                        .smoother(self.smoother_config.clone())
                        .build()
                )
            })
            .clone()
    }

    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) {
        let limiter = self.get_limiter(url).await;

        if let Some(policies) = parse_policy_header(headers) {
            limiter.update_policies(policies);
        }

        if let Some(limits) = parse_limit_header(headers) {
            limiter.update_limits(limits).await;
        }
    }
}

#[async_trait::async_trait]
impl Middleware for OriginRegistry {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        let url = req.url().clone();
        let limiter = self.get_limiter(&url).await;
        extensions.insert(limiter);
        next.run(req, extensions).await
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
        let registry = OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build();
        let url = Url::parse("https://api.example.com/test").unwrap();
        let _limiter = registry.get_limiter(&url).await;
    }

    #[tokio::test]
    async fn test_update_from_response() {
        let registry = OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build();
        let url = Url::parse("https://api.example.com/test").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("ratelimit-policy", "burst:100:60:requests".parse().unwrap());
        headers.insert("ratelimit", "burst:45:30".parse().unwrap());

        registry.update_from_response(&url, &headers).await;
        let limiter = registry.get_limiter(&url).await;
        assert_eq!(limiter.slots.len(), 1);
    }
}
