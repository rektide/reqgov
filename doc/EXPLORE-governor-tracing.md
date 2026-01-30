# Governor State Investigation for Telemetry

## Summary

Governor **DOES** provide access to state through `StateInformationMiddleware`, eliminating the need for approximations in our current tracing implementation.

## StateInformationMiddleware Discovery

### How It Works

Governor's middleware system allows customizing what data is returned from rate limiting decisions:

```rust
use governor::{RateLimiter, Quota, middleware::StateInformationMiddleware, clock::Clock};

let limiter = RateLimiter::direct(Quota::per_second(nonzero!(10u32)))
    .with_middleware::<StateInformationMiddleware>();

match limiter.check() {
    Ok(snapshot) => {
        // Real remaining burst capacity from governor's internal GCRA state
        let remaining = snapshot.remaining_burst_capacity();
        let quota = snapshot.quota();
    }
    Err(not_until) => {
        // Wait duration when rate limited
        let wait = not_until.wait_time_from(limiter.clock().now());
        let quota = not_until.quota();
    }
}
```

### Available State Information

#### `StateSnapshot` (from positive outcomes)
- `quota()` - Returns `Quota` with:
  - `max_burst: u32` - Maximum burst capacity
  - `replenish_1_per: Duration` - Time between refills
- `remaining_burst_capacity()` - **Actual remaining burst capacity** from GCRA algorithm
  - Returns number of cells that can be let through immediately
  - Based on governor's internal state, not approximations
  - Mathematically correct per GCRA specification

#### `NotUntil` (from negative outcomes)
- `earliest_possible()` - Earliest time a cell might be allowed
- `wait_time_from(from)` - Wait duration from a specific time
  - Requires using `limiter.clock().now()` (clock type varies with features)
- `quota()` - Quota used for this rate limiting decision

### Test Results

Running `test_state_info` confirmed:

1. **Burst capacity decreases correctly**:
   - Initial: 99 (100 per second quota)
   - After 50 checks: 48 (51 permits consumed)
   - Each check consumes 1 permit from burst capacity

2. **Burst capacity varies with quota**:
   - Small quota (1/sec): remaining = 0 (burst = 1)
   - Medium quota (50/sec): remaining = 49 (burst = 50)
   - Large quota (1000/sec): remaining = 999 (burst = 1000)

3. **Capacity refills over time**:
   - Consumed 10 requests (burst = 10), became rate limited
   - After 500ms delay: 4 permits available (10/sec * 0.5s = 5, but some overhead)

4. **NoOpMiddleware vs StateInformationMiddleware**:
   - NoOp: Returns `Ok(())` - no state info
   - StateInfo: Returns `Ok(StateSnapshot)` with full state

## Current reqgov Implementation Issues

### Problem: NoOpMiddleware Used

Current implementation in `Smoother` and `PolicySlot`:
```rust
use governor::middleware::NoOpMiddleware;

self.governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>
```

Result: `check()` returns `Result<(), Duration>` - no state information available.

### Consequence: Approximations Required

Current `state()` methods use approximations:

**Smoother state approximation** (`src/smoother.rs:49`):
```rust
pub fn state(&self) -> SmootherState {
    let intervals = self.base_window_secs / self.micro_interval_secs;
    let per_interval = 1.0 / intervals as f64;
    let remaining_per_interval = per_interval * self.velocity;  // Theoretical, not actual

    SmootherState {
        remaining_per_interval,  // Approximate!
        micro_interval_secs: self.micro_interval_secs,
        velocity: self.velocity,
        base_window_secs: self.base_window_secs,
    }
}
```

**PolicySlot state derived from headers** (`src/policy_slot.rs:84`):
```rust
pub fn state(&self) -> PolicySlotState {
    PolicySlotState {
        name: self.policy.name.clone(),
        quota: self.policy.quota,
        remaining: self.remaining,  // From HTTP headers, not governor state
        window_secs: self.policy.window_secs.unwrap_or(60),
        reset_at: self.reset_at,
    }
}
```

**OriginRateLimiter double-check for throttle detection** (`src/origin_limiter.rs:109`):
```rust
let will_throttle = self.check().is_err();  // Consumes a permit!
let throttle_wait_duration = self.check().err().map(...);  // Another check!
```

This is inefficient and may give incorrect results (second check sees different state).

## Solution: Use StateInformationMiddleware

### Implementation Changes Required

1. **Update `Smoother`**:
```rust
use governor::middleware::StateInformationMiddleware;

pub struct Smoother {
    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>,
    last_snapshot: RefCell<Option<StateSnapshot>>,  // Cache for telemetry
    base_window_secs: u32,
    micro_interval_secs: u32,
    velocity: f64,
}

impl Smoother {
    pub fn check(&self) -> Result<StateSnapshot, NotUntil> {
        let result = self.governor.check();
        if let Ok(ref snapshot) = result {
            self.last_snapshot.replace(Some(snapshot.clone()));
        }
        result
    }

    pub fn state(&self) -> SmootherState {
        let snapshot = self.last_snapshot.borrow();
        // Use actual remaining_burst_capacity() from governor
        let remaining = snapshot.as_ref()
            .map(|s| s.remaining_burst_capacity() as f64)
            .unwrap_or(0.0);

        SmootherState {
            remaining_per_interval: remaining,
            micro_interval_secs: self.micro_interval_secs,
            velocity: self.velocity,
            base_window_secs: self.base_window_secs,
        }
    }
}
```

2. **Update `PolicySlot`**:
```rust
pub struct PolicySlot {
    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>,
    last_snapshot: RefCell<Option<StateSnapshot>>,
    pub policy: Policy,
    pub remaining: u32,  // Still keep for HTTP header updates
    reset_at: Option<std::time::Instant>,
}

impl PolicySlot {
    pub fn check(&self) -> Result<StateSnapshot, NotUntil> {
        let result = self.governor.check();
        if let Ok(ref snapshot) = result {
            self.last_snapshot.replace(Some(snapshot.clone()));
        }
        result
    }

    pub fn state(&self) -> PolicySlotState {
        let snapshot = self.last_snapshot.borrow();
        let governor_remaining = snapshot.as_ref()
            .map(|s| s.remaining_burst_capacity())
            .unwrap_or(0);

        PolicySlotState {
            name: self.policy.name.clone(),
            quota: self.policy.quota,
            remaining: governor_remaining,  // Use actual governor state!
            window_secs: self.policy.window_secs.unwrap_or(60),
            reset_at: self.reset_at,
        }
    }
}
```

3. **Update `OriginRateLimiter`**:
```rust
impl OriginRateLimiter {
    pub fn state(&self) -> OriginRateLimiterState {
        let smoother_state = self.smoother.state();

        let policy_states: Vec<PolicySlotState> = self.slots
            .values()
            .map(|slot| slot.state())
            .collect();

        // Check throttle status without double-checking
        // Use cached snapshots from last check() calls
        let will_throttle = self.smoother.check().is_err()
            || self.slots.values().any(|slot| slot.check().is_err());

        // For wait duration, we need to actually check
        let throttle_wait_duration = self.check().err().map(|violation| match violation {
            RateLimitViolation::Smoothed { wait_duration } => wait_duration,
            RateLimitViolation::PolicyExceeded { wait_duration, .. } => wait_duration,
        });

        OriginRateLimiterState {
            smoother: Some(smoother_state),
            policies: policy_states,
            will_throttle,
            throttle_wait_duration,
        }
    }
}
```

### API Changes Required

These are **breaking changes** to reqgov's public API:

| Location | Current | New |
|----------|---------|-----|
| `Smoother::check()` | `Result<(), Duration>` | `Result<StateSnapshot, NotUntil>` |
| `PolicySlot::check()` | `Result<(), Duration>` | `Result<StateSnapshot, NotUntil>` |
| `OriginRateLimiter::check()` | `Result<(), RateLimitViolation>` | Unchanged (wraps inner calls) |
| `RateLimitTelemetry` | Uses approximations | Uses actual governor state |

### Internal Changes Required

- Add `use governor::middleware::StateInformationMiddleware` to affected files
- Change governor type signatures
- Add `last_snapshot: RefCell<Option<StateSnapshot>>` to `Smoother` and `PolicySlot`
- Update all call sites that depend on `check()` returning `()`
- Update `state()` methods to use cached snapshots

### Benefits

1. **Accurate state**: `remaining_burst_capacity()` is from actual GCRA state, not velocity calculations
2. **No approximations**: Telemetry shows real permit counts
3. **Better throttle detection**: Can use actual governor state instead of double-checking
4. **Full quota info**: `StateSnapshot.quota()` provides burst size and replenish rate

### Tradeoffs

**Pros:**
- Mathematically correct state from GCRA
- Small overhead (snapshot clone vs empty tuple)
- Mature, well-tested governor implementation
- Consistent with IETF rate limit semantics

**Cons:**
- Breaking API changes
- Requires updating all `check()` callers
- Slightly more memory allocation (snapshot struct)
- Need to cache snapshots for telemetry access

**Performance:**
- Snapshot clone is cheap (copy of 4 u64/nanos fields)
- No synchronization overhead (RefCell is single-threaded safe)
- Minimal measurable impact from test runs

## Implementation Plan

1. Add `StateInformationMiddleware` dependency and imports
2. Modify `Smoother`:
   - Change governor type signature
   - Add `last_snapshot` cache
   - Update `check()` return type
   - Update `state()` to use snapshot
3. Modify `PolicySlot`:
   - Same changes as Smoother
   - Use `remaining_burst_capacity()` for telemetry
4. Modify `OriginRateLimiter`:
   - Update state() to avoid double-checking
   - Handle new check() return types
5. Update `RateLimitTelemetry`:
   - Use actual governor state instead of approximations
6. Update all call sites:
   - `HttpApiRateLimiter` implementation
   - Tests that call `check()` directly
7. Update documentation:
   - Note API changes in CHANGELOG
   - Update README examples
   - Update Technical Notes to remove "approximate state" constraints

## Migration Notes

For downstream users:

**Current usage:**
```rust
match limiter.check() {
    Ok(()) => { /* proceed */ }
    Err(duration) => { /* wait duration */ }
}
```

**New usage:**
```rust
use governor::{StateSnapshot, NotUntil};

match limiter.check() {
    Ok(snapshot) => {
        // Proceed, snapshot has state info
        let remaining = snapshot.remaining_burst_capacity();
    }
    Err(not_until) => {
        // Wait duration
        let wait = not_until.wait_time_from(clock.now());
    }
}
```

This provides **more information** in both success and failure cases.

## What's Still Missing Even with StateInformationMiddleware

1. **Historical consumption**: Can't see how many permits were consumed over time
2. **Queue depth**: Can't see how many requests are waiting on the limiter
3. **Permit arrival time**: Can't see when next permit will arrive directly (need to calculate from wait_time)
4. **Multi-policy bottleneck identification**: When checking multiple policies, can't easily tell which has the tightest constraint without checking each individually

These are fundamental limitations of GCRA's design, not governor's implementation.
