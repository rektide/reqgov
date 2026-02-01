use crate::limiter::context::SpanContext;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum RateLimitViolation {
    Smoothed {
        wait_duration: Duration,
    },
    PolicyExceeded {
        policy_name: String,
        wait_duration: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct OriginRateLimiterState {
    pub smoother: Option<crate::smoothing::smoother::SmootherState>,
    pub policies: Vec<crate::policies::slot::PolicySlotState>,
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,
    pub span_context: Option<SpanContext>,
    pub mode: crate::limiter::context::StateMode,
}

impl OriginRateLimiterState {
    pub fn limiting_policy(&self) -> Option<&str> {
        self.span_context
            .as_ref()
            .and_then(|r| r.limiting_policy.as_deref())
    }
}
