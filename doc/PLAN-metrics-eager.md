# Metrics Eager Capture Plan

## Overview

Enhance `LastCheckResult` to capture full governor state snapshots during `check()`, enabling precise correlation between rate limit decisions and the state that caused them. This eliminates the gap between "did we rate limit?" and "what was the state at that moment?"

## Problem Statement

**Current implementation:**
- `check()` captures timing metrics (`CheckMetrics`) with pass/fail status
- `state()` fetches fresh governor state by calling `smoother.state()` / `slot.state()`
- **Gap**: `state()` reflects current state, not state **at the moment of check**

**Example of the problem:**
```rust
limiter.check();  // Request A checks, remaining=95
                 // Request B runs, consumes 1, remaining=94
limiter.state();  // Returns remaining=94 (stale relative to Request A's check)
```

**For tracing/spans:** We want state **at the moment check happened** (remaining=95), not current state.
**For monitoring:** We want real-time state (remaining=94).

Both needs are valid - we need to support both.

## State Staleness Analysis

| State Field | Changes When | Staleness Impact | Use Case Preference |
|-------------|---------------|------------------|---------------------|
| `remaining_burst_capacity` | Every `check()` call | HIGH for monitoring, LOW for tracing | Tracing wants check-time, monitoring wants fresh |
| `remaining` (policies) | Every `check()` call | HIGH for monitoring, LOW for tracing | Tracing wants check-time, monitoring wants fresh |
| `quota` | `update_limits()` from API | LOW - rare | Either is fine |
| `burst_capacity` | `update_limits()` from API | LOW - rare | Either is fine |
| `window_secs` | `update_policies()` from API | LOW - rare | Either is fine |
| `all_passed` | Never changes after capture | NONE - always accurate | Check-time is only option |
| `limiting_policy` | Never changes after capture | NONE - always accurate | Check-time is only option |
| `duration`, `wait_duration` | Never changes after capture | NONE - always accurate | Check-time is only option |

**Key insight:** Staleness is a **feature** for tracing, not a bug! The stale state represents the exact state that caused the rate limit decision.

## Proposed Architecture

### Enhanced LastCheckResult

```rust
#[derive(Debug, Clone)]
pub struct LastCheckResult {
    pub timestamp: Instant,
    
    // Full state snapshots (captured at check time)
    pub smoother_state: Option<SmootherState>,
    pub policy_states: Vec<(String, PolicySlotState)>,
    
    // Timing metrics (unchanged)
    pub smoother_metrics: Option<CheckMetrics>,
    pub policy_metrics: Vec<(String, CheckMetrics)>,
    
    // Aggregate info
    pub total_duration: Duration,
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
}
```

### OriginRateLimiterState with Dual Mode

```rust
#[derive(Debug, Clone)]
pub struct OriginRateLimiterState {
    pub smoother: Option<SmootherState>,
    pub policies: Vec<PolicySlotState>,
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,
    
    // Historical snapshot (from last check)
    pub last_check: Option<LastCheckResult>,
    
    // Mode flag for clarity
    pub mode: StateMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateMode {
    Historical,  // Using state from last check (for tracing)
    Fresh,       // Using fresh governor state (for monitoring)
}
```

### New API Methods

```rust
impl OriginRateLimiter {
    /// Get state snapshot for telemetry (uses last check's state if available)
    pub fn state(&self) -> OriginRateLimiterState {
        let last_check = self.last_check_result.read().unwrap().clone();
        
        if let Some(result) = last_check {
            OriginRateLimiterState {
                smoother: result.smoother_state.clone(),
                policies: result.policy_states.iter().map(|(_, s)| s.clone()).collect(),
                will_throttle: !result.all_passed,
                throttle_wait_duration: /* compute from metrics */,
                last_check: Some(result.clone()),
                mode: StateMode::Historical,
            }
        } else {
            // Fallback: check + fresh state snapshot
            self.fresh_state()
        }
    }
    
    /// Get fresh current state (always calls governor, no caching)
    pub fn fresh_state(&self) -> OriginRateLimiterState {
        self.check();  // Consumes permit
        OriginRateLimiterState {
            smoother: Some(self.smoother.state()),
            policies: self.slots.values().map(|slot| slot.state()).collect(),
            will_throttle: /* re-check */,
            throttle_wait_duration: /* re-check */,
            last_check: self.last_check_result.read().unwrap().clone(),
            mode: StateMode::Fresh,
        }
    }
    
    /// Get historical state from last check (no governor calls)
    pub fn historical_state(&self) -> Option<OriginRateLimiterState> {
        let last_check = self.last_check_result.read().unwrap().clone()?;
        
        Some(OriginRateLimiterState {
            smoother: last_check.smoother_state,
            policies: last_check.policy_states.into_iter().map(|(_, s)| s).collect(),
            will_throttle: !last_check.all_passed,
            throttle_wait_duration: /* compute from metrics */,
            last_check: Some(last_check),
            mode: StateMode::Historical,
        })
    }
}
```

## Use Cases

### 1. Tracing/Span Enrichment (Historical Mode)

**Goal:** Correlate rate limit decision with state that caused it.

```rust
// In middleware, during request processing
let check_result = limiter.check();
let state = limiter.historical_state().unwrap_or_else(|| limiter.state());

// Enrich span
span.record("rate_limit.remaining", state.policies[0].remaining);
span.record("rate_limit.limiting_policy", state.limiting_policy().unwrap_or("none"));
span.record("rate_limit.mode", "historical");  // Indicates this is check-time state
```

**Why historical mode:**
- Request was rate limited based on `remaining=50` at check time
- Current state might be `remaining=55` (other requests completed)
- Tracing should show the 50, not the 55

### 2. Monitoring/Dashboard (Fresh Mode)

**Goal:** Show real-time capacity for operational visibility.

```rust
// In metrics collection loop
let state = limiter.fresh_state();

// Export to Prometheus/Datadog
gauge!("rate_limit.remaining", state.policies[0].remaining);
gauge!("rate_limit.available_capacity", /* computed */);
```

**Why fresh mode:**
- Operator wants to know current capacity now
- Historical state is stale and misleading for monitoring

### 3. Fallback Behavior

```rust
// If no check has been called yet, fall back to fresh state
pub fn state(&self) -> OriginRateLimiterState {
    if let Some(result) = self.historical_state() {
        result
    } else {
        self.fresh_state()  // First check, capture + return
    }
}
```

## Implementation Steps

### Phase 1: Enhance CheckResult Types

1. Add `smoother_state: Option<SmootherState>` to `LastCheckResult`
2. Add `policy_states: Vec<(String, PolicySlotState)>` to `LastCheckResult`
3. Rename `smoother` → `smoother_metrics` for clarity
4. Rename `policies` → `policy_metrics` for clarity

### Phase 2: Capture State in check()

```rust
pub fn check(&self) -> Result<(), RateLimitViolation> {
    let start = Instant::now();

    // Check smoother + capture metrics + state
    let smoother_start = Instant::now();
    let smoother_result = self.smoother.check();
    let smoother_state = self.smoother.state();  // NEW: Capture state
    let smoother_metrics = CheckMetrics { /* ... */ };

    // Check policies + capture metrics + state
    let mut policy_results = Vec::new();
    let mut policy_states = Vec::new();  // NEW: Collect states
    
    for (name, slot) in &self.slots {
        let policy_start = Instant::now();
        let policy_result = slot.check();
        let policy_state = slot.state();  // NEW: Capture state
        let policy_metrics = CheckMetrics { /* ... */ };

        policy_results.push((name.clone(), policy_result, policy_metrics));
        policy_states.push((name.clone(), policy_state));
    }

    // Save all state + metrics
    let last_result = LastCheckResult {
        smoother_state: Some(smoother_state),
        policy_states,
        smoother_metrics: Some(smoother_metrics),
        policy_metrics: policy_results.into_iter()
            .map(|(name, _, metrics)| (name, metrics))
            .collect(),
        /* ... other fields ... */
    };

    *self.last_check_result.write().unwrap() = Some(last_result);

    // Determine result
    /* ... existing logic ... */
}
```

### Phase 3: Update state() Methods

```rust
impl OriginRateLimiter {
    pub fn state(&self) -> OriginRateLimiterState {
        self.historical_state()
            .unwrap_or_else(|| self.fresh_state())
    }
    
    pub fn fresh_state(&self) -> OriginRateLimiterState {
        self.check();
        self.state()  // Now uses historical state from just-completed check
    }
    
    pub fn historical_state(&self) -> Option<OriginRateLimiterState> {
        let last = self.last_check_result.read().unwrap().clone()?;
        
        Some(OriginRateLimiterState {
            smoother: last.smoother_state,
            policies: last.policy_states.into_iter().map(|(_, s)| s).collect(),
            will_throttle: !last.all_passed,
            throttle_wait_duration: /* from metrics */,
            last_check: Some(last),
            mode: StateMode::Historical,
        })
    }
}
```

### Phase 4: Update OriginRateLimiterState

```rust
#[derive(Debug, Clone)]
pub struct OriginRateLimiterState {
    pub smoother: Option<SmootherState>,
    pub policies: Vec<PolicySlotState>,
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,
    pub last_check: Option<LastCheckResult>,
    
    // NEW: Mode indicator
    pub mode: StateMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateMode {
    Historical,
    Fresh,
}
```

### Phase 5: Update Telemetry Middleware

```rust
// In RateLimitTelemetry middleware
impl Middleware for RateLimitTelemetry {
    async fn handle(&self, req: Request, extensions: &mut Extensions, next: Next<'_>) -> Result<Response> {
        let state = self.rate_limiter.state();
        
        // Enrich span with check-time state
        if let Some(ref last_check) = state.last_check {
            span.record("rate_limit.mode", "historical");
            span.record("rate_limit.timestamp_ms", last_check.timestamp.elapsed().as_millis());
            span.record("rate_limit.all_passed", last_check.all_passed);
            
            if let Some(ref limiting_policy) = last_check.limiting_policy {
                span.record("rate_limit.limiting_policy", limiting_policy);
            }
        }
        
        next.run(req, extensions).await
    }
}
```

### Phase 6: Add Tests

```rust
#[test]
fn test_state_capture_during_check() {
    let mut limiter = OriginRateLimiter::new(SmootherConfig::default());
    limiter.update_policies(/* ... */);
    
    limiter.check();
    let state = limiter.historical_state().unwrap();
    
    // Verify state was captured
    assert!(state.smoother.is_some());
    assert_eq!(state.policies.len(), 1);
    assert_eq!(state.mode, StateMode::Historical);
}

#[test]
fn test_fresh_state_fetches_current() {
    let limiter = OriginRateLimiter::new(SmootherConfig::default());
    limiter.update_policies(/* ... */);
    
    limiter.check();  // Remaining = 99
    
    let state1 = limiter.historical_state().unwrap();
    assert_eq!(state1.policies[0].remaining, 99);
    
    limiter.check();  // Remaining = 98
    
    let state2 = limiter.historical_state().unwrap();
    assert_eq!(state2.policies[0].remaining, 98);  // Updated!
}

#[test]
fn test_state_uses_historical_by_default() {
    let limiter = OriginRateLimiter::new(SmootherConfig::default());
    limiter.update_policies(/* ... */);
    
    limiter.check();
    let state = limiter.state();
    
    // Should use historical (no new governor calls)
    assert_eq!(state.mode, StateMode::Historical);
}
```

## Benefits

### 1. Precise Tracing Correlation

**Before:**
```
Request rate limited at T1
State shows remaining=94 (captured at T2)
Time gap: 50ms, 1 request in between
```

**After:**
```
Request rate limited at T1
State shows remaining=95 (captured at T1)
Perfect correlation: state at moment of decision
```

### 2. Eliminates "Time Travel" State

Tracing shows the exact state that caused the behavior, preventing confusion when:
- Multiple concurrent requests run
- State changes rapidly
- Rate limits are triggered intermittently

### 3. Supports Both Use Cases

- **Historical mode**: For tracing/spans (default via `state()`)
- **Fresh mode**: For monitoring dashboards (via `fresh_state()`)

### 4. Performance Improvement

No extra governor calls in `state()` when historical data is available:
- `smoother.state()` - saved from check
- `slot.state()` - saved from check
- Only metrics construction (cheap)

### 5. Backward Compatible

Default `state()` behavior preserved:
- Returns historical if available
- Falls back to fresh state on first call
- Existing code continues to work

## Tradeoffs

### Historical Mode (check-time state)

**Pros:**
- Precise correlation with rate limit decision
- Perfect for tracing and debugging
- No extra governor calls
- Predictable: state doesn't change after capture

**Cons:**
- Stale for monitoring purposes
- Can be misleading for capacity planning
- Requires explicit fresh_state() for real-time views

### Fresh Mode (current state)

**Pros:**
- Real-time capacity monitoring
- Accurate for dashboards/alerts
- Shows current system state

**Cons:**
- Uncorrelated with rate limit decision
- State may have changed since check
- Requires governor calls (permit consumption)

**Recommendation:** Use historical by default, offer fresh as explicit option.

## Performance Impact

### Storage Overhead

- `SmootherState`: ~50 bytes
- `PolicySlotState`: ~30 bytes per policy
- At 5 policies: ~200 bytes total
- **Per request:** ~200 bytes (negligible vs HTTP payload)

### Computation Overhead

- State capture in `check()`: Already calling `state()` methods, just storing result
- Historical `state()`: Zero governor calls (just clone saved data)
- Fresh `state()`: Same as before (governor calls required)
- **Net change:** Zero overhead in check(), reduced in historical state()

## Future Enhancement: Conditional Metrics Capture

### Current Issue

**Always-on behavior:**
- Every `check()` call captures full state snapshots and timing metrics
- ~200 bytes per request stored in `LastCheckResult`
- Cloning overhead even when user doesn't need detailed tracing

**Impact:**
- High-frequency rate limiting scenarios: 1000 RPS = 200KB/sec of state snapshots
- Many users only need pass/fail, not detailed timing per limiter
- Unnecessary memory allocation and GC pressure

### Proposed Enhancement

Add configuration flag to conditionally enable metrics capture:

```rust
pub struct SmootherConfig {
    pub micro_interval_secs: u32,
    pub velocity: f64,
    pub capture_metrics: bool,  // NEW: Enable/disable state snapshots
}

#[derive(Debug, Clone)]
pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Smoother,
    fastest_policy: Option<String>,
    last_check_result: Arc<RwLock<Option<LastCheckResult>>>,
    capture_metrics: bool,  // NEW: Configuration flag
}

impl OriginRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(smoother_config),
            fastest_policy: None,
            last_check_result: Arc::new(RwLock::new(None)),
            capture_metrics: smoother_config.capture_metrics,
        }
    }

    pub fn check(&self) -> Result<(), RateLimitViolation> {
        let start = Instant::now();

        if self.capture_metrics {
            // Capture full state + metrics (current implementation)
            self.check_with_metrics_capture()
        } else {
            // Only check, don't capture state/metrics
            self.check_without_metrics_capture()
        }
    }

    fn check_with_metrics_capture(&self) -> Result<(), RateLimitViolation> {
        // ... current implementation ...
    }

    fn check_without_metrics_capture(&self) -> Result<(), RateLimitViolation> {
        // Simplified check without state snapshot
        self.smoother.check().map_err(|not_until| RateLimitViolation::Smoothed {
            wait_duration: not_until.wait_time_from(self.smoother.clock().now()),
        })?;

        for (name, slot) in &self.slots {
            slot.check().map_err(|not_until| RateLimitViolation::PolicyExceeded {
                policy_name: name.clone(),
                wait_duration: not_until.wait_time_from(slot.clock().now()),
            })?;
        }

        Ok(())
    }
}
```

### Tradeoffs

| Mode | Benefits | Costs |
|-------|-----------|--------|
| **Always capture (current)** | - Always available for tracing<br>- No configuration needed<br>- Backward compatible | - ~200 bytes per request<br>- CPU overhead for cloning<br>- GC pressure |
| **Conditional capture (proposed)** | - Zero overhead when disabled<br>- Performance-optimal for production<br>- Explicit control | - Must enable for tracing<br>- Config complexity<br>- Migration path needed |
| **Hybrid (recommended)** | - Capture on rate limit only<br>- Best of both worlds<br>- Minimal overhead | - Slightly complex logic<br>- Missed metrics on passes<br>- Harder to explain |

### Recommended Approach

**Hybrid capture with tiered configuration:**

```rust
pub enum MetricsCaptureMode {
    Disabled,          // Never capture (fastest)
    OnFailure,         // Capture only on rate limit (default)
    Always,            // Always capture (full tracing)
}
```

**Benefits of hybrid:**
- `Disabled`: Production with no tracing needed
- `OnFailure` (default): Capture only when rate limit occurs (~1% of requests)
- `Always`: Full tracing for debugging/development

**Performance improvement:**
- At 1000 RPS, 1% rate limit rate: 10 captures/sec instead of 1000
- Memory reduction: 2KB/sec → 20KB/sec (100x improvement)
- Still get full context when needed (rate limit debugging)

### Implementation Plan

1. Add `MetricsCaptureMode` enum with 3 variants
2. Add `metrics_capture: MetricsCaptureMode` to `SmootherConfig`
3. Store flag in `OriginRateLimiter`
4. Modify `check()` to conditionally capture based on mode
5. Add `check_simple()` path for Disabled mode
6. Add deferred capture path for OnFailure mode
7. Update documentation with configuration examples
8. Add benchmarks for each mode
9. Add tests for mode behavior

### Configuration Examples

```rust
// Production: No tracing (fastest)
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    metrics_capture: MetricsCaptureMode::Disabled,
};

// Production: Trace failures (recommended default)
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    metrics_capture: MetricsCaptureMode::OnFailure,
};

// Development: Full tracing
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    metrics_capture: MetricsCaptureMode::Always,
};
```

### Success Criteria for Enhancement

- [ ] `MetricsCaptureMode` enum added
- [ ] Configuration field added to `SmootherConfig`
- [ ] `check()` conditionally captures based on mode
- [ ] `Disabled` mode: No state/metrics captured
- [ ] `OnFailure` mode: Captures only on rate limit
- [ ] `Always` mode: Current behavior (backward compatible)
- [ ] Benchmarks show performance improvement for Disabled/OnFailure
- [ ] Documentation updated with configuration guidance
- [ ] Migration guide provided for existing users

## Success Criteria

- [ ] `LastCheckResult` includes full state snapshots (smoother + policies)
- [ ] `check()` captures state during limiter checks
- [ ] `state()` returns historical state by default
- [ ] `fresh_state()` method returns real-time state
- [ ] `historical_state()` method returns saved state
- [ ] `StateMode` enum indicates which mode is used
- [ ] All existing tests pass
- [ ] New tests for state capture verification
- [ ] New tests for mode differentiation
- [ ] Documentation updated with use case guidance
- [ ] Telemetry middleware uses historical mode

## Related Work

- **PLAN-tracing-decompose.md**: Per-limiter tracing spans architecture
- **Ticket archive-list-2jo**: State saving to avoid double-checking (completed)
- **Ticket archive-list-lo6**: Per-limiter tracing spans (epic, pending)

## Open Questions

1. **API naming:** `fresh_state()` vs `current_state()` vs `realtime_state()`?
   - **Recommendation:** `fresh_state()` (clearer intent)

2. **Default behavior:** Should `state()` default to historical or fresh?
   - **Recommendation:** Historical (better for tracing, majority use case)

3. **State age tracking:** Should we include `timestamp` and age in span attributes?
   - **Recommendation:** Yes, include `rate_limit.state_age_ms` for staleness visibility

4. **Fallback when no check:** Should `historical_state()` return None or fall back to fresh?
   - **Recommendation:** Return None (explicit, let caller decide)

5. **Cloning cost:** State snapshots clone Arc data, but still ~200 bytes. Acceptable?
   - **Recommendation:** Yes, negligible vs HTTP overhead (1KB+)
