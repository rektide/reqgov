use governor::clock::{Clock, DefaultClock};
use governor::state::InMemoryState;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;

#[derive(Debug, Clone, Copy)]
pub struct SmootherConfig {
    pub micro_interval_secs: u32,
    pub velocity: f64,
}

/// State snapshot for telemetry
#[derive(Debug, Clone, Copy)]
pub struct SmootherState {
    pub remaining_per_interval: f64,
    pub micro_interval_secs: u32,
    pub velocity: f64,
    pub base_window_secs: u32,
}

impl Default for SmootherConfig {
    fn default() -> Self {
        Self {
            micro_interval_secs: 2,
            velocity: 1.5,
        }
    }
}

pub struct Smoother {
    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock>,

    base_window_secs: u32,
    micro_interval_secs: u32,
    velocity: f64,
}

impl Smoother {
    pub fn new(config: SmootherConfig) -> Self {
        Self {
            governor: RateLimiter::direct(Quota::per_second(NonZeroU32::MIN)),
            base_window_secs: 60,
            micro_interval_secs: config.micro_interval_secs,
            velocity: config.velocity,
        }
    }
    
    /// Get state snapshot for telemetry
    pub fn state(&self) -> SmootherState {
        // Estimate remaining in current interval based on last governor check
        // This is approximate since governor doesn't expose internal state directly
        let intervals = self.base_window_secs / self.micro_interval_secs;
        let per_interval = 1.0 / intervals as f64;
        let remaining_per_interval = per_interval * self.velocity;
        
        SmootherState {
            remaining_per_interval,
            micro_interval_secs: self.micro_interval_secs,
            velocity: self.velocity,
            base_window_secs: self.base_window_secs,
        }
    }

    pub fn configure(&mut self, remaining: u32, window_secs: u32) {
        self.base_window_secs = window_secs;

        let intervals = window_secs / self.micro_interval_secs;

        let per_interval = (remaining as f64 / intervals as f64 * self.velocity).ceil() as u32;

        let per_second = per_interval as f64 / self.micro_interval_secs as f64;
        let rps = NonZeroU32::new(per_second.ceil() as u32).unwrap_or(NonZeroU32::MIN);

        self.governor = RateLimiter::direct(Quota::per_second(rps));
    }

    pub fn check(&self) -> Result<(), std::time::Duration> {
        let clock = DefaultClock::default();
        let now = clock.now();
        self.governor
            .check()
            .map_err(|not_until| not_until.wait_time_from(now))
    }

    pub async fn wait(&self) {
        self.governor.until_ready().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smoother_creation() {
        let config = SmootherConfig::default();
        let smoother = Smoother::new(config);
        assert_eq!(smoother.base_window_secs, 60);
        assert_eq!(smoother.micro_interval_secs, 2);
        assert_eq!(smoother.velocity, 1.5);
    }

    #[test]
    fn test_smoother_custom_config() {
        let config = SmootherConfig {
            micro_interval_secs: 5,
            velocity: 2.0,
        };
        let smoother = Smoother::new(config);
        assert_eq!(smoother.micro_interval_secs, 5);
        assert_eq!(smoother.velocity, 2.0);
    }

    #[test]
    fn test_smoother_configure() {
        let config = SmootherConfig::default();
        let mut smoother = Smoother::new(config);
        smoother.configure(100, 60);
        assert_eq!(smoother.base_window_secs, 60);
    }

    #[test]
    fn test_smoother_velocity_multiplier() {
        let config = SmootherConfig {
            micro_interval_secs: 2,
            velocity: 2.0,
        };
        let mut smoother = Smoother::new(config);

        smoother.configure(100, 60);
        assert_eq!(smoother.base_window_secs, 60);

        smoother.check().unwrap();
    }

    #[test]
    fn test_smoother_conservative_velocity() {
        let config = SmootherConfig {
            micro_interval_secs: 1,
            velocity: 0.5,
        };
        let mut smoother = Smoother::new(config);
        smoother.configure(50, 60);

        assert!(smoother.check().is_ok());
    }

    #[test]
    fn test_smoother_check() {
        let config = SmootherConfig::default();
        let smoother = Smoother::new(config);
        assert!(smoother.check().is_ok());
    }

    #[test]
    fn test_smoother_custom_interval() {
        let config = SmootherConfig {
            micro_interval_secs: 10,
            velocity: 1.0,
        };
        let mut smoother = Smoother::new(config);
        smoother.configure(1000, 3600);
        assert_eq!(smoother.base_window_secs, 3600);
        assert_eq!(smoother.micro_interval_secs, 10);
    }

    #[test]
    fn test_smoother_large_window() {
        let config = SmootherConfig::default();
        let mut smoother = Smoother::new(config);
        smoother.configure(10000, 86400);
        assert_eq!(smoother.base_window_secs, 86400);
        assert!(smoother.check().is_ok());
    }

    #[test]
    fn test_smoother_zero_remaining_defaults_to_min_rate() {
        let config = SmootherConfig::default();
        let mut smoother = Smoother::new(config);
        smoother.configure(0, 60);

        let result = smoother.check();
        assert!(result.is_ok(), "Zero remaining defaults to minimum rate, doesn't block");
    }
}
