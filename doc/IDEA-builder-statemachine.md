# State Machine Style & Builder Patterns for Limiters

## 1. State Machine Style Explained

State machine style replaces `Result<(), RateLimitViolation>` with an enum representing the limiter's state:

```rust
pub enum LimiterState {
    Allowed,
    Exceeded {
        policy_name: String,
        wait_until: Instant,
    },
}

#[async_trait::async_trait]
pub trait Limiter: Send + Sync {
    async fn check(&self) -> LimiterState;
    async fn wait(&self) -> Duration;
}
```

**Why it's useful:**
- More expressive - captures all possible states explicitly
- No error types in trait - allows custom error handling
- Can add states like `Backoff { duration: Duration }` or `CircuitOpen`
- Better for telemetry/logging

**Example usage:**
```rust
match limiter.check().await {
    LimiterState::Allowed => {
        // proceed with request
    }
    LimiterState::Exceeded { policy_name, wait_until } => {
        tracing::warn!("Rate limit exceeded by policy {}", policy_name);
        tokio::time::sleep_until(wait_until).await;
    }
}
```

**Tradeoffs:**
- Pros: Flexible, extensible, no error type constraints
- Cons: Loses `?` operator ergonomics, more verbose error handling

---

## 2. Builder Patterns for Multiple Limiters

### Option A: Composite Limiter Builder

Builds multiple limiters together in one structure:

```rust
pub struct CompositeLimiterBuilder {
    origin_limiters: Vec<Arc<OriginLimiter>>,
    smoother_limiters: Vec<Arc<SmootherLimiter>>,
    concurrency_limiters: Vec<Arc<ConcurrencyRateLimiter>>,
}

impl CompositeLimiterBuilder {
    pub fn new() -> Self {
        Self {
            origin_limiters: Vec::new(),
            smoother_limiters: Vec::new(),
            concurrency_limiters: Vec::new(),
        }
    }

    pub fn add_origin_limiter(mut self, limiter: Arc<OriginLimiter>) -> Self {
        self.origin_limiters.push(limiter);
        self
    }

    pub fn add_smoother_limiter(mut self, limiter: Arc<SmootherLimiter>) -> Self {
        self.smoother_limiters.push(limiter);
        self
    }

    pub fn build(self) -> CompositeLimiter {
        CompositeLimiter {
            origin_limiters: self.origin_limiters,
            smoother_limiters: self.smoother_limiters,
            concurrency_limiters: self.concurrency_limiters,
        }
    }
}

#[async_trait::async_trait]
impl Limiter for CompositeLimiter {
    async fn check(&self) -> Result<(), RateLimitViolation> {
        // Check all limiters, return first error
        for limiter in &self.origin_limiters {
            limiter.check().await?;
        }
        for limiter in &self.smoother_limiters {
            limiter.check().await?;
        }
        Ok(())
    }

    async fn wait(&self) -> Duration {
        let mut total = Duration::ZERO;
        for limiter in &self.origin_limiters {
            total += limiter.wait().await;
        }
        for limiter in &self.smoother_limiters {
            total += limiter.wait().await;
        }
        total
    }

    // ... other methods
}
```

**Usage:**
```rust
let limiter = CompositeLimiterBuilder::new()
    .add_origin_limiter(Arc::new(OriginLimiter::builder().build()))
    .add_smoother_limiter(Arc::new(SmootherLimiter::builder().build()))
    .build();
```

### Option B: Generic Limiter Builder

Builder that accepts any Limiter implementation:

```rust
pub struct LimiterBuilder<L> {
    phantom: std::marker::PhantomData<L>,
    // builder-specific config
}

impl<L> LimiterBuilder<L>
where
    L: Limiter + Send + Sync,
{
    pub fn build(&self) -> L {
        // delegate to L::builder() or similar
        todo!()
    }
}
```

**Usage:**
```rust
let origin: OriginLimiter = LimiterBuilder::new().build();
let smoother: SmootherLimiter = LimiterBuilder::new().build();
```

### Option C: Chained Limiter (Decorator Pattern)

Chain limiters together, each wrapping the previous:

```rust
pub struct ChainedLimiter {
    inner: Box<dyn Limiter>,
    outer: Box<dyn Limiter>,
}

#[async_trait::async_trait]
impl Limiter for ChainedLimiter {
    async fn check(&self) -> Result<(), RateLimitViolation> {
        // Check inner first, then outer
        self.inner.check().await?;
        self.outer.check().await
    }
    // ...
}

// Helper function
pub fn chain(inner: Box<dyn Limiter>, outer: Box<dyn Limiter>) -> Box<dyn Limiter> {
    Box::new(ChainedLimiter { inner, outer })
}
```

**Usage:**
```rust
let limiter = chain(
    Box::new(OriginLimiter::new()),
    Box::new(SmootherLimiter::new()),
);
```

---

## 3. Generic Builder Options

### Option A: Typestate Pattern (Compile-time safety)

```rust
pub struct OriginLimiterBuilder<Policies = (), Limits = ()> {
    policies: Policies,
    limits: Limits,
}

impl OriginLimiterBuilder {
    pub fn new() -> OriginLimiterBuilder<(), ()> {
        OriginLimiterBuilder {
            policies: (),
            limits: (),
        }
    }
}

impl OriginLimiterBuilder<(), ()> {
    pub fn with_policies(self, policies: Vec<Policy>) -> OriginLimiterBuilder<Vec<Policy>, ()> {
        OriginLimiterBuilder {
            policies,
            limits: (),
        }
    }
}

impl<Policies> OriginLimiterBuilder<Policies, ()> {
    pub fn with_limits(self, limits: Vec<ServiceLimit>) -> OriginLimiterBuilder<Policies, Vec<ServiceLimit>> {
        OriginLimiterBuilder {
            policies: self.policies,
            limits,
        }
    }
}

impl OriginLimiterBuilder<Vec<Policy>, Vec<ServiceLimit>> {
    pub fn build(self) -> OriginLimiter {
        let limiter = OriginLimiter::new();
        // ... configure with policies and limits
        limiter
    }
}
```

**Pros:**
- Compile-time enforcement of required steps
- Clear, guided API

**Cons:**
- Complex generics
- Boilerplate heavy

### Option B: Using `bon` crate (already in dependencies)

```rust
use bon::bon;

#[bon]
impl OriginLimiter {
    pub fn builder() -> OriginLimiterBuilder {
        OriginLimiterBuilder {
            policies: None,
            limits: None,
        }
    }
}

#[derive(Default)]
pub struct OriginLimiterBuilder {
    policies: Option<Vec<Policy>>,
    limits: Option<Vec<ServiceLimit>>,
}

#[bon]
impl OriginLimiterBuilder {
    #[builder]
    pub fn build(self) -> OriginLimiter {
        let limiter = OriginLimiter::new();
        // configure
        limiter
    }
}
```

**Usage:**
```rust
let limiter = OriginLimiter::builder()
    .policies(vec![...])
    .limits(vec![...])
    .build();
```

### Option C: Builder Trait

```rust
pub trait LimiterBuilder {
    type Limiter;

    fn build(self) -> Self::Limiter;
}

impl LimiterBuilder for OriginLimiterBuilder {
    type Limiter = OriginLimiter;

    fn build(self) -> OriginLimiter {
        // existing implementation
    }
}
```

**Pros:**
- Generic over builder types
- Can have generic functions accepting any builder

**Cons:**
- Adds abstraction layer
- Associated types can be complex

---

## 4. Clone Costs & `clone_box()` Frequency

### Current Clone Behavior (Arc-based sharing)

```rust
impl Clone for OriginLimiter {
    fn clone(&self) -> Self {
        Self {
            slots: Arc::clone(&self.slots),  // CHEAP - just increments ref count
        }
    }
}
```

**What happens:**
- All clones share the SAME `Arc<Mutex<Vec<PolicySlot>>>`
- Changes in one clone are visible in all clones
- This is SHARING, not duplication
- Cost: O(1) - just an atomic increment

### `clone_box()` for Trait Objects

```rust
fn clone_box(&self) -> Box<dyn Limiter> where Self: Sized + Clone {
    Box::new(self.clone())  // Calls Arc::clone - shares state
}
```

**When `clone_box()` would be called:**
1. Storing limiters in `Vec<Box<dyn Limiter>>`
2. Passing trait objects across threads
3. When you need trait object erasure

**Frequency analysis:**
- Low to medium frequency
- Typically called once during setup/configuration
- NOT called per-request (that would be disastrous)

### The Problem: Governor is NOT Clone!

```rust
pub struct PolicySlot {
    policy: Policy,
    remaining: u32,
    reset_at: Option<Instant>,

    // This is NOT Clone:
    governor: RateLimiter<governor::state::NotKeyed, InMemoryState, DefaultClock, StateInformationMiddleware>,

    last_snapshot: Mutex<Option<StateSnapshot>>,
    governor_quota: (u32, u32),
}
```

**Implications:**

1. **You cannot duplicate a PolicySlot:**
   ```rust
   let slot1 = PolicySlot::new(policy);
   let slot2 = slot1.clone();  // COMPILER ERROR: PolicySlot is not Clone
   ```

2. **Arc-based cloning is the ONLY cheap option:**
   - Sharing the same slot across multiple clones
   - All clones see the same governor state
   - This is INTENTIONAL - shared rate limiting state

3. **`clone_box()` preserves sharing semantics:**
   ```rust
   let limiter1 = OriginLimiter::new();
   limiter1.update_policies(policies).await;

   let limiter2 = limiter1.clone();  // Shares the same slots
   limiter2.check().await;  // Affects the same governor as limiter1

   let boxed: Box<dyn Limiter> = Box::new(limiter1);
   let cloned_boxed = boxed.clone_box();  // STILL shares the same slots!
   ```

### When Would You Need Actual Duplication?

If you need INDEPENDENT limiters (not shared), you cannot use Clone:

```rust
// WRONG - this shares state, doesn't duplicate:
let limiter1 = OriginLimiter::new();
limiter1.update_policies(policies).await;
let limiter2 = limiter1.clone();  // Shares slots!

// CORRECT - create independent limiters:
let limiter1 = OriginLimiter::new();
limiter1.update_policies(policies.clone()).await;

let limiter2 = OriginLimiter::new();
limiter2.update_policies(policies.clone()).await;  // Independent slots
```

### Solution: Remove `Clone` from trait, use `Arc<dyn Limiter>`

```rust
#[async_trait::async_trait]
pub trait Limiter: Send + Sync {
    async fn check(&self) -> Result<(), RateLimitViolation>;
    async fn wait(&self) -> Duration;
    async fn update_policies(&self, policies: Vec<Policy>);
    async fn update_limits(&self, limits: Vec<ServiceLimit>);
}

// Store as Arc instead of Box:
let limiters: Vec<Arc<dyn Limiter>> = vec![
    Arc::new(OriginLimiter::new()),
    Arc::new(SmootherLimiter::new()),
];

// Clone is cheap (Arc clone):
let cloned = Arc::clone(&limiters[0]);
```

**Why this is better:**
- Arc cloning is always O(1)
- Makes sharing explicit
- No need for `clone_box()` workaround
- Governors remain non-cloneable, but that's fine
- Clear semantics: Arc = shared, Box = owned (but can't clone)

---

## 5. Recommended Approach

### Use `Arc<dyn Limiter>` instead of trait with Clone

```rust
#[async_trait::async_trait]
pub trait Limiter: Send + Sync {
    async fn check(&self) -> Result<(), RateLimitViolation>;
    async fn wait(&self) -> Duration;
    async fn update_policies(&self, policies: Vec<Policy>);
    async fn update_limits(&self, limits: Vec<ServiceLimit>);
}

impl Limiter for OriginLimiter { /* ... */ }
impl Limiter for SmootherLimiter { /* ... */ }

// Store in Arc:
let registry: Arc<dyn Limiter> = Arc::new(OriginLimiter::new());

// Clone is cheap:
let cloned = Arc::clone(&registry);  // O(1) - shares state

// Pass around:
async fn use_limiter(limiter: Arc<dyn Limiter>) {
    limiter.check().await?;
    // ...
}
```

### For composition, use a Composite Limiter:

```rust
pub struct CompositeLimiter {
    limiters: Vec<Arc<dyn Limiter>>,
}

#[async_trait::async_trait]
impl Limiter for CompositeLimiter {
    async fn check(&self) -> Result<(), RateLimitViolation> {
        for limiter in &self.limiters {
            limiter.check().await?;
        }
        Ok(())
    }

    async fn wait(&self) -> Duration {
        let mut total = Duration::ZERO;
        for limiter in &self.limiters {
            total += limiter.wait().await;
        }
        total
    }

    async fn update_policies(&self, policies: Vec<Policy>) {
        for limiter in &self.limiters {
            limiter.update_policies(policies.clone()).await;
        }
    }

    async fn update_limits(&self, limits: Vec<ServiceLimit>) {
        for limiter in &self.limiters {
            limiter.update_limits(limits.clone()).await;
        }
    }
}

impl CompositeLimiter {
    pub fn builder() -> CompositeLimiterBuilder {
        CompositeLimiterBuilder::new()
    }
}

pub struct CompositeLimiterBuilder {
    limiters: Vec<Arc<dyn Limiter>>,
}

impl CompositeLimiterBuilder {
    pub fn new() -> Self {
        Self { limiters: Vec::new() }
    }

    pub fn add_limiter(mut self, limiter: Arc<dyn Limiter>) -> Self {
        self.limiters.push(limiter);
        self
    }

    pub fn build(self) -> CompositeLimiter {
        CompositeLimiter { limiters: self.limiters }
    }
}
```

**Usage:**
```rust
let composite = CompositeLimiter::builder()
    .add_limiter(Arc::new(OriginLimiter::new()))
    .add_limiter(Arc::new(SmootherLimiter::new()))
    .build();

composite.check().await?;
```

### Summary

1. **State machine style**: Enum-based state representation instead of Result
2. **Builder patterns**: Composite builder for combining limiters, generic builder for type flexibility
3. **Generic builders**: Typestate for compile-time safety, `bon` crate for ergonomic builders
4. **Clone costs**: Current Arc-based cloning is O(1) and intentional for sharing
5. **Governor non-cloneable**: Accept this limitation - use Arc for sharing, create new instances for independent limiters
6. **Recommended**: Remove Clone from trait, use `Arc<dyn Limiter>` for trait objects
