# Optional Smoother Plan

## Overview

Allow `OriginRateLimiter` to operate without a smoother component, making the smoother optional. Currently, the smoother is mandatory and deeply integrated into the rate limiting logic. This change will enable users who only need per-policy rate limiting to disable the smoother functionality entirely.

## Problem Statement

**Current implementation:**
- `OriginRateLimiter::new()` requires a `SmootherConfig` parameter
- Smoother is always instantiated and tightly coupled to the rate limiter
- Smoother checks are mandatory in the `check()` method
- Smoother state is always included in telemetry

**Missing flexibility:**
- Users cannot disable smoothing if they only want per-policy rate limiting
- Smoother adds overhead even when not needed for some use cases
- Configuration requires providing a `SmootherConfig` even if smoothing is undesired
- Telemetry includes smoother data even when not relevant

**Example of the problem:**
```rust
// User wants only per-policy rate limiting, but must provide smoother config
let limiter = OriginRateLimiter::new(SmootherConfig::default());  // Unnecessary!

// Check always runs smoother logic, even if user doesn't want it
limiter.check();  // Always calls smoother.check()

// State always includes smoother, even when not configured/used
let state = limiter.state();
assert!(state.smoother.is_some());  // Always Some, never None
```

## Proposed Architecture

### Optional Smoother Field

```rust
pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Option<Smoother>,  // Changed from Smoother
    fastest_policy: Option<String>,
    last_check_result: Arc<RwLock<Option<LastCheckResult>>>,
}
```

### Flexible Constructor

```rust
impl OriginRateLimiter {
    /// Create a new rate limiter without a smoother
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            smoother: None,  // No smoother by default
            fastest_policy: None,
            last_check_result: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a new rate limiter with a smoother
    pub fn with_smoother(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Some(Smoother::new(smoother_config)),
            fastest_policy: None,
            last_check_result: Arc::new(RwLock::new(None)),
        }
    }
}
```

### Updated check() Method

```rust
impl OriginRateLimiter {
    pub fn check(&self) -> Result<(), RateLimitViolation> {
        let start = Instant::now();

        // Only check smoother if configured
        let (smoother_result, smoother_state, smoother_metrics) = if let Some(ref smoother) = self.smoother {
            let smoother_start = Instant::now();
            let result = smoother.check();
            let state = smoother.state();
            let duration = smoother_start.elapsed();
            let metrics = CheckMetrics {
                duration,
                passed: result.is_ok(),
                wait_duration: result.as_ref().err().map(|not_until| {
                    not_until.wait_time_from(smoother.clock().now())
                }),
            };
            (Some(result), Some(state), Some(metrics))
        } else {
            (None, None, None)  // Smoother disabled
        };

        // Check policies (always run)
        let mut policy_results = Vec::new();
        let mut policy_states = Vec::new();

        for (name, slot) in &self.slots {
            let policy_start = Instant::now();
            let policy_result = slot.check();
            let policy_duration = policy_start.elapsed();
            let policy_state = slot.state();

            policy_states.push((name.clone(), policy_state));
            policy_results.push((
                name.clone(),
                CheckMetrics {
                    duration: policy_duration,
                    passed: policy_result.is_ok(),
                    wait_duration: policy_result.as_ref().err().map(|not_until| {
                        not_until.wait_time_from(slot.clock().now())
                    }),
                },
            ));
        }

        let total_duration = start.elapsed();

        // Determine if all checks passed (consider optional smoother)
        let smoother_passed = smoother_result.as_ref().map(|r| r.is_ok()).unwrap_or(true);
        let all_passed = smoother_passed && policy_results.iter().all(|(_, r)| r.passed);

        let limiting_policy = if !all_passed {
            if !smoother_passed {
                None  // Smoother blocked, not a specific policy
            } else {
                policy_results.iter().find(|(_, r)| !r.passed).map(|(name, _)| name.clone())
            }
        } else {
            None
        };

        let failed_policy = if all_passed || !smoother_passed {
            None
        } else {
            policy_results.iter().find(|(_, r)| !r.passed).map(|(name, result)| {
                (name.clone(), result.wait_duration.unwrap())
            })
        };

        let last_result = LastCheckResult {
            timestamp: Instant::now(),
            smoother_state,
            policy_states,
            smoother_metrics,
            policy_metrics: policy_results,
            total_duration,
            all_passed,
            limiting_policy,
        };

        *self.last_check_result.write().unwrap() = Some(last_result);

        // Return errors (smoother or policy)
        if let Some(Err(not_until)) = smoother_result {
            return Err(RateLimitViolation::Smoothed {
                wait_duration: not_until.wait_time_from(self.smoother.as_ref().unwrap().clock().now()),
            });
        }

        if let Some((name, wait_duration)) = failed_policy {
            return Err(RateLimitViolation::PolicyExceeded {
                policy_name: name,
                wait_duration,
            });
        }

        Ok(())
    }
}
```

### Updated wait() Method

```rust
impl OriginRateLimiter {
    pub async fn wait(&self) {
        if let Some(ref smoother) = self.smoother {
            smoother.wait().await;
        }
        // No-op if smoother is None
    }
}
```

### Updated reconfigure_smoother()

```rust
impl OriginRateLimiter {
    fn reconfigure_smoother(&mut self) {
        if let Some(ref smoother) = self.smoother {
            if let Some(ref name) = self.fastest_policy {
                if let Some(slot) = self.slots.get(name) {
                    let window = slot.policy.window_secs.unwrap_or(60);
                    smoother.configure(slot.remaining, window);
                }
            }
        }
        // No-op if smoother is None
    }
}
```

### Updated state() Method

```rust
impl OriginRateLimiter {
    pub fn state(&self) -> OriginRateLimiterState {
        let smoother_state = self.smoother.as_ref().map(|s| s.state());

        let policy_states: Vec<_> = self.slots
            .values()
            .map(|slot| slot.state())
            .collect();

        let last_check = self.last_check_result.read().unwrap().clone();
        let (will_throttle, limiting_policy, throttle_wait_duration) = match last_check {
            Some(ref result) => (
                !result.all_passed,
                result.limiting_policy.clone(),
                result.smoother_metrics.as_ref().and_then(|m| m.wait_duration)
                    .or_else(|| {
                        result.policy_metrics.iter()
                            .find(|(_, m)| !m.passed)
                            .and_then(|(_, m)| m.wait_duration)
                    }),
            ),
            None => {
                // Fallback: check without saving
                let check_result = self.check();
                (
                    check_result.is_err(),
                    check_result.err().and_then(|v| match v {
                        RateLimitViolation::Smoothed { .. } => None,
                        RateLimitViolation::PolicyExceeded { policy_name, .. } => Some(policy_name),
                    }),
                    None,
                )
            }
        };

        OriginRateLimiterState {
            smoother: smoother_state,
            policies: policy_states,
            will_throttle,
            throttle_wait_duration,
            last_check,
            mode: StateMode::Historical,
        }
    }
}
```

## Use Cases

### 1. Per-Policy Rate Limiting Only

**Goal:** Use only per-policy rate limiting, disable smoothing.

```rust
// Create limiter without smoother
let mut limiter = OriginRateLimiter::new();

// Add policies only
limiter.update_policies(vec![
    Policy {
        name: "burst".to_string(),
        quota: 100,
        window_secs: Some(60),
        // ... other fields
    },
]);

// Check only runs policies
limiter.check();  // No smoother overhead

// State reflects no smoother
let state = limiter.state();
assert!(state.smoother.is_none());  // Correct!
```

### 2. Smoothing + Per-Policy Rate Limiting

**Goal:** Use both smoothing and per-policy rate limiting (current behavior).

```rust
// Create limiter with smoother
let mut limiter = OriginRateLimiter::with_smoother(SmootherConfig::default());

// Add policies
limiter.update_policies(vec![/* ... */]);

// Check runs smoother + policies
limiter.check();  // Full rate limiting

// State includes smoother
let state = limiter.state();
assert!(state.smoother.is_some());  // Correct!
```

### 3. Migration Path

**Goal:** Gradually migrate existing code to optional smoother.

```rust
// Before (current API, requires breaking change)
let limiter = OriginRateLimiter::new(SmootherConfig::default());

// After (new API, non-breaking if both constructors coexist)
let limiter = OriginRateLimiter::with_smoother(SmootherConfig::default());
// OR
let limiter = OriginRateLimiter::new();  // No smoother
```

## Implementation Steps

### Phase 1: Update Struct Definition

1. Change `smoother: Smoother` to `smoother: Option<Smoother>` in `OriginRateLimiter`
2. Update all references to `self.smoother` to handle `None`
3. No functional changes yet

### Phase 2: Add Flexible Constructors

1. Add `OriginRateLimiter::new()` (no smoother by default)
2. Add `OriginRateLimiter::with_smoother(config)` (smoother enabled)
3. Deprecate old `OriginRateLimiter::new(config)` (optional, for migration)
4. Update docstrings to document both constructors

### Phase 3: Update check() Method

1. Wrap smoother operations in `if let Some(ref smoother) = self.smoother`
2. Set smoother results to `None` when smoother is disabled
3. Update `all_passed` logic to treat missing smoother as "passed"
4. Update `limiting_policy` logic to handle `None` smoother case
5. Ensure `LastCheckResult` properly reflects optional smoother

### Phase 4: Update Other Methods

1. Update `wait()` to be no-op when smoother is `None`
2. Update `reconfigure_smoother()` to be no-op when smoother is `None`
3. Update `state()` to return `None` for `smoother` field when disabled
4. Update `update_limits()` and `update_policies()` if they reference smoother

### Phase 5: Update Telemetry

1. Ensure `LastCheckResult` handles `None` smoother state/metrics
2. Verify `OriginRateLimiterState.smoother` is `None` when smoother disabled
3. Update tracing middleware to handle optional smoother spans
4. Add documentation for telemetry behavior without smoother

### Phase 6: Update Tests

1. Add test for `OriginRateLimiter::new()` without smoother
2. Add test for `OriginRateLimiter::with_smoother()` with smoother
3. Add test for `check()` behavior without smoother
4. Add test for `state()` returning `None` smoother state
5. Add test for `wait()` being no-op without smoother
6. Update existing tests to use new constructors where appropriate
7. Ensure all existing tests still pass

### Phase 7: Documentation

1. Update `OriginRateLimiter` struct documentation
2. Add examples for both constructor variants
3. Update API reference documentation
4. Add migration guide for breaking changes (if any)
5. Update CHANGELOG with API changes

## Benefits

### 1. Flexible Rate Limiting

Users can choose whether to use smoothing based on their needs:
- Per-policy rate limiting only (simpler, less overhead)
- Smoothing + per-policy (current behavior, full-featured)

### 2. Reduced Overhead

When smoother is disabled:
- No smoother instantiation (memory savings)
- No smoother checks (CPU savings)
- Smaller state snapshots (telemetry bandwidth)
- Cleaner traces (no smoother spans)

### 3. Clearer Intent

API makes it explicit whether smoothing is enabled:
```rust
let limiter = OriginRateLimiter::new();  // Clearly: no smoothing
let limiter = OriginRateLimiter::with_smoother(config);  // Clearly: smoothing enabled
```

### 4. Better Use Case Alignment

Different use cases have different requirements:
- **API gateway with per-endpoint limits**: Per-policy only
- **API client with burst protection**: Smoothing + per-policy
- **Service mesh with global limits**: Per-policy only
- **Microservice with token bucket**: Smoothing + per-policy

### 5. Migration Path

Existing code can be updated incrementally:
- Keep using old constructor during transition
- Migrate to new constructors gradually
- No breaking change if both constructors coexist

## Tradeoffs

### Smoother Enabled (current behavior)

**Pros:**
- Burst protection prevents API server overload
- Smooths out request spikes
- Better for rate-limited external APIs

**Cons:**
- Adds complexity to rate limiting logic
- Small performance overhead (governor operations)
- Telemetry includes smoother data (even if not needed)

### Smoother Disabled

**Pros:**
- Simpler rate limiting (only per-policy)
- Lower overhead (no smoother checks)
- Clearer telemetry (no smoother spans)
- Easier to understand and debug

**Cons:**
- No burst protection
- Request spikes can hit API limits directly
- May need external rate limiting for burst protection

**Recommendation:** Make smoother optional, default to disabled (simpler behavior), let users opt-in to smoothing.

## Performance Impact

### Memory Overhead

| Component | With Smoother | Without Smoother | Savings |
|-----------|---------------|------------------|---------|
| Smoother struct | ~200 bytes | 0 bytes | 200 bytes |
| Smoother state in telemetry | ~50 bytes | 0 bytes | 50 bytes |
| Total per limiter | ~250 bytes | 0 bytes | ~250 bytes |

### CPU Overhead

| Operation | With Smoother | Without Smoother | Savings |
|-----------|---------------|------------------|---------|
| check() | ~1μs + governor | ~0.5μs (policies only) | ~0.5μs |
| state() | ~0.2μs (smoother snapshot) | ~0.1μs (no snapshot) | ~0.1μs |
| wait() | ~0.1μs | ~0.01μs (no-op) | ~0.09μs |

**Per-request savings:** ~0.6μs when smoother disabled
**At 100 RPS:** ~60 μs/sec savings (negligible but measurable)
**At 10,000 RPS:** ~6 ms/sec savings (more significant at scale)

## Breaking Changes

### Option 1: Non-Breaking (Recommended)

Keep both constructors:
```rust
// Old constructor (deprecated but still works)
#[deprecated(since = "0.x.0", note = "Use new() or with_smoother()")]
pub fn new(smoother_config: SmootherConfig) -> Self {
    Self::with_smoother(smoother_config)
}

// New constructors
pub fn new() -> Self { /* no smoother */ }
pub fn with_smoother(config: SmootherConfig) -> Self { /* with smoother */ }
```

**Pros:**
- Existing code continues to work
- Gradual migration path
- No forced upgrades

**Cons:**
- API surface area slightly larger
- Deprecation warnings in IDEs

### Option 2: Breaking Change

Replace old constructor with new:
```rust
// Only new constructors
pub fn new() -> Self { /* no smoother */ }
pub fn with_smoother(config: SmootherConfig) -> Self { /* with smoother */ }
```

**Pros:**
- Cleaner API surface
- Clearer intent
- No deprecated code

**Cons:**
- Forces immediate upgrade
- Breaking change for users
- Requires major version bump

**Recommendation:** Option 1 (non-breaking) for smoother adoption.

## Success Criteria

- [ ] `OriginRateLimiter.smoother` field changed to `Option<Smoother>`
- [ ] `OriginRateLimiter::new()` constructor added (no smoother)
- [ ] `OriginRateLimiter::with_smoother()` constructor added (with smoother)
- [ ] Old `OriginRateLimiter::new(config)` deprecated (non-breaking path)
- [ ] `check()` method handles `None` smoother correctly
- [ ] `wait()` method handles `None` smoother correctly
- [ ] `reconfigure_smoother()` handles `None` smoother correctly
- [ ] `state()` returns `None` for smoother when disabled
- [ ] `LastCheckResult` handles `None` smoother state/metrics
- [ ] All existing tests pass with new implementation
- [ ] New tests for optional smoother behavior
- [ ] Telemetry correctly reports `None` smoother when disabled
- [ ] Documentation updated with examples
- [ ] Migration guide provided (if breaking)
- [ ] CHANGELOG updated with API changes
- [ ] No performance regression when smoother enabled
- [ ] Measurable performance improvement when smoother disabled

## Related Work

- **PLAN-metrics-eager.md**: State capture architecture (already supports `Option<SmootherState>`)
- **PLAN-tracing-decompose.md**: Per-limiter tracing spans (already handles optional components)
- **Ticket archive-list-lo6**: Per-limiter tracing spans (epic)
- **Ticket archive-list-e6a**: Optional semaphores (similar pattern for optional components)

## Open Questions

1. **Default behavior:** Should `new()` enable or disable smoother?
   - **Recommendation:** Disable smoother by default (simpler, less surprising)

2. **Constructor naming:** `with_smoother()` vs `with_smoother_config()` vs `with_config()`?
   - **Recommendation:** `with_smoother()` (clearer intent)

3. **Breaking change:** Should we make this a breaking change or support both?
   - **Recommendation:** Non-breaking (support both constructors, deprecate old)

4. **Telemetry spans:** Should we omit smoother spans entirely or show them as "disabled"?
   - **Recommendation:** Omit entirely (cleaner traces, less noise)

5. **Rate limit errors:** When smoother is disabled, can we still return `RateLimitViolation::Smoothed`?
   - **Recommendation:** No, only return policy violations when smoother is disabled

6. **SmootherConfig defaults:** Should `with_smoother()` accept `Option<SmootherConfig>` or require explicit config?
   - **Recommendation:** Require explicit config (clearer intent, no hidden defaults)

7. **Wait semantics:** Should `wait()` be a no-op when smoother is disabled, or panic?
   - **Recommendation:** No-op (graceful degradation, similar to how semaphores behave when not configured)
