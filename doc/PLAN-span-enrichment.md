# Granular Span Attribute Capture Design

## Problem Statement

**Current naming issue:**
- We call this "Metrics" but we're really capturing "SpanAttributes"
- These are values that get recorded into tracing spans
- "Metrics" usually implies counters/gauges for monitoring systems
- What we have is snapshot data for span enrichment

**Current coupling issue:**
- `LastCheckResult` mixes rate limiting state with span attributes
- Can't extend span attributes without touching rate limiting code
- No way to customize what gets captured per use case

## Proposed Naming

### Option 1: SpanAttributes (Recommended)

```rust
#[derive(Debug, Clone)]
pub struct SpanAttributes {
    pub rate_limit_mode: String,
    pub rate_limit_timestamp_ms: u64,
    pub rate_limit_all_passed: bool,
    pub rate_limit_limiting_policy: Option<String>,
    pub rate_limit_duration_ms: u64,
    pub smoother_remaining_burst_capacity: u32,
    pub policy_remaining: Vec<(String, u32)>,
}
```

**Pros:**
- Clear: These are attributes for spans
- Familiar: Tracing libraries use this terminology
- Accurate: Not metrics, not state, but span data

**Cons:**
- Still couples to specific attributes
- Hard to extend without modifying struct

### Option 2: SpanPayload

```rust
#[derive(Debug, Clone)]
pub struct SpanPayload {
    pub attributes: HashMap<&'static str, AttributeValue>,
    pub metadata: SpanMetadata,
}

pub enum AttributeValue {
    Bool(bool),
    U64(u64),
    I64(i64),
    Str(String),
    Float(f64),
    Duration(Duration),
}

#[derive(Debug, Clone)]
pub struct SpanMetadata {
    pub timestamp: Instant,
    pub duration: Duration,
    pub mode: StateMode,
}
```

**Pros:**
- Flexible: Can store any attribute types
- Extensible: Add new attributes without struct changes
- Generic: Works with any span backend

**Cons:**
- Less type-safe: HashMap instead of fields
- More runtime cost: String keys, Boxed values
- Harder to document: No IDE completion for keys

### Option 3: SpanContext (Hybrid)

```rust
#[derive(Debug, Clone)]
pub struct SpanContext {
    // Fixed attributes (commonly used)
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
    pub duration_ms: u64,
    
    // Flexible extension (custom attributes)
    pub extensions: SpanExtensions,
}

pub struct SpanExtensions {
    pub attributes: HashMap<&'static str, AttributeValue>,
}

impl SpanExtensions {
    pub fn new() -> Self { /* ... */ }
    
    pub fn set(&mut self, key: &'static str, value: impl Into<AttributeValue>) {
        self.attributes.insert(key, value.into());
    }
}
```

**Pros:**
- Best of both: Type-safe common fields, flexible extensions
- Backward compatible: Can add fixed fields over time
- Extensible: Custom attributes via extensions

**Cons:**
- More complex: Two levels of access
- Decision needed: What's "fixed" vs "extension"?

**Recommendation:** `SpanContext` with hybrid approach

## Trait-Based Architecture

### Core Trait

```rust
/// Trait for enriching spans with rate limit information
pub trait SpanEnricher {
    /// Enrich a span with rate limit context
    fn enrich(&self, span: &Span, context: &SpanContext);
    
    /// Check if enrichment is enabled
    fn is_enabled(&self) -> bool {
        true
    }
}
```

### Built-in Enrichers

#### 1. Minimal Enricher

```rust
pub struct MinimalSpanEnricher;

impl SpanEnricher for MinimalSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        if context.all_passed {
            span.record("rate_limit", "allowed");
        } else {
            span.record("rate_limit", "blocked");
        }
    }
}
```

**Use case:** Production monitoring, low overhead

#### 2. Standard Enricher

```rust
pub struct StandardSpanEnricher;

impl SpanEnricher for StandardSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.allowed", context.all_passed);
        span.record("rate_limit.duration_ms", context.duration_ms);
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy);
        }
    }
}
```

**Use case:** Default tracing, good observability

#### 3. Detailed Enricher

```rust
pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.mode", context.mode.to_string());
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.duration_ms);
        span.record("rate_limit.timestamp_ms", context.metadata.timestamp.elapsed().as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy);
        }
        
        // Governor state
        if let Some(ref smoother) = context.extensions.smoother_state {
            span.record("rate_limit.smoother.remaining_burst_capacity", smoother.remaining_burst_capacity);
        }
        
        // Policy states
        for (name, state) in &context.extensions.policy_states {
            span.record(&format!("rate_limit.policy.{}.remaining", name), state.remaining);
            span.record(&format!("rate_limit.policy.{}.quota", name), state.quota);
        }
    }
}
```

**Use case:** Debugging, detailed observability

#### 4. Custom Enricher

```rust
pub struct CustomSpanEnricher {
    pub custom_attributes: Vec<(&'static str, AttributeValue)>,
}

impl SpanEnricher for CustomSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        // Add custom attributes from config
        for (key, value) in &self.custom_attributes {
            match value {
                AttributeValue::Bool(b) => span.record(key, *b),
                AttributeValue::U64(n) => span.record(key, *n),
                AttributeValue::Str(s) => span.record(key, s),
                _ => span.record(key, value.to_string()),
            }
        }
    }
}
```

**Use case:** Custom observability, integration with specific tracing backends

### Configuration Integration

```rust
pub struct SmootherConfig {
    pub micro_interval_secs: u32,
    pub velocity: f64,
    pub span_enricher: Box<dyn SpanEnricher + Send + Sync>,  // NEW
}

impl Default for SmootherConfig {
    fn default() -> Self {
        Self {
            micro_interval_secs: 1,
            velocity: 1.5,
            span_enricher: Box::new(StandardSpanEnricher),  // Default
        }
    }
}
```

### OriginRateLimiter Integration

```rust
pub struct OriginRateLimiter {
    slots: HashMap<String, PolicySlot>,
    smoother: Smoother,
    fastest_policy: Option<String>,
    last_check_result: Arc<RwLock<Option<SpanContext>>>,  // NEW NAME
    span_enricher: Box<dyn SpanEnricher + Send + Sync>,  // NEW
}

impl OriginRateLimiter {
    pub fn new(smoother_config: SmootherConfig) -> Self {
        Self {
            slots: HashMap::new(),
            smoother: Smoother::new(&smoother_config),
            fastest_policy: None,
            last_check_result: Arc::new(RwLock::new(None)),
            span_enricher: smoother_config.span_enricher,  // NEW
        }
    }
    
    pub fn check(&self) -> Result<(), RateLimitViolation> {
        let start = Instant::now();
        
        // ... existing check logic ...
        
        // Build SpanContext instead of LastCheckResult
        let context = SpanContext {
            all_passed,
            limiting_policy,
            duration_ms: start.elapsed().as_millis(),
            extensions: SpanExtensions {
                smoother_state: Some(smoother_state),
                policy_states,
            },
            metadata: SpanMetadata {
                timestamp: Instant::now(),
                duration: start.elapsed(),
                mode: StateMode::Historical,
            },
        };
        
        *self.last_check_result.write().unwrap() = Some(context);
        
        // Determine result
        /* ... */
    }
}
```

### Middleware Integration

```rust
pub struct RateLimitTelemetry {
    rate_limiter: Arc<OriginRateLimiter>,
}

impl Middleware for RateLimitTelemetry {
    async fn handle(&self, req: Request, extensions: &mut Extensions, next: Next<'_>) -> Result<Response> {
        // Check rate limit (captures SpanContext internally)
        let _ = self.rate_limiter.check();
        
        // Get span context
        let context = self.rate_limiter.span_context().unwrap();
        
        // Enrich span using configured enricher
        if let Some(span) = tracing::Span::current() {
            // Get enricher from rate limiter
            // For now, use default enricher
            let enricher = StandardSpanEnricher;
            
            if enricher.is_enabled() {
                enricher.enrich(&span, &context);
            }
        }
        
        next.run(req, extensions).await
    }
}
```

## Granular Control via Configuration

### Example 1: Production Monitoring

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(MinimalSpanEnricher),  // Fastest
};

// Result in span:
// rate_limit: "allowed" or "blocked"
```

### Example 2: Standard Observability

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(StandardSpanEnricher),  // Default
};

// Result in span:
// rate_limit.allowed: true/false
// rate_limit.duration_ms: 2
// rate_limit.limiting_policy: "burst" (if blocked)
```

### Example 3: Debug Tracing

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(DetailedSpanEnricher),  // Full context
};

// Result in span:
// rate_limit.mode: "historical"
// rate_limit.all_passed: false
// rate_limit.duration_ms: 3
// rate_limit.smoother.remaining_burst_capacity: 75
// rate_limit.policy.burst.remaining: 50
// rate_limit.policy.burst.quota: 100
// ... and more
```

### Example 4: Custom Attributes

```rust
let custom_enricher = CustomSpanEnricher {
    custom_attributes: vec![
        ("my_org.rate_limit_id", AttributeValue::Str("rl-123")),
        ("my_org.team", AttributeValue::Str("platform")),
        ("my_org.region", AttributeValue::Str("us-east-1")),
    ],
};

let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(custom_enricher),
};

// Result in span:
// my_org.rate_limit_id: "rl-123"
// my_org.team: "platform"
// my_org.region: "us-east-1"
```

## Per-Limiter Span Enrichment

### Trait for Limiter-Specific Enrichment

```rust
pub trait LimiterSpanEnricher {
    fn enrich_smoother_span(&self, span: &Span, metrics: &CheckMetrics, state: &SmootherState);
    fn enrich_policy_span(&self, span: &Span, name: &str, metrics: &CheckMetrics, state: &PolicySlotState);
}
```

### Implementation

```rust
pub struct PerLimiterSpanEnricher;

impl LimiterSpanEnricher for PerLimiterSpanEnricher {
    fn enrich_smoother_span(&self, span: &Span, metrics: &CheckMetrics, state: &SmootherState) {
        span.record("rate_limit.smoother.check.duration_ms", metrics.duration.as_millis());
        span.record("rate_limit.smoother.check.passed", metrics.passed);
        span.record("rate_limit.smoother.remaining_burst_capacity", state.remaining_burst_capacity);
        
        if let Some(ref wait) = metrics.wait_duration {
            span.record("rate_limit.smoother.check.wait_duration_ms", wait.as_millis());
        }
    }
    
    fn enrich_policy_span(&self, span: &Span, name: &str, metrics: &CheckMetrics, state: &PolicySlotState) {
        span.record(&format!("rate_limit.policy.{}.check.duration_ms", name), metrics.duration.as_millis());
        span.record(&format!("rate_limit.policy.{}.check.passed", name), metrics.passed);
        span.record(&format!("rate_limit.policy.{}.remaining", name), state.remaining);
        span.record(&format!("rate_limit.policy.{}.quota", name), state.quota);
        
        if let Some(ref wait) = metrics.wait_duration {
            span.record(&format!("rate_limit.policy.{}.check.wait_duration_ms", name), wait.as_millis());
        }
    }
}
```

### Integration with PerLimiterTraceMiddleware

```rust
pub struct PerLimiterTraceMiddleware {
    limiter_enricher: Box<dyn LimiterSpanEnricher + Send + Sync>,
}

impl Middleware for PerLimiterTraceMiddleware {
    async fn handle(&self, req: Request, extensions: &mut Extensions, next: Next<'_>) -> Result<Response> {
        if !self.limiter_enricher.is_enabled() {
            return next.run(req, extensions).await;
        }
        
        let span = tracing::Span::current();
        
        // Get last check result
        let context = self.rate_limiter.span_context().unwrap();
        
        // Create per-limiter spans
        if let Some(ref smoother_state) = context.extensions.smoother_state {
            if let Some(ref smoother_metrics) = context.extensions.smoother_metrics {
                self.limiter_enricher.enrich_smoother_span(&span, smoother_metrics, smoother_state);
            }
        }
        
        for (name, policy_state) in &context.extensions.policy_states {
            if let Some(ref policy_metrics) = context.extensions.policy_metrics.iter()
                .find(|(n, _)| n == name)
                .map(|(_, m)| m) 
            {
                self.limiter_enricher.enrich_policy_span(&span, name, policy_metrics, policy_state);
            }
        }
        
        next.run(req, extensions).await
    }
}
```

## Implementation Steps

### Phase 1: Rename and Restructure

1. Rename `LastCheckResult` → `SpanContext`
2. Rename `CheckMetrics` → keep (accurate name)
3. Add `SpanExtensions` struct for flexible attributes
4. Add `SpanMetadata` struct for span-level metadata
5. Update all references to new names

### Phase 2: Create SpanEnricher Trait

1. Define `SpanEnricher` trait with `enrich()` and `is_enabled()` methods
2. Implement `MinimalSpanEnricher`
3. Implement `StandardSpanEnricher` (default)
4. Implement `DetailedSpanEnricher`
5. Implement `CustomSpanEnricher`

### Phase 3: Add Configuration

1. Add `span_enricher: Box<dyn SpanEnricher>` to `SmootherConfig`
2. Add `span_enricher` field to `OriginRateLimiter`
3. Set default to `StandardSpanEnricher`
4. Update `OriginRateLimiter::new()` to use configured enricher

### Phase 4: Update check() to Build SpanContext

1. Modify `check()` to build `SpanContext` instead of `LastCheckResult`
2. Include `SpanExtensions` for flexible attributes
3. Include `SpanMetadata` for span-level info
4. Store in `last_check_result` (renamed field)

### Phase 5: Update Middleware

1. Modify `RateLimitTelemetry` to use configured enricher
2. Get `span_context()` from limiter (new method)
3. Call `enricher.enrich()` with span and context
4. Handle case when enricher is disabled

### Phase 6: Create PerLimiterSpanEnricher Trait

1. Define `LimiterSpanEnricher` trait
2. Implement per-limiter span enrichment logic
3. Create `PerLimiterTraceMiddleware` using trait

### Phase 7: Add Tests

1. Test each enricher produces expected attributes
2. Test configuration integration
3. Test per-limiter span enrichment
4. Test custom enricher with custom attributes
5. Test that disabled enricher doesn't enrich

### Phase 8: Documentation

1. Update docs with trait-based architecture
2. Provide examples for each enricher type
3. Document how to create custom enrichers
4. Add migration guide for existing users

## Benefits

### 1. Decoupled Architecture

- Rate limiting logic independent of span enrichment
- Can change enricher without touching limiter code
- Multiple enrichers for different use cases

### 2. Type-Safe Extensibility

- Fixed common attributes in `SpanContext` (type-safe)
- Flexible extensions via `SpanExtensions` (custom attributes)
- Compile-time validation of attribute types

### 3. Granular Control

- Choose enrichment level via configuration
- Performance-optimal for production (minimal)
- Detailed for development (full context)
- Custom for integrations (organization-specific)

### 4. Backward Compatible

- Default `StandardSpanEnricher` matches current behavior
- Existing code continues to work unchanged
- Migration path: Switch enricher config

### 5. Testable Components

- Each enricher independently testable
- Mock enrichers for testing
- Clear separation of concerns

## Tradeoffs

### Trait-Based Approach

**Pros:**
- Extensible: Add new enrichers without modifying core
- Configurable: Choose enrichment level at runtime
- Testable: Each enricher unit-testable
- Type-safe: Fixed attributes + flexible extensions

**Cons:**
- More complex: Trait dispatch, Box<dyn>
- Runtime cost: Virtual function calls
- Learning curve: New trait to understand

### Fixed Struct Approach

**Pros:**
- Simple: Direct field access
- Fast: No virtual dispatch
- Predictable: Compile-time known

**Cons:**
- Rigid: Can't extend without modifying struct
- Coupled: Span structure hardcoded
- Breaking: Adding fields requires API changes

**Recommendation:** Trait-based approach for production use

## Success Criteria

- [ ] `LastCheckResult` renamed to `SpanContext`
- [ ] `SpanEnricher` trait defined and documented
- [ ] 4 built-in enrichers implemented (Minimal, Standard, Detailed, Custom)
- [ ] `SmootherConfig` has `span_enricher` field
- [ ] `OriginRateLimiter` uses configured enricher
- [ ] `check()` builds `SpanContext` with extensions
- [ ] `LimiterSpanEnricher` trait for per-limiter enrichment
- [ ] `PerLimiterTraceMiddleware` implemented
- [ ] All existing tests pass with new architecture
- [ ] New tests for each enricher type
- [ ] Configuration examples for all use cases
- [ ] Documentation updated with trait-based design
- [ ] Migration guide provided

## Related Work

- **PLAN-metrics-eager.md**: Eager state capture during check()
- **PLAN-tracing-decompose.md**: Per-limiter tracing spans architecture
- **Ticket archive-list-xat**: Eager metrics capture (completed)
- **Ticket archive-list-lo6**: Per-limiter tracing spans (epic, pending)
