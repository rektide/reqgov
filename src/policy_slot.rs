use crate::policy::{Policy, ServiceLimit};
use governor::clock::{Clock, DefaultClock};
use governor::state::InMemoryState;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::time::Duration;

/// State snapshot for telemetry
#[derive(Debug, Clone)]
pub struct PolicySlotState {
    pub name: String,
    pub quota: u32,
    pub remaining: u32,
    pub window_secs: u32,
    pub reset_at: Option<std::time::Instant>,
}

pub struct PolicySlot {
    pub policy: Policy,
    pub remaining: u32,
    pub reset_at: Option<std::time::Instant>,

    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock>,

    governor_quota: (u32, u32),
}

impl PolicySlot {
    pub fn new(policy: Policy) -> Self {
        let quota = Self::compute_quota(&policy, policy.quota);
        Self {
            remaining: policy.quota,
            reset_at: None,
            governor: RateLimiter::direct(quota),
            governor_quota: (policy.quota, policy.window_secs.unwrap_or(60)),
            policy,
        }
    }

    fn compute_quota(policy: &Policy, remaining: u32) -> Quota {
        let window = policy.window_secs.unwrap_or(60);
        let per_second = NonZeroU32::new((remaining as f64 / window as f64).ceil() as u32)
            .unwrap_or(NonZeroU32::MIN);

        Quota::per_second(per_second)
    }

    pub fn update(&mut self, limit: &ServiceLimit) {
        self.remaining = limit.remaining;

        if let Some(reset_secs) = limit.reset_secs {
            self.reset_at = Some(std::time::Instant::now() + Duration::from_secs(reset_secs as u64));
        }

        self.maybe_rebuild_governor();
    }

    fn maybe_rebuild_governor(&mut self) {
        let window = self.policy.window_secs.unwrap_or(60);
        let (old_rem, old_window) = self.governor_quota;

        let should_rebuild = old_window != window || self.remaining < old_rem.saturating_sub(old_rem / 5);

        if should_rebuild {
            let quota = Self::compute_quota(&self.policy, self.remaining);
            self.governor = RateLimiter::direct(quota);
            self.governor_quota = (self.remaining, window);
        }
    }

    pub fn check(&self) -> Result<(), Duration> {
        let clock = DefaultClock::default();
        let now = clock.now();
        self.governor
            .check()
            .map_err(|not_until| not_until.wait_time_from(now))
    }

    pub async fn wait(&self) {
        self.governor.until_ready().await;
    }
    
    /// Get state snapshot for telemetry
    pub fn state(&self) -> PolicySlotState {
        PolicySlotState {
            name: self.policy.name.clone(),
            quota: self.policy.quota,
            remaining: self.remaining,
            window_secs: self.policy.window_secs.unwrap_or(60),
            reset_at: self.reset_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_slot_creation() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let slot = PolicySlot::new(policy);
        assert_eq!(slot.remaining, 100);
        assert_eq!(slot.policy.name, "burst");
    }

    #[test]
    fn test_policy_slot_check() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let slot = PolicySlot::new(policy);
        assert!(slot.check().is_ok());
    }

    #[test]
    fn test_policy_slot_update() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let mut slot = PolicySlot::new(policy);
        assert_eq!(slot.remaining, 100);

        let limit = ServiceLimit {
            name: "burst".to_string(),
            remaining: 45,
            reset_secs: Some(30),
            partition_key: None,
        };

        slot.update(&limit);
        assert_eq!(slot.remaining, 45);
        assert!(slot.reset_at.is_some());
    }

    #[test]
    fn test_policy_slot_update_without_reset() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let mut slot = PolicySlot::new(policy);

        let limit = ServiceLimit {
            name: "burst".to_string(),
            remaining: 80,
            reset_secs: None,
            partition_key: None,
        };

        slot.update(&limit);
        assert_eq!(slot.remaining, 80);
        assert!(slot.reset_at.is_none());
    }

    #[test]
    fn test_policy_slot_governor_rebuild_on_quota_drop() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let mut slot = PolicySlot::new(policy);
        let initial_governor_quota = slot.governor_quota;

        let limit = ServiceLimit {
            name: "burst".to_string(),
            remaining: 10,
            reset_secs: Some(30),
            partition_key: None,
        };

        slot.update(&limit);
        assert_eq!(slot.remaining, 10);
        assert_ne!(slot.governor_quota, initial_governor_quota);
    }

    #[test]
    fn test_policy_slot_governor_no_rebuild_small_change() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: crate::policy::QuotaUnit::Requests,
            partition_key: None,
        };

        let mut slot = PolicySlot::new(policy);
        let initial_governor_quota = slot.governor_quota;

        let limit = ServiceLimit {
            name: "burst".to_string(),
            remaining: 95,
            reset_secs: None,
            partition_key: None,
        };

        slot.update(&limit);
        assert_eq!(slot.remaining, 95);
        assert_eq!(slot.governor_quota, initial_governor_quota);
    }
}
