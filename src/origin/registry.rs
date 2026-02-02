use crate::origin::origin_limiter::OriginLimiter;
use crate::origin::smoother_limiter::SmootherLimiter;
use crate::origin::parsing::{parse_limit_header, parse_policy_header};
use crate::origin::smoother::SmootherConfig;
use crate::origin::state::RateLimitViolation;
use http::{Extensions, HeaderMap};
use reqwest_middleware::{Middleware, Next, Result};
use dashmap::DashMap;
use std::sync::Arc;
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
        OriginRegistry {
            origin_limiters: Arc::new(DashMap::new()),
            smoother_limiters: Arc::new(DashMap::new()),
            smoother_config: self.smoother_config,
        }
    }
}

pub struct OriginRegistry {
    origin_limiters: Arc<DashMap<String, Arc<OriginLimiter>>>,
    smoother_limiters: Arc<DashMap<String, Arc<SmootherLimiter>>>,
    smoother_config: Option<SmootherConfig>,
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

    pub fn get_origin_limiter(&self, url: &Url) -> Arc<OriginLimiter> {
        let key = Self::origin_key(url);

        self.origin_limiters
            .entry(key)
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .clone()
    }

    pub fn get_smoother_limiter(&self, url: &Url) -> Arc<SmootherLimiter> {
        let key = Self::origin_key(url);
        let config = self.smoother_config.as_ref().unwrap();

        self.smoother_limiters
            .entry(key)
            .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
            .value()
            .clone()
    }

    pub async fn check(&self, url: &Url) -> std::result::Result<(), RateLimitViolation> {
        let origin_limiter = self.get_origin_limiter(url);
        origin_limiter.check().await?;

        if self.smoother_config.is_some() {
            let smoother_limiter = self.get_smoother_limiter(url);
            smoother_limiter.check().await?;
        }
        Ok(())
    }

    pub async fn wait(&self, url: &Url) {
        let origin_limiter = self.get_origin_limiter(url);
        origin_limiter.wait().await;

        if self.smoother_config.is_some() {
            let smoother_limiter = self.get_smoother_limiter(url);
            smoother_limiter.wait().await;
        }
    }

    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) {
        let origin_limiter = self.get_origin_limiter(url);

        if let Some(policies) = parse_policy_header(headers) {
            origin_limiter.update_policies(policies.clone());
            if self.smoother_config.is_some() {
                let smoother_limiter = self.get_smoother_limiter(url);
                smoother_limiter.update_policies(policies);
            }
        }

        if let Some(limits) = parse_limit_header(headers) {
            origin_limiter.update_limits(limits.clone()).await;
            if self.smoother_config.is_some() {
                let smoother_limiter = self.get_smoother_limiter(url);
                smoother_limiter.update_limits(limits).await;
            }
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
        self.check(&url).await.map_err(|e| reqwest_middleware::Error::middleware(e))?;
        extensions.insert(self.get_origin_limiter(&url));
        if self.smoother_config.is_some() {
            extensions.insert(self.get_smoother_limiter(&url));
        }
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
        let _origin_limiter = registry.get_origin_limiter(&url);
        let _smoother_limiter = registry.get_smoother_limiter(&url);
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
        let origin_limiter = registry.get_origin_limiter(&url);
        assert_eq!(origin_limiter.slots.len(), 1);
    }

    #[tokio::test]
    async fn test_check_passes() {
        let registry = OriginRegistry::builder()
            .smoother(SmootherConfig::default())
            .build();
        let url = Url::parse("https://api.example.com/test").unwrap();

        assert!(registry.check(&url).await.is_ok());
    }

    #[tokio::test]
    async fn test_registry_without_smoother() {
        let registry = OriginRegistry::builder().build();
        let url = Url::parse("https://api.example.com/test").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("ratelimit-policy", "burst:100:60:requests".parse().unwrap());
        headers.insert("ratelimit", "burst:45:30".parse().unwrap());

        registry.update_from_response(&url, &headers).await;
        assert!(registry.check(&url).await.is_ok());
        registry.wait(&url).await;

        let origin_limiter = registry.get_origin_limiter(&url);
        assert_eq!(origin_limiter.slots.len(), 1);
    }
}
