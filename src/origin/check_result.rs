use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct RateLimitCheckResult {
    pub allowed: bool,
    pub wait_duration: Option<Duration>,
}

impl RateLimitCheckResult {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            wait_duration: None,
        }
    }

    pub fn blocked(wait_duration: Duration) -> Self {
        Self {
            allowed: false,
            wait_duration: Some(wait_duration),
        }
    }
}
