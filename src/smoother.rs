use governor::clock::{Clock, DefaultClock};
use governor::state::InMemoryState;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;

#[derive(Clone)]
pub struct SmootherConfig {
    pub micro_interval_secs: u32,
    pub velocity: f64,
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
    config: SmootherConfig,
}

impl Smoother {
    pub fn new(config: SmootherConfig) -> Self {
        Self {
            governor: RateLimiter::direct(Quota::per_second(NonZeroU32::MIN)),
            base_window_secs: 60,
            micro_interval_secs: config.micro_interval_secs,
            velocity: config.velocity,
            config,
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
    fn test_smoother_configure() {
        let config = SmootherConfig::default();
        let mut smoother = Smoother::new(config);
        smoother.configure(100, 60);
        assert_eq!(smoother.base_window_secs, 60);
    }

    #[test]
    fn test_smoother_check() {
        let config = SmootherConfig::default();
        let smoother = Smoother::new(config);
        assert!(smoother.check().is_ok());
    }
}
