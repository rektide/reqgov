use crate::origin::state::RateLimitViolation;
use crate::origin::policies::{Policy, ServiceLimit};
use crate::origin::smoother::{Smoother, SmootherConfig};
use governor::clock::Clock;
use std::sync::Arc;

#[derive(Default)]
pub struct SmootherLimiterBuilder {
    config: Option<SmootherConfig>,
}

impl SmootherLimiterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(mut self, config: SmootherConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn build(self) -> SmootherLimiter {
        SmootherLimiter {
            smoother: Arc::new(tokio::sync::RwLock::new(self.config.map(Smoother::new))),
        }
    }
}

pub struct SmootherLimiter {
    pub(crate) smoother: Arc<tokio::sync::RwLock<Option<Smoother>>>,
}

impl Clone for SmootherLimiter {
    fn clone(&self) -> Self {
        Self {
            smoother: Arc::clone(&self.smoother),
        }
    }
}

impl SmootherLimiter {
    pub fn builder() -> SmootherLimiterBuilder {
        SmootherLimiterBuilder::new()
    }

    pub fn new() -> Self {
        Self::builder().build()
    }

    pub async fn check(&self) -> Result<(), RateLimitViolation> {
        if let Some(smoother) = self.smoother.read().await.as_ref() {
            if let Err(not_until) = smoother.check() {
                return Err(RateLimitViolation::Smoothed {
                    wait_duration: not_until.wait_time_from(smoother.clock().now()),
                });
            }
        }
        Ok(())
    }

    pub async fn wait(&self) {
        if let Some(smoother) = self.smoother.write().await.as_mut() {
            smoother.wait().await;
        }
    }

    pub async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        for limit in limits {
            let window = limit.reset_secs.unwrap_or(60);
            if let Some(smoother) = self.smoother.write().await.as_mut() {
                smoother.configure(limit.remaining, window);
            }
        }
    }

    pub async fn update_policies(&self, policies: Vec<Policy>) {
        if let Some(first_policy) = policies.first() {
            if let Some(smoother) = self.smoother.write().await.as_mut() {
                smoother.configure_from_policy(first_policy);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_smoother_limiter_new() {
        let limiter = SmootherLimiter::new();
        assert!(limiter.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_smoother_limiter_with_config() {
        let limiter = SmootherLimiter::builder()
            .config(SmootherConfig::default())
            .build();
        assert!(limiter.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_smoother_limiter_wait() {
        let limiter = SmootherLimiter::new();
        limiter.wait().await;
    }

    #[test]
    fn test_smoother_limiter_is_clone() {
        let limiter = SmootherLimiter::builder().build();
        let _cloned = limiter.clone();
    }
}
