use bon::Builder;
use std::time::Duration;

#[derive(Debug, Clone, Builder)]
pub struct RateLimitCheckResult {
    pub allowed: bool,
    pub wait_duration: Option<Duration>,
    pub wait_count: u32,
}

impl RateLimitCheckResult {
    pub fn allowed() -> Self {
        Self::builder().allowed(true).wait_count(0).build()
    }

    pub fn blocked(wait_duration: Duration) -> Self {
        Self::builder()
            .allowed(false)
            .wait_duration(wait_duration)
            .wait_count(1)
            .build()
    }
}
