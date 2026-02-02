use std::fmt;
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

impl fmt::Display for RateLimitViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Smoothed { wait_duration } => {
                write!(f, "Rate limit smoothed, wait {:?}", wait_duration)
            }
            Self::PolicyExceeded {
                policy_name,
                wait_duration,
            } => {
                write!(
                    f,
                    "Policy '{}' exceeded, wait {:?}",
                    policy_name, wait_duration
                )
            }
        }
    }
}

impl std::error::Error for RateLimitViolation {}
