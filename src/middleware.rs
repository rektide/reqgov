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

    pub fn with_registry(registry: Arc<OriginRegistry>) -> Self {
        Self {
            registry,
            current_url: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub async fn set_url(&self, url: url::Url) {
        *self.current_url.write().await = Some(url);
    }
}

impl reqwest_ratelimit::RateLimiter for HttpApiRateLimiter {
    fn acquire_permit(&self) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            if let Some(ref url) = *self.current_url.read().await {
                let limiter = self.registry.get_limiter(url).await;
                limiter.read().await.wait().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_creation() {
        let config = SmootherConfig::default();
        let _limiter = HttpApiRateLimiter::new(config);
    }
}
