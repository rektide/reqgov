use super::policies::{Policy, ServiceLimit};
use governor::clock::{Clock, DefaultClock};
use governor::middleware::{StateInformationMiddleware, StateSnapshot};
use governor::state::InMemoryState;
use governor::{NotUntil, Quota, RateLimiter};
use std::sync::Mutex;
use std::num::NonZeroU32;
use std::time::Duration;

pub struct PolicySlot {
    pub policy: Policy,
    pub remaining: u32,
    pub reset_at: Option<std::time::Instant>,

    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>,
    last_snapshot: Mutex<Option<StateSnapshot>>,

    governor_quota: (u32, u32),
}

impl PolicySlot {
    pub fn new(policy: Policy) -> Self {
        let quota = Self::compute_quota(&policy, policy.quota);
        Self {
            remaining: policy.quota,
            reset_at: None,
            governor: RateLimiter::direct(quota).with_middleware::<StateInformationMiddleware>(),
            last_snapshot: Mutex::new(None),
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
            self.governor = RateLimiter::direct(quota).with_middleware::<StateInformationMiddleware>();
            *self.last_snapshot.lock().unwrap() = None; // Clear cached snapshot on rebuild
            self.governor_quota = (self.remaining, window);
        }
    }

    pub fn check(&self) -> Result<StateSnapshot, NotUntil<<DefaultClock as Clock>::Instant>> {
        let result = self.governor.check();
        // Cache snapshot for telemetry access
        if let Ok(ref snapshot) = result {
            *self.last_snapshot.lock().unwrap() = Some(snapshot.clone());
        }
        result
    }

    pub async fn wait(&self) -> Duration {
        let start = std::time::Instant::now();
        self.governor.until_ready().await;
        start.elapsed()
    }

    pub fn clock(&self) -> &DefaultClock {
        self.governor.clock()
    }

    /// Get governor remaining from last snapshot
    pub fn governor_remaining(&self) -> u32 {
        self.last_snapshot
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.remaining_burst_capacity())
            .unwrap_or(self.remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::policies::QuotaUnit;

    #[test]
    fn test_policy_slot_creation() {
        let policy = Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: QuotaUnit::Requests,
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
            quota_unit: QuotaUnit::Requests,
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
            quota_unit: QuotaUnit::Requests,
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
            quota_unit: QuotaUnit::Requests,
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
