# Per-Limiter Tracing Decomposition Plan

## Overview

Create per-limiter tracing spans (semaphore acquisition, governor check, policy checks) to provide granular observability into rate limiting operations. This decomposes the monolithic "rate limit" decision into traceable steps.

## Problem Statement

**Current tracing**: Single HTTP request span enriched with governor state.

**Missing visibility**:
- Which limiter blocked the request? (semaphore vs governor)
- How long did each limiter take? (semaphore wait, governor check duration)
- Which policy failed? (if rate limited)
- Did semaphores release correctly? (guard drop)
- Did we retry after semaphore wait?

**Trace example showing the problem**:
```
http.request
└── rate_limit.state (aggregated snapshot)
```

Can't see:
- Semaphore acquired in 2ms
- Governor check passed in 1ms
- Policy X failed (rate limited)
- Total limiter time: 3ms

## Proposed Architecture

### Span Hierarchy

```
http.request                    (reqwest-tracing)
├── rate_limiter.global_semaphore.acquire  (new)
│   ├── duration_ms: 2
│   └── permits_available: 98
├── rate_limiter.origin_semaphore.acquire  (new)
│   ├── origin: "github.com"
│   ├── duration_ms: 1
│   └── permits_available: 9
├── rate_limiter.smoother.check  (new)
│   ├── origin: "github.com"
│   ├── duration_ms: 1
│   └── velocity: 1.5
├── rate_limiter.policy.burst.check  (new)
│   ├── origin: "github.com"
│   ├── policy: "burst"
│   ├── duration_ms: 2
│   └── remaining: 75
├── rate_limiter.policy.daily.check  (new)
│   ├── origin: "github.com"
│   ├── policy: "daily"
│   ├── duration_ms: 1
│   └── remaining: 8500
└── rate_limit.state  (enriched aggregate)
    ├── all_checks_passed: false
    └── limiting_policy: "burst"
```

### Per-Limiter Span Types

```rust
pub enum LimiterSpanType {
    GlobalSemaphoreAcquire {
        duration: Duration,
        permits_available: usize,
    },
    GlobalSemaphoreRelease {
        // Just tracking release event
    },
    PerOriginSemaphoreAcquire {
        origin: String,
        duration: Duration,
        permits_available: usize,
    },
    SmootherCheck {
        origin: String,
        duration: Duration,
        velocity: f64,
    },
    PolicyCheck {
        origin: String,
        policy: String,
        duration: Duration,
        remaining: usize,
        passed: bool,  // Key for policy failure detection
        wait_duration: Option<Duration>,  // If rate limited
    },
}
```

## Implementation Approach

### Phase 1: State Saving

**Modify `OriginRateLimiter::check()`** to save last result:

```rust
impl OriginRateLimiter {
    // New field for last result
    last_check_result: Arc<RwLock<Option<LastCheckResult>>>,

    pub fn check(&self) -> Result<(), RateLimitViolation> {
        let start = Instant::now();

        // Check each limiter with timing
        let smoother_result = time_it("smoother.check", || self.smoother.check());

        let mut policy_results = Vec::new();
        for (name, slot) in &self.slots {
            let result = time_it(&format!("policy.{}.check", name), || slot.check());
            policy_results.push((name.clone(), result));
        }

        let elapsed = start.elapsed();

        // Save detailed results
        let last_result = LastCheckResult {
            timestamp: Instant::now(),
            smoother: Some(smoother_result),
            policies: policy_results,
            total_duration: elapsed,
        };

        *self.last_check_result.write().unwrap() = Some(last_result);

        // Determine overall result (existing logic)
        if smoother_result.is_err() || policy_results.iter().any(|(_, r)| r.is_err()) {
            Err(determine_violation(&smoother_result, &policy_results))
        } else {
            Ok(())
        }
    }
}

pub struct LastCheckResult {
    pub timestamp: Instant,
    pub smoother: Option<CheckResult>,
    pub policies: Vec<(String, CheckResult)>,

    // Aggregate state
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
}

pub struct CheckResult {
    pub duration: Duration,
    pub passed: bool,
    pub wait_duration: Option<Duration>,  // For rate limits
}

fn time_it<R>(name: &'static str, f: impl FnOnce() -> R) -> CheckResult {
    let start = Instant::now();
    let result = f();
    let duration = start.elapsed();

    CheckResult {
        duration,
        passed: result.is_ok(),
        wait_duration: result.err().map(|v| match v {
            RateLimitViolation::Smoothed { wait_duration } => wait_duration,
            RateLimitViolation::PolicyExceeded { wait_duration, .. } => wait_duration,
        }),
    }
}
```

### Phase 2: Span Creation Middleware

**New middleware**: `PerLimiterTraceMiddleware`

```rust
use tracing::info;
use crate::origin_limiter::LastCheckResult;

pub struct PerLimiterTraceMiddleware {
    enable_per_limiter_spans: bool,
}

#[async_trait::async_trait]
impl Middleware for PerLimiterTraceMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        if !self.enable_per_limiter_spans {
            return next.run(req, extensions).await;
        }

        // Get last check result before request
        let last_result = self.registry.get_last_check_result();

        // Create span for each limiter operation
        if let Some(ref result) = last_result {
            // Smoother span
            if let Some(ref smoother) = result.smoother {
                tracing::info!(
                    parent: None,
                    rate_limiter.smoother.check.duration_ms = smoother.duration.as_millis(),
                    rate_limiter.smoother.check.passed = smoother.passed,
                    rate_limiter.smoother.check.wait_duration_ms = smoother.wait_duration.map(|d| d.as_millis()),
                    rate_limiter.smoother.velocity = result.velocity.unwrap_or(0.0),
                    "rate_limiter.smoother.check"
                );
            }

            // Policy spans
            for (name, policy_result) in &result.policies {
                let span_name = format!("rate_limiter.policy.{}.check", name);

                if policy_result.passed {
                    tracing::info!(
                        parent: None,
                        rate_limiter.policy.check.duration_ms = policy_result.duration.as_millis(),
                        rate_limiter.policy.check.passed = true,
                        rate_limiter.policy.name = name,
                        "{}",
                        span_name
                    );
                } else {
                    let wait_ms = policy_result.wait_duration.as_ref().map(|d| d.as_millis());
                    let passed = false;

                    tracing::warn!(
                        parent: None,
                        rate_limiter.policy.check.duration_ms = policy_result.duration.as_millis(),
                        rate_limiter.policy.check.passed = false,
                        rate_limiter.policy.name = name,
                        rate_limiter.policy.check.wait_duration_ms = wait_ms.unwrap_or(0),
                        "{}",
                        span_name
                    );
                }
            }
        }

        // Execute request
        next.run(req, extensions).await
    }
}
```

### Phase 3: Integration with State Saving

**Modify `OriginRateLimiterState`** building:

```rust
impl OriginRateLimiter {
    pub fn state(&self) -> OriginRateLimiterState {
        let smoother_state = self.smoother.state();

        let policy_states: Vec<_> = self.slots
            .values()
            .map(|slot| slot.state())
            .collect();

        // Use saved last result instead of re-checking
        let last_check = self.last_check_result.read().unwrap();
        let (will_throttle, limiting_policy) = match &*last_check {
            Some(result) => (
                !result.all_passed,
                result.limiting_policy.clone(),
            ),
            None => (
                // Fallback: check without saving result
                self.check().is_err(),
                self.check().err().and_then(|v| match v {
                    RateLimitViolation::Smoothed { .. } => None,
                    RateLimitViolation::PolicyExceeded { policy_name, .. } => Some(policy_name),
                }),
            ),
        };

        OriginRateLimiterState {
            smoother: Some(smoother_state),
            policies: policy_states,
            will_throttle,
            throttle_wait_duration: last_check
                .as_ref()
                .and_then(|r| r.total_duration.clone().into()),
        }
    }
}
```

## Benefits

### 1. Root Cause Analysis

**Before**: Request failed, which limiter blocked it?
```
HTTP 429 (rate limited)
└── rate_limit.will_throttle: true
```

**After**: Clear sequence showing failure point
```
rate_limiter.smoother.check (passed, 1ms)
├── rate_limiter.policy.burst.check (FAILED, 2ms)
│   └── rate_limiter.policy.burst.check.wait_duration_ms: 500
├── rate_limiter.policy.daily.check (passed, 1ms)
└── rate_limiter.state (limiting_policy: burst)
```

Immediately see: **Burst policy caused rate limit**, waited 500ms.

### 2. Performance Optimization Detection

**Trace sample**:
```
rate_limiter.smoother.check (10ms)  ← Slower than expected?
rate_limiter.policy.burst.check (1ms)
rate_limiter.policy.daily.check (2ms)
Total: 13ms before request
```

**Analysis**: Smoother took 10ms, investigating governor config or GC pause.

### 3. Retried Request Detection

**Trace sample**:
```
rate_limiter.global_semaphore.acquire (passed)
rate_limiter.smoother.check (FAILED: wait 100ms)
rate_limiter.global_semaphore.acquire (passed)  ← Retry!
rate_limiter.smoother.check (passed)
HTTP request succeeds
```

**Analysis**: Request retried after semaphore wait, showing retry logic working.

### 4. Per-Policy Statistics

**Trace aggregation**:
```
rate_limiter.policy.burst.check
├── passed: 950 times
├── failed: 50 times
└── avg_duration_ms: 1.2

rate_limiter.policy.daily.check
├── passed: 1000 times
└── failed: 0 times
```

**Analysis**: Burst policy causing most rate limits, daily never hit.

### 5. Multi-Origin Behavior

**Trace sample**:
```
rate_limiter.smoother.check origin="github.com" (passed)
rate_limiter.smoother.check origin="gitlab.com" (FAILED)
rate_limiter.smoother.check origin="bitbucket.org" (passed)
```

**Analysis**: GitLab rate limited, others OK, isolated to that origin.

## Naming Convention

Use consistent span attribute names for observability tools:

| Limiter Type | Span Name | Attributes |
|--------------|-----------|------------|
| Global Semaphore | `rate_limiter.global_semaphore.{acquire,release}` | `duration_ms`, `permits_available` |
| Per-Origin Semaphore | `rate_limiter.origin_semaphore.{acquire,release}` | `duration_ms`, `origin`, `permits_available` |
| Smoother | `rate_limiter.smoother.check` | `duration_ms`, `velocity`, `passed`, `wait_duration_ms` |
| Policy Check | `rate_limiter.policy.{policy_name}.check` | `duration_ms`, `policy`, `passed`, `remaining`, `wait_duration_ms` |

## Configuration

### Global Toggle

```rust
// In SmootherConfig or new TracingConfig
pub struct TracingConfig {
    pub enable_per_limiter_spans: bool,  // Disabled by default
    pub enable_smoother_spans: bool,
    pub enable_policy_spans: bool,
}
```

### Runtime Control

```rust
// Environment variables
RQL_TRACE_PER_LIMITER=false  // Disable entirely
RQL_TRACE_SMOOTHER=true      // Smoother spans only
RQL_TRACE_POLICIES=true       // Policy spans only

// Dynamic via runtime config
tracing::subscriber::set_global_default(
    tracing_subscriber::Registry::default()
        .with(env_filter::EnvFilter::from_default_env())
)
)?;
```

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_last_result_saved() {
        let limiter = OriginRateLimiter::new(SmootherConfig::default());
        limiter.check();

        let result = limiter.last_check_result.read().unwrap();
        assert!(result.is_some());
        assert!(result.as_ref().unwrap().smoother.is_some());
    }

    #[tokio::test]
    async fn test_span_creation() {
        // Set up tracing subscriber to capture spans
        let subscriber = tracing_subscriber::Registry::default()
            .with(tracing_subscriber::fmt::test().with_writer(tracing_subscriber::TestWriter::new()))
            .with(tracing::layer::IdentityLayer::new());
        tracing::subscriber::set_global_default(subscriber);

        let limiter = OriginRateLimiter::new(SmootherConfig::default());
        let _ = limiter.check();

        // Verify spans created (mockable via span hooks)
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_trace_hierarchies() {
    let limiter = OriginRateLimiter::new(SmootherConfig::default());

    // Make multiple requests
    for i in 0..5 {
        limiter.check();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Verify trace shows:
    // 1. Each limiter operation as separate span
    // 2. Timing information for each
    // 3. Parent-child relationships
    // 4. Which policy caused throttling
}
```

## Performance Impact

### Storage Overhead

- `LastCheckResult`: ~200 bytes per check
- `Arc<RwLock<Option<...>>>`: ~48 bytes + mutex overhead
- **Per-request**: ~248 bytes total
- **Per-second**: At 100 RPS: ~24KB/sec (negligible vs HTTP payload)

### Span Creation Overhead

- Per limiter span: ~100ns (tracing span creation)
- Attribute recording: ~50ns per attribute
- 5 limiter spans = ~750ns total
- **Per-request overhead**: ~0.75μs (750 nanoseconds)

### Comparison: Single vs Decomposed

| Metric | Single Span | Decomposed Spans | Overhead |
|--------|--------------|------------------|---------|
| Span creation | 1 | 5 | +400ns |
| Attribute count | 8 | 20 | +600ns |
| Total overhead | ~1μs | ~1.75μs | +750ns |
| Observability | Low | **High** | Worth it |

## Success Criteria

- [ ] `LastCheckResult` struct added to `origin_limiter.rs`
- [ ] `check()` method saves detailed results with timing
- [ ] `state()` uses saved last result (avoids re-checking)
- [ ] `OriginRateLimiterState` includes `limiting_policy` field
- [ ] `PerLimiterTraceMiddleware` implemented
- [ ] Per-limiter spans: smoother, policies, semaphores
- [ ] Span naming convention documented
- [ ] Configuration options (global enable, per-type toggles)
- [ ] Environment variable support
- [ ] All unit tests passing
- [ ] Integration tests with trace verification
- [ ] Performance benchmarks showing <2μs overhead
- [ ] Documentation updated with examples
- [ ] Backward compatible (disabled by default)

## Open Questions

1. **State retention**: How long to keep `LastCheckResult`? Per-request, per-second?
   - Recommendation: Per-request (single value), older results lost naturally

2. **Span sampling**: Should per-limiter spans be sampled at 10% to reduce overhead?
   - Recommendation: No, rate limiting is critical path, always need full visibility

3. **Attribute precision**: Use Duration::as_millis() or as_nanos()?
   - Recommendation: Milliseconds for readability, nanoseconds for precision if needed (configurable)

4. **Error vs Warning**: Use `tracing::warn()` for failed checks or `tracing::error()`?
   - Recommendation: `warn()` for expected rate limits, `error()` for unexpected failures

5. **Backward compatibility**: Should per-limiter spans require feature flag?
   - Recommendation: No, runtime config is sufficient for testing

## Related Plans

- **PLAN-semaphore-limit.md**: Semaphore/concurrency limiting architecture
- **PLAN-composable-limiters.md**: Composable limiter patterns (unified acquisition)
- **doc/PLUGIN-architecture.md**: Plugin hooks for extensibility
