use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct RateLimitCheckResult {
    pub allowed: bool,
    pub blocked_by: Option<RateLimitBlockedBy>,
    pub wait_duration: Option<Duration>,
}

impl RateLimitCheckResult {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            blocked_by: None,
            wait_duration: None,
        }
    }

    pub fn blocked_by_smoother(wait_duration: Duration) -> Self {
        Self {
            allowed: false,
            blocked_by: Some(RateLimitBlockedBy::Smoother),
            wait_duration: Some(wait_duration),
        }
    }

    pub fn blocked_by_policy(policy_name: String, wait_duration: Duration) -> Self {
        Self {
            allowed: false,
            blocked_by: Some(RateLimitBlockedBy::Policy(policy_name)),
            wait_duration: Some(wait_duration),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RateLimitBlockedBy {
    Smoother,
    Policy(String),
}
