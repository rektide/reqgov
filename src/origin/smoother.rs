use governor::clock::{Clock, DefaultClock};
use governor::RateLimiter;
use governor::state::InMemoryState;
use governor::middleware::StateInformationMiddleware;
use governor::{Quota, NotUntil};
use governor::middleware::StateSnapshot;
use std::num::NonZeroU32;
use std::sync::Mutex;
use bon::Builder;

use crate::origin::policies::Policy;

#[derive(Debug, Clone, Copy, Builder)]
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
    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>,
    last_snapshot: Mutex<Option<StateSnapshot>>,

    pub base_window_secs: u32,
    pub micro_interval_secs: u32,
    pub velocity: f64,
}

impl Smoother {
    pub fn new(config: SmootherConfig) -> Self {
        Self {
            governor: RateLimiter::direct(Quota::per_second(NonZeroU32::MIN))
                .with_middleware::<StateInformationMiddleware>(),
            last_snapshot: Mutex::new(None),
            base_window_secs: 60,
            micro_interval_secs: config.micro_interval_secs,
            velocity: config.velocity,
        }
    }

    /// Get remaining from governor snapshot
    pub fn remaining(&self) -> u32 {
        self.last_snapshot
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.remaining_burst_capacity())
            .unwrap_or(0)
    }

    pub fn configure(&mut self, remaining: u32, window_secs: u32) {
        self.base_window_secs = window_secs;

        let intervals = window_secs / self.micro_interval_secs;

        let per_interval = (remaining as f64 / intervals as f64 * self.velocity).ceil() as u32;

        let per_second = per_interval as f64 / self.micro_interval_secs as f64;
        let rps = NonZeroU32::new(per_second.ceil() as u32).unwrap_or(NonZeroU32::MIN);

        self.governor = RateLimiter::direct(Quota::per_second(rps))
            .with_middleware::<StateInformationMiddleware>();
        *self.last_snapshot.lock().unwrap() = None; // Clear cached snapshot on reconfigure
    }

    pub fn configure_from_policy(&mut self, policy: &Policy) {
        self.configure(policy.quota, policy.window_secs.unwrap_or(60));
    }

    pub fn check(&self) -> Result<StateSnapshot, NotUntil<<DefaultClock as Clock>::Instant>> {
        let result = self.governor.check();
        // Cache snapshot for telemetry access
        if let Ok(ref snapshot) = result {
            *self.last_snapshot.lock().unwrap() = Some(snapshot.clone());
        }
        result
    }

    pub async fn wait(&self) {
        self.governor.until_ready().await;
    }

    pub fn clock(&self) -> &DefaultClock {
        self.governor.clock()
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
        let config = SmootherConfig::builder()
            .micro_interval_secs(5)
            .velocity(2.0)
            .build();
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
        let config = SmootherConfig::builder()
            .micro_interval_secs(2)
            .velocity(2.0)
            .build();
        let mut smoother = Smoother::new(config);

        smoother.configure(100, 60);
        assert_eq!(smoother.base_window_secs, 60);

        smoother.check().unwrap();
    }

    #[test]
    fn test_smoother_conservative_velocity() {
        let config = SmootherConfig::builder()
            .micro_interval_secs(1)
            .velocity(0.5)
            .build();
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
        let config = SmootherConfig::builder()
            .micro_interval_secs(10)
            .velocity(1.0)
            .build();
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
