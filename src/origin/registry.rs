use crate::origin::origin_limiter::OriginLimiter;
use crate::origin::smoother_limiter::SmootherLimiter;
use crate::origin::parsing::{parse_limit_header, parse_policy_header};
use crate::origin::smoother::SmootherConfig;
use crate::origin::state::RateLimitViolation;
use crate::origin::check_result::RateLimitCheckResult;
use crate::url::origin_key;
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

    pub fn get_origin_limiter(&self, url: &Url) -> Arc<OriginLimiter> {
        let key = origin_key(url);
        self.origin_limiters
            .entry(key)
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .clone()
    }

    pub fn get_smoother_limiter(&self, url: &Url) -> Arc<SmootherLimiter> {
        let key = origin_key(url);
        let config = self.smoother_config.as_ref().unwrap();
        self.smoother_limiters
            .entry(key)
            .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
            .value()
            .clone()
    }

    pub async fn check(&self, url: &Url) -> std::result::Result<(), RateLimitViolation> {
        let key = origin_key(url);
        self.origin_limiters.entry(key.clone())
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .check()
            .await?;

        if let Some(config) = &self.smoother_config {
            self.smoother_limiters.entry(key)
                .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
                .value()
                .check()
                .await?;
        }
        Ok(())
    }

    pub async fn wait(&self, url: &Url) -> RateLimitCheckResult {
        let start = std::time::Instant::now();
        let key = origin_key(url);

        self.origin_limiters.entry(key.clone())
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .wait()
            .await;

        if let Some(config) = &self.smoother_config {
            self.smoother_limiters.entry(key)
                .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
                .value()
                .wait()
            .await;
        }

        let wait_duration = start.elapsed();

        if wait_duration.as_millis() > 0 {
            RateLimitCheckResult::blocked(wait_duration)
        } else {
            RateLimitCheckResult::allowed()
        }
    }

    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) {
        let key = origin_key(url);

        let origin_limiter = self.origin_limiters.entry(key.clone())
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .clone();

        if let Some(policies) = parse_policy_header(headers) {
            origin_limiter.update_policies(policies.clone()).await;
            if let Some(config) = &self.smoother_config {
                self.smoother_limiters.entry(key.clone())
                    .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
                    .value()
                    .update_policies(policies);
            }
        }

        if let Some(limits) = parse_limit_header(headers) {
            origin_limiter.update_limits(limits.clone()).await;
            if let Some(config) = &self.smoother_config {
                self.smoother_limiters.entry(key)
                    .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
                    .value()
                    .update_limits(limits).await;
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
        let key = origin_key(&url);

        let check_result = self.wait(&url).await;
        extensions.insert(check_result);

        extensions.insert(self.origin_limiters.entry(key.clone())
            .or_insert_with(|| Arc::new(OriginLimiter::new()))
            .value()
            .clone());
        if let Some(config) = &self.smoother_config {
            extensions.insert(self.smoother_limiters.entry(key)
                .or_insert_with(|| Arc::new(SmootherLimiter::builder().config(config.clone()).build()))
                .value()
                .clone());
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
        let key = origin_key(&url);
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
        assert_eq!(origin_limiter.slots.lock().await.len(), 1);
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
        assert_eq!(origin_limiter.slots.lock().await.len(), 1);
    }
}
