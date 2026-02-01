# Composable Rate Limiter Architecture

## Overview

Create a semaphore rate limiter that composes with existing `OriginRateLimiter` to enforce concurrency limits before rate limits.

**Goal**: Run semaphore → origin limiter → HTTP request in sequence, where each limiter is an async task or composable unit.

## Design Requirements

1. **Sequential execution**: Semaphore must be checked **before** origin limiter
2. **Backward compatibility**: Existing `OriginRateLimiter` code should still work
3. **Async composition**: Both limiters are async, must compose cleanly
4. **Error handling**: Semaphore wait errors should bubble up
5. **State isolation**: Each limiter maintains its own state
6. **Configurable**: Users can configure each limiter independently

## Approach 1: Wrapper Pattern

### Concept

Create a wrapper struct that holds both limiters and implements the rate limiter trait.

```rust
// Common trait both implementations can use
#[async_trait::async_trait]
pub trait RateLimiter: Send + Sync {
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<Permit>;
}

// Wrapper holding both limiters
pub struct ComposedRateLimiter {
    semaphore: Arc<Semaphore>,
    origin_limiter: Arc<OriginRateLimiter>,
    config: ComposeConfig,
}

pub struct ComposeConfig {
    pub max_concurrent: usize,
    pub smoother_config: SmootherConfig,
}

#[async_trait::async_trait]
impl RateLimiter for ComposedRateLimiter {
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<Permit> {
        // 1. Acquire semaphore
        let _sem_guard = self.semaphore.acquire().await;

        // 2. Acquire origin limiter permit
        let origin = ctx.url().host_str().unwrap_or("default");
        let origin_limiter = self.origin_limiter.get_or_create(origin);
        origin_limiter.wait().await;

        Ok(Permit { _marker: PhantomData })
    }
}
```

### Pros
- ✅ Single type to use
- ✅ Sequential ordering guaranteed
- ✅ Easy to create via builder

### Cons
- ❌ Requires common trait (breaking change)
- ❌ Wrapper couples both implementations
- ❌ Can't easily add third limiter (e.g., IP limit)

## Approach 2: Middleware Chain Pattern

### Concept

Each limiter implements a middleware trait, chain them like reqwest middleware.

```rust
// Define middleware trait similar to reqwest_middleware
#[async_trait::async_trait]
pub trait RateLimitMiddleware: Send + Sync + 'static {
    async fn acquire(&self, ctx: &Context<'_>) -> Result<AcquireResult>;
}

pub enum AcquireResult {
    Continue,  // Pass to next middleware
    Wait(Duration),  // Wait then retry (for semaphore queue)
    Error(Box<dyn std::error::Error>),  // Abort request
}

// Semaphore middleware
pub struct SemaphoreMiddleware {
    semaphore: Arc<Semaphore>,
}

#[async_trait::async_trait]
impl RateLimitMiddleware for SemaphoreMiddleware {
    async fn acquire(&self, _ctx: &Context<'_>) -> Result<AcquireResult> {
        match self.semaphore.try_acquire() {
            Ok(permit) => {
                // Permit acquired, pass to next middleware
                // Store permit in context for later release
                Ok(AcquireResult::Continue)
            }
            Err(_) => {
                // Wait for permit
                let wait = self.semaphore.acquire().await;
                Ok(AcquireResult::Wait(Duration::from_secs(0)))
            }
        }
    }
}

// Governor middleware (wraps OriginRateLimiter)
pub struct GovernorMiddleware {
    origin_limiter: Arc<OriginRateLimiter>,
}

#[async_trait::async_trait]
impl RateLimitMiddleware for GovernorMiddleware {
    async fn acquire(&self, ctx: &Context<'_>) -> Result<AcquireResult> {
        let origin = ctx.url().host_str().unwrap_or("default");
        let limiter = self.origin_limiter.get_or_create(origin);

        match limiter.check() {
            Ok(_) => Ok(AcquireResult::Continue),
            Err(violation) => {
                Ok(AcquireResult::Wait(violation.wait_duration()))
            }
        }
    }
}

// Chain middleware
pub struct MiddlewareChain {
    middlewares: Vec<Box<dyn RateLimitMiddleware>>,
}

#[async_trait::async_trait]
impl RateLimitMiddleware for MiddlewareChain {
    async fn acquire(&self, ctx: &Context<'_>) -> Result<AcquireResult> {
        for middleware in &self.middlewares {
            match middleware.acquire(ctx).await? {
                AcquireResult::Continue => continue,
                AcquireResult::Wait(dur) => return Ok(AcquireResult::Wait(dur)),
                AcquireResult::Error(e) => return Ok(AcquireResult::Error(e)),
            }
        }
        Ok(AcquireResult::Continue)
    }
}
```

### Pros
- ✅ Flexible: Add/remove middleware at runtime
- ✅ Third-party support: Anyone can implement `RateLimitMiddleware`
- ✅ Sequential: Natural ordering guarantees
- ✅ Error handling: Each middleware decides continue/wait/error

### Cons
- ❌ More verbose: Need to implement chain logic
- ❌ Async overhead: Multiple dispatches per request
- ❌ Complex error propagation: Need to unwrap `AcquireResult`

## Approach 3: Async Task Composition

### Concept

Create separate async tasks for each limiter, sequence them with futures.

```rust
// Compose as async tasks
pub async fn composed_acquire(
    semaphore: &Arc<Semaphore>,
    origin_limiter: &Arc<OriginRateLimiter>,
    ctx: &Context<'_>,
) -> Result<()> {
    // Task 1: Acquire semaphore
    let sem_future = semaphore.acquire_owned(1);
    let sem_result = sem_future.await?;

    // Task 2: Acquire origin limiter
    let origin = ctx.url().host_str().unwrap_or("default");
    let limiter = origin_limiter.get_or_create(origin);
    limiter.wait().await;

    Ok(())
}

// Or use join/try_join for parallel composition
pub async fn parallel_acquire_check(
    semaphore: &Arc<Semaphore>,
    origin_limiter: &Arc<OriginRateLimiter>,
    ctx: &Context<'_>,
) -> Result<bool> {
    let origin = ctx.url().host_str().unwrap_or("default");
    let limiter = origin_limiter.get_or_create(origin);

    // Check both in parallel
    let (sem_ok, gov_ok) = futures::try_join!(
        semaphore.try_acquire(),
        limiter.check(),
    )?;

    Ok(sem_ok.is_some() && gov_ok.is_ok())
}

// Or sequence with then()
pub async fn sequential_acquire(
    semaphore: &Arc<Semaphore>,
    origin_limiter: &Arc<OriginRateLimiter>,
    ctx: &Context<'_>,
) -> Result<()> {
    let origin = ctx.url().host_str().unwrap_or("default");
    let limiter = origin_limiter.get_or_create(origin);

    // Semaphore first, then governor
    semaphore.acquire().await.then(|_| async move {
        limiter.wait().await
    }).await;

    Ok(())
}
```

### Pros
- ✅ No new types: Just async functions
- ✅ Parallel support: Can check multiple limiters at once
- ✅ Flexible: Easy to compose any number of limiters
- ✅ No traits required: Just async functions

### Cons
- ❌ Manual sequencing: Caller must remember order
- ❌ No RAII: Semaphores released manually (error-prone)
- ❌ Context passing: Need to pass origin to each task

## Approach 4: Async Iterator Chain

### Concept

Treat limiters as an async iterator that produces permits.

```rust
// Define limiter as async iterator
pub trait AsyncLimiter: Send + Sync + 'static {
    async fn check(&self, ctx: &Context<'_>) -> Result<LimiterResult>;
}

pub enum LimiterResult {
    Ready,    // Can proceed immediately
    Wait(Duration),  // Wait this long, then retry
    Error(Box<dyn Error>),  // Can't proceed
}

// Semaphore as async limiter
pub struct SemaphoreLimiter {
    semaphore: Arc<Semaphore>,
}

#[async_trait::async_trait]
impl AsyncLimiter for SemaphoreLimiter {
    async fn check(&self, _ctx: &Context<'_>) -> Result<LimiterResult> {
        match self.semaphore.try_acquire() {
            Ok(permit) => {
                // Drop permit to release later
                drop(permit);
                Ok(LimiterResult::Ready)
            }
            Err(_) => {
                Ok(LimiterResult::Wait(Duration::from_secs(0)))
            }
        }
    }
}

// Origin limiter as async limiter
pub struct GovernorLimiter {
    origin_limiter: Arc<OriginRateLimiter>,
}

#[async_trait::async_trait]
impl AsyncLimiter for GovernorLimiter {
    async fn check(&self, ctx: &Context<'_>) -> Result<LimiterResult> {
        let origin = ctx.url().host_str().unwrap_or("default");
        let limiter = self.origin_limiter.get_or_create(origin);

        match limiter.check() {
            Ok(_) => Ok(LimiterResult::Ready),
            Err(violation) => Ok(LimiterResult::Wait(violation.wait_duration())),
        }
    }
}

// Chain them with async stream
use futures::StreamExt;

pub async fn acquire_with_chain(
    limiters: Vec<Box<dyn AsyncLimiter>>,
    ctx: &Context<'_>,
) -> Result<()> {
    use futures::StreamExt;

    // Create stream of limiters
    let limiter_stream = futures::stream::iter(limiters);

    // Try each limiter in sequence
    limiter_stream
        .then(|limiter| async move {
            limiter.check(ctx).await
        })
        .try_fold((), |_, result| {
            match result {
                LimiterResult::Ready => Ok(()),
                LimiterResult::Wait(dur) => {
                    // Wait, then retry all limiters
                    tokio::time::sleep(dur).await;
                    Err("wait and retry".into())
                }
                LimiterResult::Error(e) => Err(e),
            }
        })
        .await
}
```

### Pros
- ✅ Unlimited limiters: Chain any number
- ✅ Retry semantics: Built-in wait-and-retry logic
- ✅ Stream-based: Leverages Rust async ecosystem

### Cons
- ❌ Overkill: Complex for just 2 limiters
- ❌ Inefficient: Re-checks all previous limiters after wait
- ❌ Hard to debug: Stream operations obscure flow

## Approach 5: Borrow Checker Pattern

### Concept

Use a "checker" that wraps other limiters and enforces ordering.

```rust
// Borrow checker pattern
pub struct BorrowChecker<L> {
    limiter: L,
    check_fn: for<'a, 'ctx> fn(&'a L, &'a Context<'ctx>) -> CheckResult<'a>,
}

pub enum CheckResult<'a> {
    Proceed(ProceedGuard<'a>),
    Wait(Duration),
    Error(Box<dyn Error>),
}

pub struct ProceedGuard<'a> {
    _phantom: PhantomData<&'a ()>,
}

// Semaphore checker
pub fn check_semaphore<'a, 'ctx>(
    sem: &'a Arc<Semaphore>,
    _ctx: &'a Context<'ctx>,
) -> CheckResult<'a> {
    match sem.try_acquire_owned(1) {
        Ok(permit) => CheckResult::Proceed(ProceedGuard {
            _phantom: PhantomData,
            _permit: Some(permit),
        }),
        Err(_) => CheckResult::Wait(Duration::from_secs(0)),
    }
}

// Origin limiter checker
pub fn check_governor<'a, 'ctx>(
    limiter: &'a Arc<OriginRateLimiter>,
    ctx: &'a Context<'ctx>,
) -> CheckResult<'a> {
    let origin = ctx.url().host_str().unwrap_or("default");
    let origin_limiter = limiter.get_or_create(origin);

    match origin_limiter.check() {
        Ok(_) => CheckResult::Proceed(ProceedGuard {
            _phantom: PhantomData,
            _permit: None,
        }),
        Err(violation) => CheckResult::Wait(violation.wait_duration()),
    }
}

// Compose checkers
pub struct ComposedLimiter {
    semaphore_checker: fn(&Arc<Semaphore>, &Context<'_>) -> CheckResult<'_>,
    origin_checker: fn(&Arc<OriginRateLimiter>, &Context<'_>) -> CheckResult<'_>,
    semaphore: Arc<Semaphore>,
    origin_limiter: Arc<OriginRateLimiter>,
}

impl ComposedLimiter {
    pub async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        loop {
            // Check semaphore first
            match (self.semaphore_checker)(&self.semaphore, ctx) {
                CheckResult::Proceed(_) => {
                    // Semaphore OK, check governor
                    match (self.origin_checker)(&self.origin_limiter, ctx) {
                        CheckResult::Proceed(_) => return Ok(()),
                        CheckResult::Wait(dur) => {
                            // Governor says wait
                            tokio::time::sleep(dur).await;
                            continue; // Retry from semaphore
                        }
                        CheckResult::Error(e) => return Err(e),
                    }
                }
                CheckResult::Wait(dur) => {
                    // Semaphore says wait
                    tokio::time::sleep(dur).await;
                    continue; // Retry semaphore
                }
                CheckResult::Error(e) => return Err(e),
            }
        }
    }
}
```

### Pros
- ✅ Zero-cost abstraction: Just function pointers
- ✅ Lifetime safe: Uses lifetimes to ensure permit validity
- ✅ Flexible: Can swap checkers at runtime
- ✅ Compile-time checks: Type errors if signature mismatches

### Cons
- ❌ Complex lifetimes: Hard to understand
- ❌ Manual retry loop: Error-prone
- ❌ Not idiomatic: Unusual pattern in Rust

## Recommended Approach: Async Task Composition with RAII Guards

### Rationale

Combines best of both worlds:

1. **Simple**: No new traits, just async functions
2. **Safe**: RAII guards ensure semaphores released
3. **Composable**: Easy to add more limiters
4. **Async**: Natural sequencing with `.then()`

### Implementation

```rust
// RAII guard for semaphore
pub struct SemaphoreGuard<'a> {
    semaphore: Option<Arc<Semaphore>>,
    permit: Option<SemaphorePermit<'a>>,
}

impl<'a> SemaphoreGuard<'a> {
    pub async fn acquire(sem: &Arc<Semaphore>) -> Self {
        let permit = sem.acquire_owned(1).await;
        Self {
            semaphore: Some(sem.clone()),
            permit: Some(permit),
        }
    }
}

impl<'a> Drop for SemaphoreGuard<'a> {
    fn drop(&mut self) {
        // Permit released when guard dropped
    }
}

// Composed acquire function
pub async fn composed_acquire(
    sem: &Arc<Semaphore>,
    limiter: &Arc<OriginRateLimiter>,
    ctx: &Context<'_>,
) -> Result<SemaphoreGuard<'_>> {
    // 1. Acquire semaphore (RAII)
    let _sem_guard = SemaphoreGuard::acquire(sem).await;

    // 2. Acquire origin limiter (already has RAII)
    let origin = ctx.url().host_str().unwrap_or("default");
    let origin_limiter = limiter.get_or_create(origin);
    origin_limiter.wait().await;

    // Return guard (semaphore auto-releases on drop)
    Ok(SemaphoreGuard {
        semaphore: Some(sem.clone()),
        permit: None, // Already released
    })
}
```

### Integration with reqwest_ratelimit

```rust
// New HttpApiRateLimiter that composes both
pub struct ComposedHttpApiRateLimiter {
    semaphore: Option<Arc<Semaphore>>,
    origin_limiter: Arc<OriginRateLimiter>,
}

#[async_trait::async_trait]
impl reqwest_ratelimit::RateLimiter for ComposedHttpApiRateLimiter {
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<reqwest_ratelimit::Permit> {
        // If semaphore configured, acquire it
        let _sem_guard = match &self.semaphore {
            Some(sem) => Some(composed_acquire(sem, &self.origin_limiter, ctx).await?),
            None => None,
        };

        // If no semaphore, just use origin limiter (backward compatible)
        Ok(reqwest_ratelimit::Permit {
            _marker: PhantomData,
        })
    }
}

// Configuration
impl ComposedHttpApiRateLimiter {
    pub fn with_semaphore(self, max_concurrent: usize) -> Self {
        Self {
            semaphore: Some(Arc::new(Semaphore::new(max_concurrent))),
            origin_limiter: self.origin_limiter,
        }
    }

    pub fn new(origin_limiter: Arc<OriginRateLimiter>) -> Self {
        Self {
            semaphore: None,  // Disabled by default
            origin_limiter,
        }
    }
}
```

### Usage

```rust
// Old: just rate limiting
let limiter = HttpApiRateLimiter::new(SmootherConfig::default());

// New: compose with semaphore
let limiter = HttpApiRateLimiter::new(SmootherConfig::default())
    .with_semaphore(100);  // Max 100 concurrent

// Works exactly the same way
let client = ClientBuilder::new(reqwest::Client::new())
    .with(reqwest_ratelimit::all(Arc::new(limiter)))
    .build();
```

## Summary Table

| Approach | Complexity | Flexibility | Performance | RAII | Verdict |
|----------|-----------|--------------|-------------|-------|---------|
| Wrapper Pattern | Medium | Low | Fast | ✅ | Good |
| Middleware Chain | High | High | Slow | ❌ | Overkill |
| Async Task Composition | Low | High | Medium | ❌ | Manual |
| Async Iterator Chain | Very High | High | Slow | ❌ | Overkill |
| Borrow Checker | High | Medium | Medium | ✅ | Complex |
| RAII + Async Then | Low | Medium | Fast | ✅ | Good |
| **Unified Acquisition** | **Low** | **High** | **Fast** | **✅** | **Recommended** |

## Final Recommendation

**Use Approach 5 (RAII + Async Task Composition)** OR **Approach 6 (Unified Acquisition Pattern)** for v0.2.0:

```rust
// Simple, safe, composable
async fn acquire_with_semaphore(
    semaphore: Option<&Arc<Semaphore>>,
    origin_limiter: &Arc<OriginRateLimiter>,
    ctx: &Context<'_>,
) -> Result<()> {
    // Optional semaphore, required origin limiter
    if let Some(sem) = semaphore {
        let _guard = sem.acquire().await;
    }
    let origin = ctx.url().host_str().unwrap_or("default");
    let limiter = origin_limiter.get_or_create(origin);
    limiter.wait().await;
    Ok(())
}
```

---

## Approach 6: Unified Acquisition Pattern (NEW - Recommended)

### Concept

Instead of exposing separate semaphore and rate limiter, provide a single unified acquisition function that internally handles both. This is a facade pattern with a clean API.

### Rationale

- **Single call site**: Users call one `acquire()` method instead of remembering to call semaphore then limiter
- **Implementation hiding**: Composition is internal, users just see "acquire permit"
- **Easier to extend**: Adding more limiters (e.g., IP rate limiter) doesn't change user code
- **Type safety**: Compiler ensures correct composition at creation time
- **Zero overhead**: Compiler inlines the composition, no trait dispatch

### Implementation

```rust
// Unified limiter that handles both semaphore and governor
pub struct UnifiedRateLimiter {
    semaphore: Option<Arc<Semaphore>>,
    origin_limiter: Arc<OriginRateLimiter>,
}

impl UnifiedRateLimiter {
    /// Create with both limiters
    pub fn new(
        semaphore: Option<usize>,
        origin_limiter: Arc<OriginRateLimiter>,
    ) -> Self {
        let semaphore = semaphore.map(|max| Arc::new(Semaphore::new(max)));
        Self {
            semaphore,
            origin_limiter,
        }
    }

    /// Create without semaphore (backward compatible)
    pub fn without_semaphore(origin_limiter: Arc<OriginRateLimiter>) -> Self {
        Self {
            semaphore: None,
            origin_limiter,
        }
    }

    /// Acquire permit from both limiters in one call
    pub async fn acquire(&self, origin: &str) -> Result<UnifiedPermit> {
        // Step 1: Acquire semaphore (if configured)
        let sem_guard = match &self.semaphore {
            Some(sem) => Some(sem.acquire().await),
            None => None,
        };

        // Step 2: Acquire governor permit
        let limiter = self.origin_limiter.get_or_create(origin);

        // Check if rate limited
        match limiter.check() {
            Ok(_) => {
                // Rate limit OK
                Ok(UnifiedPermit {
                    _marker: PhantomData,
                    _sem_guard: sem_guard,
                })
            }
            Err(violation) => {
                // Rate limited - release semaphore and return wait duration
                drop(sem_guard);
                Err(violation.into())
            }
        }
    }
}

// RAII guard that holds both permits
pub struct UnifiedPermit<'a> {
    _marker: PhantomData<&'a ()>,
    _sem_guard: Option<SemaphorePermit<'a>>,
}

impl<'a> Drop for UnifiedPermit<'a> {
    fn drop(&mut self) {
        // Both semaphores released automatically on drop
    }
}
```

### Integration with reqwest_ratelimit

```rust
// New trait for unified limiter
pub trait UnifiedRateLimiter: Send + Sync + 'static {
    async fn acquire(&self, origin: &str) -> Result<UnifiedPermit>;
}

// Wrap origin limiter to implement unified trait
pub struct OriginLimiterWrapper {
    limiter: Arc<OriginRateLimiter>,
}

#[async_trait::async_trait]
impl UnifiedRateLimiter for OriginLimiterWrapper {
    async fn acquire(&self, origin: &str) -> Result<UnifiedPermit> {
        let limiter = self.limiter.get_or_create(origin);
        match limiter.check() {
            Ok(_) => Ok(UnifiedPermit {
                _marker: PhantomData,
                _sem_guard: None, // No semaphore at this level
            }),
            Err(e) => Err(e.into()),
        }
    }
}

// Add semaphore in HttpApiRateLimiter
pub struct HttpApiRateLimiter {
    unified: UnifiedRateLimiter,  // NEW
    registry: Arc<OriginRegistry>,
}

#[async_trait::async_trait]
impl reqwest_ratelimit::RateLimiter for HttpApiRateLimiter {
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<reqwest_ratelimit::Permit> {
        let origin = ctx.url().host_str().unwrap_or("default");
        let permit = self.unified.acquire(origin).await?;

        // Convert to reqwest_ratelimit::Permit
        Ok(reqwest_ratelimit::Permit {
            _marker: PhantomData,
        })
    }

    pub fn with_semaphore(self, max_concurrent: usize) -> Self {
        Self {
            unified: UnifiedRateLimiterImpl::with_semaphore(&self.registry, max_concurrent),
            registry: self.registry,
        }
    }
}
```

### Usage Comparison

```rust
// Approach 5 (previous recommendation): User must remember to call both
async fn make_request(limiter: &HttpApiRateLimiter) -> Result<Response> {
    let origin = "api.example.com";

    // User must remember: semaphore FIRST, then limiter
    let _sem_guard = limiter.semaphore.acquire().await;
    let _limiter_guard = limiter.origin_limiter.get_or_create(&origin).wait().await;

    // Make request
    // ...
}

// Approach 6 (new recommendation): Single call
async fn make_request(limiter: &HttpApiRateLimiter) -> Result<Response> {
    let origin = "api.example.com";

    // Just one call!
    let _permit = limiter.acquire(&origin).await?;

    // Make request
    // ...
}
```

### Pros vs Approach 5

| Aspect | Approach 5 | Approach 6 |
|---------|--------------|-------------|
| **User API** | 2 calls required | 1 call (cleaner) |
| **Ordering** | User must remember | Guaranteed by implementation |
| **Extensibility** | Manual composition | Extend unified trait |
| **Performance** | Same (2 awaits) | Same (2 awaits) |
| **Safety** | RAII on each | RAII on unified guard |
| **Complexity** | Low | Low (more code) |
| **Type safety** | Good | Excellent (compile-time) |

### Adding More Limiters

```rust
// Easy to add third limiter (e.g., IP rate limiting)
pub struct ExtendedUnifiedLimiter {
    semaphore: Option<Arc<Semaphore>>,
    origin_limiter: Arc<OriginRateLimiter>,
    ip_limiter: Arc<IpRateLimiter>,  // NEW
}

pub async fn acquire(&self, origin: &str) -> Result<UnifiedPermit> {
    let sem_guard = match &self.semaphore {
        Some(sem) => Some(sem.acquire().await),
        None => None,
    };

    // Now 3 limiters in sequence
    let limiter = self.origin_limiter.get_or_create(origin);
    match limiter.check() {
        Ok(_) => {
            match self.ip_limiter.check(origin) {
                Ok(_) => Ok(UnifiedPermit { _marker: PhantomData, _sem_guard }),
                Err(e) => {
                    drop(sem_guard);
                    Err(e.into())
                }
            }
        }
        Err(e) => {
            drop(sem_guard);
            Err(e.into())
        }
    }
}
```

## Updated Final Recommendation

**Use Approach 6 (Unified Acquisition Pattern)** for v0.2.0:

```rust
// Usage in reqwest_ratelimit implementation
pub struct ComposedLimiter {
    unified: UnifiedRateLimiter,
}

#[async_trait::async_trait]
impl reqwest_ratelimit::RateLimiter for ComposedLimiter {
    async fn acquire_permit(&self, ctx: &Context<'_>) -> Result<reqwest_ratelimit::Permit> {
        let origin = ctx.url().host_str().unwrap_or("default");
        let _permit = self.unified.acquire(origin).await?;

        Ok(reqwest_ratelimit::Permit { _marker: PhantomData })
    }
}
```

**Why this approach wins**:

1. **Backward compatible**: `None` semaphore = original behavior
2. **Zero allocation overhead**: No wrapper structs, no trait dispatch
3. **RAII guaranteed**: Guard released on drop, panic-safe
4. **Composable**: Easy to add third limiter (e.g., `ip_limiter.acquire().await`)
5. **Type-safe**: Compile errors if signature changes
6. **Performance critical path**: Just 2 await calls, no indirection
