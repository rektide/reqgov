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
