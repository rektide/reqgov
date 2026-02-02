# Chained Middleware Architecture for req-gov

## Current State

Currently, `OriginRegistry` and `ConcurrencyRateLimiter` both implement `reqwest_middleware::Middleware` and bundle multiple concerns:

**OriginRegistry responsibilities:**
1. Rate limiting logic (check/wait for policies and smoothing)
2. Policy/Limit header parsing
3. Limiter instance management (DashMap storage)
4. Extension injection (inserting limiters into request extensions)
5. Response processing (updating limiters from response headers)

**ConcurrencyRateLimiter responsibilities:**
1. Concurrency limiting (semaphore acquisition)
2. URL tracking
3. Extension injection

**Tracer middlewares (already separated):**
1. `PolicyTracer` - Traces policy slot states
2. `StatusTracer` - Traces rate limit check results
3. `SmootherTracer` - Traces smoother state
4. `ConcurrencyTracer` - Traces concurrency state

## Problem

The current monolithic middlewares mix:
- **Domain logic** (rate limiting, concurrency control)
- **State management** (limiter storage, header parsing)
- **Middleware orchestration** (extension injection, request/response handling)
- **Observability** (tracing, metrics)

This makes:
- Testing harder (need to mock multiple concerns)
- Configuration complex (all-or-nothing approach)
- Customization difficult (can't easily swap individual components)
- Reusability limited (can't use rate limiting without header parsing)

## Proposed Architecture: Chain of Responsibility

Split into single-purpose middlewares that can be chained independently:

### Layer 1: Limiter Management (No Middleware)

These components manage limiter instances but are NOT middleware:

```rust
// src/origin/manager.rs
pub struct OriginLimiterManager {
    origin_limiters: Arc<DashMap<String, Arc<OriginLimiter>>>,
    smoother_limiters: Arc<DashMap<String, Arc<SmootherLimiter>>>,
    smoother_config: Option<SmootherConfig>,
}

impl OriginLimiterManager {
    pub fn get_origin_limiter(&self, url: &Url) -> Arc<OriginLimiter> { /* ... */ }
    pub fn get_smoother_limiter(&self, url: &Url) -> Arc<SmootherLimiter> { /* ... */ }
    pub async fn update_from_response(&self, url: &Url, headers: &HeaderMap) { /* ... */ }
}
```

**Benefits:**
- Can be used independently without reqwest
- Testable in isolation
- Clear separation from middleware concerns

### Layer 2: Header Parsing Middleware

```rust
// src/middleware/header_parser.rs
pub struct HeaderParser {
    manager: Arc<OriginLimiterManager>,
}

#[async_trait::async_trait]
impl Middleware for HeaderParser {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url().clone();
        let response = next.run(req, extensions).await?;

        // Parse and update from response headers
        self.manager.update_from_response(&url, response.headers()).await;

        Ok(response)
    }
}
```

**Responsibilities:**
- Parse `ratelimit-policy` and `ratelimit` headers
- Update manager's limiters with parsed data
- No rate limiting logic, just state updates

### Layer 3: Rate Limiter Middleware

```rust
// src/middleware/rate_limiter.rs
pub struct OriginRateLimiter {
    manager: Arc<OriginLimiterManager>,
}

#[async_trait::async_trait]
impl Middleware for OriginRateLimiter {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url().clone();

        // Check limits
        let result = self.manager.wait(&url).await;
        extensions.insert(result);

        // Inject limiters for downstream middlewares
        extensions.insert(self.manager.get_origin_limiter(&url));
        extensions.insert(self.manager.get_smoother_limiter(&url));

        next.run(req, extensions).await
    }
}
```

**Responsibilities:**
- Check/wait rate limits
- Inject limiters into extensions
- Defer header parsing to HeaderParser

### Layer 4: Concurrency Middleware

```rust
// src/middleware/concurrency.rs
pub struct ConcurrencyLimiter {
    registry: Arc<ConcurrencyRegistry>,
}

#[async_trait::async_trait]
impl Middleware for ConcurrencyLimiter {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url().clone();

        // Acquire semaphores
        let global_semaphore = self.registry.get_global_semaphore();
        let domain_semaphore = self.registry.get_domain_semaphore(&url);

        let _global_permit = global_semaphore.acquire().await.unwrap();
        let _domain_permit = domain_semaphore.acquire().await.unwrap();

        extensions.insert(self.registry.clone());

        next.run(req, extensions).await
    }
}
```

**Responsibilities:**
- Acquire concurrency permits
- Inject registry into extensions
- No other logic

### Layer 5: Tracing Middlewares (Existing)

Already well-separated:
- `PolicyTracer` - traces policy slots
- `StatusTracer` - traces check results
- `SmootherTracer` - traces smoother state
- `ConcurrencyTracer` - traces concurrency

## Usage Patterns

### Basic Rate Limiting

```rust
use reqwest_middleware::{ClientBuilder, Middleware};
use reqgov::{OriginLimiterManager, OriginRateLimiter};

let manager = Arc::new(OriginLimiterManager::builder().build());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(OriginRateLimiter::new(Arc::clone(&manager)))
    .build();
```

### Rate Limiting + Header Parsing

```rust
let manager = Arc::new(OriginLimiterManager::builder().build());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(OriginRateLimiter::new(Arc::clone(&manager)))
    .with(HeaderParser::new(Arc::clone(&manager)))
    .build();
```

### Full Stack with Concurrency

```rust
let origin_manager = Arc::new(OriginLimiterManager::builder().build());
let concurrency_registry = Arc::new(ConcurrencyRegistry::builder()
    .max_concurrent_global(100)
    .max_concurrent_per_domain(10)
    .build());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(ConcurrencyLimiter::new(Arc::clone(&concurrency_registry)))
    .with(OriginRateLimiter::new(Arc::clone(&origin_manager)))
    .with(HeaderParser::new(Arc::clone(&origin_manager)))
    .with(PolicyTracer)
    .with(StatusTracer)
    .build();
```

### Custom Combinations

Skip header parsing if you configure limits programmatically:

```rust
let manager = Arc::new(OriginLimiterManager::builder().build());

// Configure limits directly
let url = Url::parse("https://api.example.com").unwrap();
let limiter = manager.get_origin_limiter(&url);
limiter.update_policies(policies).await;

let client = ClientBuilder::new(reqwest::Client::new())
    .with(OriginRateLimiter::new(Arc::clone(&manager)))
    // No HeaderParser needed
    .build();
```

## Benefits

### 1. Separation of Concerns
- Each middleware has ONE responsibility
- Easy to understand and reason about
- Clear boundaries between layers

### 2. Testability
```rust
#[tokio::test]
async fn test_rate_limiter() {
    let manager = Arc::new(OriginLimiterManager::new());
    let middleware = OriginRateLimiter::new(Arc::clone(&manager));
    // Test rate limiting logic alone
}

#[tokio::test]
async fn test_header_parser() {
    let manager = Arc::new(OriginLimiterManager::new());
    let middleware = HeaderParser::new(Arc::clone(&manager));
    // Test header parsing alone
}
```

### 3. Flexibility
- Use only what you need
- Order middlewares as needed
- Skip entire layers (e.g., no header parsing)

### 4. Composability
- Combine with custom middlewares
- Insert logging/metrics anywhere
- Easy to add new layers

### 5. Performance
- Skip unnecessary layers for cold paths
- Compile-time optimization per combination
- No monolithic lock contention

### 6. Debugging
- Clear call chain
- Easy to enable/disable individual layers
- Each layer has clear invariants

## Migration Strategy

### Phase 1: Extract Manager (Non-Breaking)
- Keep `OriginRegistry` as middleware
- Internally, use new `OriginLimiterManager`
- Public API unchanged

```rust
pub struct OriginRegistry {
    inner: Arc<OriginLimiterManager>,
}

impl OriginRegistry {
    pub fn builder() -> OriginRegistryBuilder { /* ... */ }
}

#[async_trait::async_trait]
impl Middleware for OriginRegistry {
    async fn handle(...) {
        // Delegate to inner
        self.inner.check_and_inject(&url, extensions).await;
        let response = next.run(req, extensions).await?;
        self.inner.update_from_response(&url, response.headers()).await;
        Ok(response)
    }
}
```

### Phase 2: Add Public Middlewares (Non-Breaking)
- Introduce `OriginRateLimiter`, `HeaderParser`
- Keep `OriginRegistry` for backwards compatibility
- Mark `OriginRegistry` as "legacy" in docs

```rust
// New public APIs
pub use middleware::{OriginRateLimiter, HeaderParser};

// Keep old API
pub use origin::OriginRegistry;
```

### Phase 3: Deprecate Monolithic (Breaking in v2.0)
- Deprecate `OriginRegistry::middleware`
- Encourage using separate middlewares
- Update examples

### Phase 4: Remove Legacy (v2.0)
- Remove `OriginRegistry::Middleware` impl
- Keep `OriginRegistry` as pure manager
- Clean up internals

## Naming Conventions

### Managers (No Middleware)
- `OriginLimiterManager` - manages rate limiters
- `ConcurrencyRegistry` - manages semaphores (already exists)

### Middlewares (Implement Middleware)
- `OriginRateLimiter` - enforces rate limits
- `HeaderParser` - parses response headers
- `ConcurrencyLimiter` - enforces concurrency limits

### Tracers (Implement Middleware)
- `PolicyTracer` - traces policies
- `StatusTracer` - traces check results
- etc.

## Alternative: Builder Pattern for Middleware Chains

Make it easy to compose common stacks:

```rust
pub struct MiddlewareChainBuilder {
    layers: Vec<Box<dyn Middleware>>,
}

impl MiddlewareChainBuilder {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub fn with_rate_limiting(mut self, manager: Arc<OriginLimiterManager>) -> Self {
        self.layers.push(Box::new(OriginRateLimiter::new(manager)));
        self
    }

    pub fn with_header_parsing(mut self, manager: Arc<OriginLimiterManager>) -> Self {
        self.layers.push(Box::new(HeaderParser::new(manager)));
        self
    }

    pub fn with_concurrency(mut self, registry: Arc<ConcurrencyRegistry>) -> Self {
        self.layers.push(Box::new(ConcurrencyLimiter::new(registry)));
        self
    }

    pub fn with_tracing(mut self) -> Self {
        self.layers.push(Box::new(PolicyTracer));
        self.layers.push(Box::new(StatusTracer));
        self
    }

    pub fn build(self) -> Vec<Box<dyn Middleware>> {
        self.layers
    }
}

// Usage:
let manager = Arc::new(OriginLimiterManager::builder().build());
let registry = Arc::new(ConcurrencyRegistry::builder()
    .max_concurrent_global(100)
    .build());

let middlewares = MiddlewareChainBuilder::new()
    .with_concurrency(registry)
    .with_rate_limiting(manager.clone())
    .with_header_parsing(manager.clone())
    .with_tracing()
    .build();

let client = ClientBuilder::new(reqwest::Client::new())
    .with_arc(middlewares)
    .build();
```

## Open Questions

1. **Should `OriginLimiterManager` be public?**
   - Pros: Users can use it without reqwest
   - Cons: More public API surface
   - Recommendation: Yes, but clearly document as "advanced usage"

2. **Should `HeaderParser` be optional?**
   - Some users configure limits programmatically
   - Recommendation: Yes, separate middleware

3. **How to handle shared state?**
   - All middlewares share the same `Arc<OriginLimiterManager>`
   - This is intentional and well-understood pattern
   - Recommendation: Keep as is

4. **Should we provide presets?**
   - E.g., `RateLimiting::github_style()`, `RateLimiting::basic()`
   - Recommendation: Start simple, add presets based on user feedback

5. **Performance impact of multiple middlewares?**
   - Each async call adds overhead
   - Recommendation: Profile after implementation, optimize hot paths if needed
