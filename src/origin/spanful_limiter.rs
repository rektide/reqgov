use super::{OriginLimiter, Policy, ServiceLimit, RateLimitViolation, PolicySlot};
use std::sync::Arc;

pub struct OriginLimiterTracer {
    inner: Arc<OriginLimiter>,
}

impl OriginLimiterTracer {
    pub fn new(limiter: Arc<OriginLimiter>) -> Self {
        Self { inner: limiter }
    }

    pub async fn update_policies(&self, policies: Vec<Policy>) {
        self.inner.update_policies(policies).await;
    }

    pub async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        self.inner.update_limits(limits).await;
    }

    pub async fn check(&self) -> Result<(), RateLimitViolation> {
        self.inner.check().await
    }

    pub async fn wait(&self) -> std::time::Duration {
        let mut total_wait = std::time::Duration::ZERO;

        let slots = self.inner.slots.lock().await;
        for slot in slots.iter() {
            let _span = tracing::info_span!(
                "rate_limit.wait.policy",
                policy_name = %slot.policy.name
            ).entered();
            total_wait += slot.wait().await;
        }

        total_wait
    }

    pub async fn for_each_slot<F, Fut>(&self, f: F)
    where
        F: Fn(&PolicySlot) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        self.inner.for_each_slot(f).await;
    }
}

impl Clone for OriginLimiterTracer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
