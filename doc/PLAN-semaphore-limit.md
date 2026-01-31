# Semaphore/Concurrency Limiting Implementation Plan

## Overview

Add optional concurrent request limiting to reqgov, complementing the existing rate limiting (permits per time window) with concurrency control (active requests at once).

### Motivation

Rate limiting controls request frequency (X requests per Y time window), but doesn't control how many requests execute simultaneously. This can lead to:

- **API server overload**: Too many concurrent requests overwhelming server resources
- **Connection exhaustion**: Client-side connection pool saturation
- **Rate limit false positives**: High concurrency can trigger rate limits faster than expected
- **Poor user experience**: Starvation when many origins compete for limited resources

Semaphores complement rate limiting by enforcing: "No more than N requests active at once, regardless of rate limits."

### Goals

1. Provide global concurrent request limit across all origins
2. Provide per-origin concurrent request limits (override global)
3. Make semaphores optional (default disabled for backward compatibility)
4. Integrate seamlessly with existing governor rate limiting
5. Add telemetry for concurrent request visibility
6. Minimal performance overhead (async semaphore acquisition)

## Configuration

### Global Configuration

```rust
// New struct for HttpApiRateLimiter configuration
pub struct HttpApiRateLimiterConfig {
    pub smoother_config: SmootherConfig,
    pub max_concurrent: Option<usize>,  // Global limit (None = disabled)
}

// Or extend existing SmootherConfig
pub struct SmootherConfig {
    pub micro_interval_secs: u32,
    pub velocity: f64,
    pub max_concurrent_global: Option<usize>,  // New field
}
```

### Per-Origin Configuration

```rust
// Extend Policy or ServiceLimit with per-origin override
pub struct Policy {
    pub name: String,
    pub quota: u32,
    pub window_secs: Option<u32>,
    pub max_concurrent: Option<usize>,  // New field: override global limit
}
```

### Configuration Examples

```rust
// Disabled by default (backward compatible)
let config = SmootherConfig::default();
// max_concurrent_global = None

// Global limit only
let config = SmootherConfig {
    micro_interval_secs: 2,
    velocity: 1.5,
    max_concurrent_global: Some(100),  // Max 100 concurrent total
};

// Per-origin override via Policy
let policy = Policy {
    name: "github-api".to_string(),
    quota: 5000,
    window_secs: Some(3600),
    max_concurrent: Some(10),  // Max 10 concurrent for this origin
};
```

## Implementation Approach

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│               HttpApiRateLimiter              │
│  ┌─────────────────────────────────────────────┐   │
│  │       Global Semaphore (optional)        │   │
│  │   Arc<Semaphore>                      │   │
│  └─────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────┐   │
│  │         OriginRegistry                  │   │
│  │  ┌───────────────────────────────────┐  │   │
│  │  │   OriginRateLimiter          │  │   │
│  │  │  ┌───────────────────────────┐ │  │   │
│  │  │  │ Per-origin Semaphore      │ │  │   │
│  │  │  │   Arc<Semaphore>        │ │  │   │
│  │  │  └───────────────────────────┘ │  │   │
│  │  │  ┌───────────────────────────┐ │  │   │
│  │  │  │ Smoother + Policies    │ │  │   │
│  │  │  │  (governor rate limit) │ │  │   │
│  │  │  └───────────────────────────┘ │  │   │
│  │  └───────────────────────────────────┘  │  │
│  └─────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### Middleware Ordering

**Critical**: Semaphores must be acquired **before** governor rate limiting:

```
Request Flow:
1. Global semaphore acquire (if configured)
2. Per-origin semaphore acquire (if configured)
3. Governor rate limit check (existing)
4. Execute HTTP request
5. Release semaphores (global, then per-origin)
```

**Reasoning**:
- Semaphore limits "how many at once"
- Governor limits "how fast"
- Semaphore first ensures concurrency cap respected even if rate limit allows
- Prevents requesting permit then waiting on semaphore (inefficient)

### Per-Request Lifecycle

```rust
// In RateLimitTelemetry or new semaphore middleware
async fn handle_request(
    req: Request,
    extensions: &mut Extensions,
    next: Next<'_>,
) -> Result<Response> {
    let _global_guard = match &self.global_semaphore {
        Some(sem) => Some(sem.acquire().await),
        None => None,
    };

    let origin = extract_origin(req);
    let _origin_guard = match self.registry.get_semaphore(origin) {
        Some(sem) => Some(sem.acquire().await),
        None => None,
    };

    // Now pass to reqwest_ratelimit (governor)
    let result = next.run(req, extensions).await;

    // Guards release automatically when dropped (RAII pattern)
    Ok(result)
}
```

### Storage Architecture

```rust
// In OriginRateLimiter
pub struct OriginRateLimiter {
    smoother: Smoother,
    slots: HashMap<String, PolicySlot>,
    semaphore: Option<Arc<Semaphore>>,  // New: per-origin semaphore
    fastest_policy: Option<String>,
}

// In HttpApiRateLimiter
pub struct HttpApiRateLimiter {
    registry: Arc<OriginRegistry>,
    global_semaphore: Option<Arc<Semaphore>>,  // New: global semaphore
    current_url: Arc<RwLock<Option<String>>>,
}

// In OriginRegistry
pub struct OriginRegistry {
    limiters: HashMap<String, Arc<RwLock<OriginRateLimiter>>>,
    // Need to expose per-origin semaphores for middleware access
    semaphores: HashMap<String, Arc<Semaphore>>,  // New: map of origin → semaphore
}
```

## API Changes

### Breaking Changes

None - semaphores are **optional** (Option<T>), defaulting to disabled.

### New APIs

```rust
// SmootherConfig
impl SmootherConfig {
    pub fn with_max_concurrent_global(self, max: usize) -> Self {
        Self {
            max_concurrent_global: Some(max),
            ..self
        }
    }
}

// Policy
impl Policy {
    pub fn with_max_concurrent(self, max: usize) -> Self {
        Self {
            max_concurrent: Some(max),
            ..self
        }
    }
}

// HttpApiRateLimiter
impl HttpApiRateLimiter {
    pub fn with_global_semaphore(self, max: usize) -> Self {
        // Set up global semaphore before registry creation
        // Requires constructor change
    }
}

// OriginRegistry
impl OriginRegistry {
    pub fn get_semaphore(&self, origin: &str) -> Option<&Arc<Semaphore>> {
        self.semaphores.get(origin)
    }

    pub fn update_semaphore(&mut self, origin: String, max: Option<usize>) {
        if let Some(max) = max {
            self.semaphores.insert(origin, Arc::new(Semaphore::new(max)));
        } else {
            self.semaphores.remove(&origin);
        }
    }
}
```

### Rate Limiting Integration

**Update `reqwest_ratelimit::RateLimiter` trait implementation**:

```rust
#[async_trait::async_trait]
impl RateLimiter for HttpApiRateLimiter {
    async fn acquire_permit(&self, ctx: &Context<'_>) {
        let origin = ctx.url().host_str().unwrap_or("default");

        // 1. Acquire global semaphore
        let _global_guard = match &self.global_semaphore {
            Some(sem) => Some(sem.acquire_owned(1).await),
            None => None,
        };

        // 2. Acquire per-origin semaphore
        let limiter = self.registry.get_or_create(origin);
        let _origin_guard = match limiter.semaphore {
            Some(sem) => Some(sem.acquire_owned(1).await),
            None => None,
        };

        // 3. Existing governor rate limiting
        let origin_limiter = limiter.read().await;
        origin_limiter.wait().await;

        // Guards release when dropped
        Ok(Permit { _marker: PhantomData })
    }
}
```

## Telemetry

### New Span Attributes

Add to existing `RateLimitTelemetry` span attributes:

```rust
// In RateLimitTelemetry or new SemaphoreTelemetry middleware
span.record("concurrency.global.available", global_semaphore.available_permits() as i64);
span.record("concurrency.global.in_use", global_semaphore.available_permits() as i64 - max_global as i64);
span.record("concurrency.per_origin.available", origin_semaphore.available_permits() as i64);
span.record("concurrency.per_origin.in_use", origin_semaphore.available_permits() as i64 - max_origin as i64);
span.record("concurrency.global.max", max_global as i64);
span.record("concurrency.per_origin.max", max_origin as i64);
```

### State Snapshot Addition

Extend `OriginRateLimiterState`:

```rust
pub struct OriginRateLimiterState {
    pub smoother: Option<SmootherState>,
    pub policies: Vec<PolicySlotState>,
    pub will_throttle: bool,
    pub throttle_wait_duration: Option<Duration>,

    // New: concurrency telemetry
    pub concurrency_global: Option<ConcurrencyState>,
    pub concurrency_per_origin: Option<HashMap<String, ConcurrencyState>>,
}

pub struct ConcurrencyState {
    pub max: usize,
    pub available: usize,
    pub in_use: usize,
    pub waiters: usize,  // If we track queue length
}
```

## Performance Considerations

### Semaphore Acquisition Overhead

- `tokio::sync::Semaphore::acquire()` is O(1) atomic operation
- Async await on acquisition adds minimal scheduling overhead
- RAII guard pattern ensures automatic release on drop
- **No blocking**: Tokio semaphore uses queue internally, no busy-wait

### Memory Overhead

- Global semaphore: `Arc<Semaphore>` + optional counter (~100 bytes)
- Per-origin semaphore: `HashMap<String, Arc<Semaphore>>` (100 origins × ~100 bytes = ~10KB)
- Negligible compared to HTTP connection buffers and governor state

### Contention Analysis

- **Worst case**: 100 concurrent requests, all at semaphore limit
- **Semaphore queue**: Tokio manages internally, no manual polling
- **Fairness**: FIFO queue for waiting tasks
- **Starvation**: Global semaphore may starve low-traffic origins if limit too tight

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    // Semaphore creation and configuration
    #[test]
    fn test_semaphore_disabled_by_default() {
        let config = SmootherConfig::default();
        assert!(config.max_concurrent_global.is_none());
    }

    #[test]
    fn test_semaphore_enabled_with_limit() {
        let config = SmootherConfig {
            max_concurrent_global: Some(10),
            ..Default::default()
        };
        assert_eq!(config.max_concurrent_global, Some(10));
    }

    // Per-origin override
    #[test]
    fn test_per_origin_override() {
        let policy = Policy::with_max_concurrent(Policy::default(), 5);
        assert_eq!(policy.max_concurrent, Some(5));
    }

    // Acquisition and release
    #[tokio::test]
    async fn test_semaphore_acquire_release() {
        let sem = Arc::new(Semaphore::new(2));

        let guard1 = sem.acquire().await;
        assert_eq!(sem.available_permits(), 1);

        let guard2 = sem.acquire().await;
        assert_eq!(sem.available_permits(), 0);

        let guard3 = sem.acquire().await;
        assert_eq!(sem.available_permits(), 0); // Wait in queue

        drop(guard1);
        assert_eq!(sem.available_permits(), 1);
    }

    // Integration with governor
    #[tokio::test]
    async fn test_semaphore_before_rate_limit() {
        let sem = Arc::new(Semaphore::new(1));
        let limiter = RateLimiter::direct(Quota::per_second(nonzero!(10)));

        // Semaphore blocks even if rate limit allows
        let _guard1 = sem.acquire().await;
        assert!(limiter.check().is_ok());

        let _guard2 = sem.acquire().await;
        // Must wait for semaphore, even though governor allows 10/sec
    }
}
```

### Integration Tests

```rust
#[cfg(test)]
mod integration_tests {
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_global_semaphore_limits_concurrency() {
        let limiter = HttpApiRateLimiter::with_global_semaphore(2);
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(reqwest_ratelimit::all(limiter.clone()))
            .build();

        // Launch 5 concurrent requests
        let handles: Vec<_> = (0..5).map(|_| {
            let client = client.clone();
            tokio::spawn(async move {
                client.get("https://api.example.com/test").send().await
            })
        }).collect();

        // Only 2 should complete immediately
        sleep(Duration::from_millis(100)).await;

        let active = handles.iter().filter(|h| !h.is_finished()).count();
        assert!(active <= 2);
    }

    #[tokio::test]
    async fn test_per_origin_semaphore_isolation() {
        let limiter = HttpApiRateLimiter::default();
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(reqwest_ratelimit::all(limiter.clone()))
            .build();

        // Launch requests to different origins
        let h1 = tokio::spawn(client.get("https://github.com/test").send());
        let h2 = tokio::spawn(client.get("https://gitlab.com/test").send());
        let h3 = tokio::spawn(client.get("https://bitbucket.org/test").send());

        // Different origins have independent semaphores
        // (if per-origin limits are configured)
        sleep(Duration::from_millis(100)).await;

        let active = [&h1, &h2, &h3].iter()
            .filter(|h| !h.is_finished()).count();

        // Verify isolation
        assert_eq!(active, 3); // All should proceed if no per-origin limit
    }

    #[tokio::test]
    async fn test_semaphore_telemetry() {
        // Verify span attributes include concurrency metrics
        // Requires tracing subscriber setup
    }
}
```

### Stress Tests

```rust
#[tokio::test]
async fn test_semaphore_under_high_contention() {
    let sem = Arc::new(Semaphore::new(10));

    // Launch 100 concurrent tasks competing for 10 permits
    let handles: Vec<_> = (0..100).map(|i| {
        let sem = sem.clone();
        tokio::spawn(async move {
            let _guard = sem.acquire().await;
            sleep(Duration::from_millis(10)).await;
        })
    }).collect();

    // Verify no deadlock, all complete eventually
    for handle in handles {
        handle.await.unwrap();
    }
}
```

## Migration Guide

### For Library Users

**Version 0.2.0** introduces optional semaphores with no breaking changes.

```rust
// Old code (v0.1.0) - still works
let limiter = HttpApiRateLimiter::new(SmootherConfig::default());
// No semaphores, only rate limiting

// New code (v0.2.0) - opt-in to semaphores
let config = SmootherConfig {
    max_concurrent_global: Some(100),
    ..Default::default()
};
let limiter = HttpApiRateLimiter::new(config);
// Has both rate limiting AND concurrency limiting
```

### Configuration Migration

```yaml
# config.yaml or environment variables
# Old: only rate limiting settings
RQL_SMOOTH_VELOCITY=1.5
RQL_MICRO_INTERVAL_SECS=2

# New: add concurrency settings (optional)
RQL_MAX_CONCURRENT_GLOBAL=100
RQL_GITHUB_MAX_CONCURRENT=10  # Per-origin via policy
```

## Open Questions

1. **Default limits**: Should we provide sensible defaults if user doesn't configure?
   - Recommendation: No default (opt-in only), to avoid surprise behavior changes

2. **Per-origin storage**: Should semaphores be in `OriginRateLimiter` or `OriginRegistry`?
   - Recommendation: `OriginRateLimiter`, closer to governor state

3. **Telemetry naming**: Should concurrency metrics be separate span or attributes on existing span?
   - Recommendation: Attributes on existing HTTP span, simpler for tracing backends

4. **Error semantics**: What error when semaphore acquisition times out?
   - Recommendation: No timeout (queue indefinitely), let governor rate limiting handle backpressure

5. **Dynamic reconfiguration**: Should semaphores support runtime limit changes?
   - Recommendation: No (require limiter rebuild), matching policy update behavior

## Success Criteria

- [ ] Global semaphore configurable and working
- [ ] Per-origin semaphore configurable and working
- [ ] Semaphores optional (disabled by default)
- [ ] All existing tests pass (backward compatibility)
- [ ] New unit tests for semaphore behavior
- [ ] Integration tests for concurrency limiting
- [ ] Telemetry attributes added for concurrency
- [ ] Documentation updated with examples
- [ ] No performance regression (>5% overhead for semaphore operations)
- [ ] Stress tests pass (high contention scenarios)
