# Plugin Architecture for Semaphore/Concurrency Limiting

## Motivation

Rather than writing semaphore code directly into reqgov's core, a plugin architecture would:

- **Separate concerns**: Rate limiting (governor) vs concurrency limiting (semaphores)
- **Make it optional**: Feature flag enables/disables the plugin
- **Enable swapping**: Different implementations (tokio, async-std, custom)
- **Improve testability**: Test concurrency in isolation from rate limiting
- **Reduce core complexity**: Keep core codebase focused on IETF rate limits

## Hook Points Needed

### 1. Limiter Lifecycle Hooks

```rust
// When HttpApiRateLimiter is created
pub trait Plugin: Send + Sync + 'static {
    fn on_limiter_created(&self, config: &HttpApiRateLimiterConfig) -> Result<()>;
    
    fn on_limiter_destroyed(&self);
}
```

**Use case**: Create/destroy global and per-origin semaphores on limiter startup/shutdown.

### 2. Permit Acquisition Hooks

```rust
// Called before governor rate limit check
pub trait ConcurrencyPlugin: Plugin {
    fn on_pre_acquire(&self, origin: &str) -> Result<Option<AcquireGuard>>;

    // Called after governor rate limit check (if successful)
    fn on_post_acquire(&self, origin: &str, governor_success: bool) -> Result<()>;
}
```

**Use case**: Acquire semaphore before allowing request through rate limiter.

### 3. Request Lifecycle Hooks

```rust
// Called before HTTP request starts
pub trait RequestPlugin: Plugin {
    fn on_request_start(&self, origin: &str, request: &Request) -> Result<()>;

    // Called after HTTP request completes
    fn on_request_end(&self, origin: &str, response: &Response, duration: Duration) -> Result<()>;
}
```

**Use case**: Track concurrent request count for telemetry, measure request timing.

### 4. Per-Origin Lifecycle Hooks

```rust
// Called when OriginRateLimiter is created for a new origin
pub trait OriginPlugin: Plugin {
    fn on_origin_created(&self, origin: &str, config: &OriginConfig) -> Result<()>;

    fn on_origin_destroyed(&self, origin: &str);
}
```

**Use case**: Create/destroy per-origin semaphores, cleanup on origin removal.

### 5. Telemetry Hooks

```rust
// Called when span is created/enriched
pub trait TelemetryPlugin: Plugin {
    fn on_span_created(&self, span: &Span);

    fn on_span_enriched(&self, span: &Span, attributes: &[(String, Value)]);
}
```

**Use case**: Add concurrency metrics to tracing spans (pending, active, waiters).

### 6. Policy Update Hooks

```rust
// Called when Policy or ServiceLimit is updated
pub trait PolicyPlugin: Plugin {
    fn on_policy_updated(&self, policy: &Policy) -> Result<()>;

    fn on_limit_updated(&self, origin: &str, limit: &ServiceLimit) -> Result<()>;
}
```

**Use case**: Update per-origin semaphore limits when policies change (e.g., API documentation says "max 10 concurrent per domain").

## Proposed Plugin Trait

```rust
use std::any::Any;
use std::time::Duration;
use reqwest::{Request, Response};

pub trait ConcurrencyPlugin: Send + Sync + 'static {
    /// Plugin name for identification
    fn name(&self) -> &str;

    /// Called when HttpApiRateLimiter is initialized
    fn on_limiter_created(
        &self,
        config: &HttpApiRateLimiterConfig,
    ) -> Result<Box<dyn Any + Send>>;

    /// Called when origin limiter is created
    fn on_origin_created(
        &self,
        origin: &str,
        config: &OriginConfig,
    ) -> Result<()>;

    /// Called before permit acquisition (acquire semaphore here)
    fn on_pre_acquire(&self, origin: &str) -> Result<Option<AcquireGuard>>;

    /// Called after permit acquisition (if successful)
    fn on_post_acquire(&self, origin: &str) -> Result<()>;

    /// Called before permit release
    fn on_pre_release(&self, origin: &str, guard: &AcquireGuard) -> Result<()>;

    /// Called after permit release
    fn on_post_release(&self, origin: &str) -> Result<()>;

    /// Called when request starts
    fn on_request_start(
        &self,
        origin: &str,
        request: &Request,
    ) -> Result<()>;

    /// Called when request ends
    fn on_request_end(
        &self,
        origin: &str,
        response: &Response,
        duration: Duration,
    ) -> Result<()>;

    /// Called when plugin is unloaded
    fn on_shutdown(&self);
}

pub struct AcquireGuard {
    /// Guard returned from on_pre_acquire, must be passed to on_pre_release
    data: Box<dyn Any + Send>,
}
```

## Integration Points in reqgov

### HttpApiRateLimiter

```rust
pub struct HttpApiRateLimiter {
    registry: Arc<OriginRegistry>,
    plugins: Vec<Arc<dyn ConcurrencyPlugin>>,  // NEW

    // Wrap existing acquire_permit to call hooks
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<Permit> {
        for plugin in &self.plugins {
            let _ = plugin.on_pre_acquire(ctx.url().host_str().unwrap_or("default"))?;
        }

        // Call existing rate limiting logic
        let result = self.registry.acquire_permit(ctx).await;

        for plugin in &self.plugins {
            let _ = plugin.on_post_acquire(ctx.url().host_str().unwrap_or("default"))?;
        }

        result
    }

    pub fn register_plugin(&mut self, plugin: Arc<dyn ConcurrencyPlugin>) {
        self.plugins.push(plugin);
        plugin.on_limiter_created(&self.config).unwrap();
    }
}
```

### OriginRateLimiter

```rust
pub struct OriginRateLimiter {
    smoother: Smoother,
    slots: HashMap<String, PolicySlot>,
    plugins: Vec<Arc<dyn ConcurrencyPlugin>>,  // NEW

    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            smoother: Smoother::new(smoother_config),
            slots: HashMap::new(),
            fastest_policy: None,
            plugins: vec![],  // NEW
        }
    }

    // Wrap check() to call hooks
    pub fn check(&self) -> Result<(), RateLimitViolation> {
        for plugin in &self.plugins {
            let _ = plugin.on_pre_acquire("origin");
        }

        let result = self.governor_checks();

        for plugin in &self.plugins {
            let _ = plugin.on_post_acquire("origin");
        }

        result
    }
}
```

### Middleware Integration

```rust
// In RateLimitTelemetry or new ConcurrencyTelemetry middleware
#[async_trait::async_trait]
impl<S> Middleware for ConcurrencyTelemetryMiddleware<S> {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let origin = extract_origin(req);

        for plugin in &self.plugins {
            let _ = plugin.on_request_start(&origin, &req)?;
        }

        let start = Instant::now();
        let result = next.run(req, extensions).await;
        let duration = start.elapsed();

        match &result {
            Ok(response) => {
                for plugin in &self.plugins {
                    let _ = plugin.on_request_end(&origin, response, duration)?;
                }
            }
            result
        }
            Err(e) => result,
        }
    }
}
```

## Example Plugin: SemaphorePlugin

```rust
use tokio::sync::Semaphore;

pub struct SemaphorePlugin {
    global_semaphore: Option<Arc<Semaphore>>,
    origin_semaphores: HashMap<String, Arc<Semaphore>>,
    max_concurrent_global: Option<usize>,
    max_concurrent_per_origin: HashMap<String, usize>,
    metrics: ConcurrentMetrics,
}

struct ConcurrentMetrics {
    global_available: AtomicUsize,
    global_in_use: AtomicUsize,
    origin_available: HashMap<String, AtomicUsize>,
    origin_in_use: HashMap<String, AtomicUsize>,
}

impl ConcurrencyPlugin for SemaphorePlugin {
    fn name(&self) -> &str {
        "semaphore"
    }

    fn on_limiter_created(&self, config: &HttpApiRateLimiterConfig) -> Result<Box<dyn Any + Send>> {
        if let Some(max) = config.max_concurrent_global {
            self.global_semaphore = Some(Arc::new(Semaphore::new(max)));
            self.max_concurrent_global = Some(max);
        }
        Ok(Box::new(()))  // Return shared state for on_pre_acquire
    }

    fn on_origin_created(&self, origin: &str, config: &OriginConfig) -> Result<()> {
        if let Some(max) = config.max_concurrent {
            self.origin_semaphores.insert(origin.to_string(), Arc::new(Semaphore::new(max)));
            self.max_concurrent_per_origin.insert(origin.to_string(), max);
        }
        Ok(())
    }

    fn on_pre_acquire(&self, origin: &str) -> Result<Option<AcquireGuard>> {
        // Acquire global semaphore first
        if let Some(ref global_sem) = self.global_semaphore {
            let permit = global_sem.acquire_owned(1).await;
            self.metrics.global_in_use.fetch_add(1, Ordering::Relaxed);
            self.metrics.global_available.fetch_sub(1, Ordering::Relaxed);

            return Ok(Some(AcquireGuard {
                data: Box::new(permit),
            }));
        }

        // Acquire per-origin semaphore
        if let Some(ref origin_sem) = self.origin_semaphores.get(origin) {
            let permit = origin_sem.acquire_owned(1).await;
            self.origin_metrics(origin).in_use.fetch_add(1, Ordering::Relaxed);
            self.origin_metrics(origin).available.fetch_sub(1, Ordering::Relaxed);

            return Ok(Some(AcquireGuard {
                data: Box::new(permit),
            }));
        }

        Ok(None)  // No semaphore configured
    }

    fn on_pre_release(&self, origin: &str, guard: &AcquireGuard) -> Result<()> {
        // Release semaphore (happens when guard is dropped)
        // Implementation details in PLAN-semaphore-limit.md
        Ok(())
    }

    fn on_request_end(&self, origin: &str, _response: &Response, _duration: Duration) -> Result<()> {
        self.origin_metrics(origin).in_use.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }
}
```

## Alternative: Composition Over Inheritance

Instead of single `ConcurrencyPlugin` trait, prefer composition:

```rust
// Separate, focused traits
pub trait LifecyclePlugin {
    fn on_limiter_created(&self, config: &LimiterConfig) -> Result<()>;
    fn on_shutdown(&self);
}

pub trait AcquirePlugin {
    fn on_pre_acquire(&self, origin: &str) -> Result<Option<Guard>>;
    fn on_post_acquire(&self, origin: &str) -> Result<()>;
}

pub trait MetricsPlugin {
    fn on_record_metrics(&self, origin: &str, metrics: &Metrics) -> Result<()>;
}

// Plugins implement only what they need
struct SemaphorePlugin;
impl LifecyclePlugin for SemaphorePlugin { /* ... */ }
impl AcquirePlugin for SemaphorePlugin { /* ... */ }
impl MetricsPlugin for SemaphorePlugin { /* ... */ }
```

## Pros and Cons

### Plugin Architecture Pros

✅ **Separation of concerns**: Rate limiting core stays focused on IETF headers
✅ **Optional feature**: Users opt-in via config or feature flag
✅ **Testable**: Test semaphore plugin without governor complexity
✅ **Extensible**: Third-party can write their own concurrency strategies
✅ **Swapable**: Switch to async-std semaphores if tokio not used
✅ **Versioning**: Plugins can have their own version/release cycle
✅ **Reduced core complexity**: No if/else scattered throughout core code

### Plugin Architecture Cons

❌ **Indirection overhead**: Function call through trait for every request
❌ **Complex initialization**: Plugin discovery, dependency injection, lifecycle management
❌ **Debugging harder**: Call stack goes through trait dispatch
❌ **Feature flag complexity**: Enable/disable at compile time vs runtime
❌ **Tighter coupling**: Plugins need to know reqgov internals (OriginRateLimiter structure)

### Direct Implementation Pros

✅ **Simple**: Just add fields and methods to existing structs
✅ **Fast**: No trait dispatch overhead
✅ **Type-safe**: Compile-time errors if API changes
✅ **Easy to debug**: Direct code paths, clear call stacks

### Direct Implementation Cons

❌ **Coupling**: Semaphores tightly integrated into rate limiting code
❌ **Hard to disable**: Can't easily remove feature without breaking changes
❌ **Less flexible**: Can't swap implementations without code changes

## Recommendation

**For v0.2.0 (near term)**: Implement semaphores directly
- Feature is straightforward, no complex interaction points needed
- Smaller implementation surface, easier to get right
- Performance critical path, minimal overhead preferred

**For v0.3.0+ (future)**: Consider plugin architecture if:
- Multiple third-party extensions emerge
- Complex feature interactions require hooks
- Users request custom concurrency strategies
- Need to support multiple implementations simultaneously

**Hybrid approach** (best of both worlds):
- Implement semaphores directly in v0.2.0
- Expose hook points in key locations (lifecycle, acquire, release)
- Document hooks as "experimental" public API
- Gather feedback on hook usefulness before committing to plugin architecture

## Hooks Summary

| Hook Point | When Called | Use Case |
|------------|---------------|------------|
| `on_limiter_created` | HttpApiRateLimiter::new() | Initialize global semaphore |
| `on_limiter_destroyed` | HttpApiRateLimiter drop() | Cleanup global semaphore |
| `on_origin_created` | OriginRateLimiter::new() | Initialize per-origin semaphore |
| `on_origin_destroyed` | OriginRateLimiter drop() | Cleanup per-origin semaphore |
| `on_pre_acquire` | Before governor::check() | Acquire semaphore |
| `on_post_acquire` | After successful governor::check() | Record acquisition telemetry |
| `on_pre_release` | Before permit release | Prepare metrics for release |
| `on_post_release` | After permit release | Update concurrent counts |
| `on_request_start` | Before HTTP request | Track request start |
| `on_request_end` | After HTTP response | Update metrics, calculate duration |
| `on_shutdown` | Plugin unload | Cleanup resources |
