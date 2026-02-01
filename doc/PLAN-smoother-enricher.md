# Optional Smoother Enrichment Plan

## Overview

Update span enrichment system to handle optional smoother components, since smoother may not be required in all rate limiting scenarios. Create consistent enrichment patterns for optional components.

## Problem Statement

**Current assumption:**
- All rate limiters have a smoother component
- Smoother state is always available in `SpanContext`
- Enrichers assume smoother exists and is non-optional

**New reality:**
- Smoother is now optional in rate limiting
- `SpanExtensions.smoother_state` is `Option<SmootherState>`
- Some rate limiters may operate without smoothing
- Existing enrichers will panic on missing smoother state

**Example of problem:**
```rust
// Current DetailedSpanEnricher
if let Some(ref smoother_state) = context.extensions.smoother_state {
    span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
}
// Works when smoother exists, but what if None?
```

## Proposed Solution

### Option 1: Guard in Existing Enrichers (Minimal Change)

```rust
pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.mode", format!("{:?}", context.metadata.mode).as_str());
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        span.record("rate_limit.timestamp_ms", context.metadata.timestamp.elapsed().as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        // NEW: Guard optional smoother
        if let Some(ref smoother_state) = context.extensions.smoother_state {
            span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
        }
        
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name).as_str();
            span.record(remaining_key, state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name).as_str();
            span.record(quota_key, state.quota);
        }
    }
}
```

**Pros:**
- Minimal changes to existing code
- No new trait needed
- Backward compatible (works when smoother present)

**Cons:**
- Inconsistent enrichment behavior (smoother sometimes present, sometimes not)
- Harder to document: When does smoother appear?
- Less intuitive: Missing smoother silently omitted

### Option 2: Conditional Enrichment Based on Smoother Presence (Recommended)

```rust
pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.mode", format!("{:?}", context.metadata.mode).as_str());
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        span.record("rate_limit.timestamp_ms", context.metadata.timestamp.elapsed().as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        // Check if smoother enrichment should be applied
        if self.should_enrich_smoother(&context) {
            self.enrich_smoother(span, &context);
        }
        
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name).as_str();
            span.record(remaining_key, state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name).as_str();
            span.record(quota_key, state.quota);
        }
    }
    
    fn should_enrich_smoother(&self, context: &SpanContext) -> bool {
        context.extensions.smoother_state.is_some()
    }
    
    fn enrich_smoother(&self, span: &Span, context: &SpanContext) {
        if let Some(ref smoother_state) = context.extensions.smoother_state {
            span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
        }
    }
}
```

**Pros:**
- Clear conditional logic
- Explicit methods for optional enrichment
- Easy to test: Test `should_enrich_smoother()` separately
- Documentable: Explain when smoother appears

**Cons:**
- More code in enrichers
- Boilerplate for each enricher

**Recommendation:** This approach for production use

### Option 3: SmootherEnricher Trait (Most Flexible)

```rust
pub trait SmootherEnricher: Send + Sync {
    fn enrich_smoother(&self, span: &Span, state: &SmootherState);
    fn is_enabled(&self) -> bool {
        true
    }
}

pub struct StandardSmootherEnricher;

impl SmootherEnricher for StandardSmootherEnricher {
    fn enrich_smoother(&self, span: &Span, state: &SmootherState) {
        span.record("rate_limit.smoother.remaining_per_interval", state.remaining_per_interval);
        span.record("rate_limit.smoother.micro_interval_secs", state.micro_interval_secs);
        span.record("rate_limit.smoother.velocity", state.velocity);
    }
}

pub struct DetailedSmootherEnricher;

impl SmootherEnricher for DetailedSmootherEnricher {
    fn enrich_smoother(&self, span: &Span, state: &SmootherState) {
        span.record("rate_limit.smoother.remaining_per_interval", state.remaining_per_interval);
        span.record("rate_limit.smoother.micro_interval_secs", state.micro_interval_secs);
        span.record("rate_limit.smoother.velocity", state.velocity);
        span.record("rate_limit.smoother.base_window_secs", state.base_window_secs);
    }
}
```

Then update `SpanContext` to include optional smoother enricher:

```rust
pub struct SpanContext {
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
    pub total_duration: Duration,
    pub smoother_metrics: Option<CheckMetrics>,
    pub policy_metrics: Vec<(String, CheckMetrics)>,
    pub extensions: SpanExtensions,
    pub metadata: SpanMetadata,
    pub smoother_enricher: Option<Arc<dyn SmootherEnricher + Send + Sync>>,  // NEW
}
```

And update enrichers to use it:

```rust
pub struct DetailedSpanEnricher;

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.mode", format!("{:?}", context.metadata.mode).as_str());
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        span.record("rate_limit.timestamp_ms", context.metadata.timestamp.elapsed().as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        // NEW: Use smoother enricher if available
        if let (Some(ref smoother_state), Some(ref smoother_enricher)) = (
            &context.extensions.smoother_state,
            &context.smoother_enricher,
        ) {
            if smoother_enricher.is_enabled() {
                smoother_enricher.enrich_smoother(span, smoother_state);
            }
        }
        
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name).as_str();
            span.record(remaining_key, state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name).as_str();
            span.record(quota_key, state.quota);
        }
    }
}
```

**Pros:**
- Separate trait for smoother-specific enrichment
- Can enable/disable smoother enrichment independently
- Consistent with `SpanEnricher` pattern
- Extensible: Add new smoother enrichers without modifying `SpanEnricher`
- Clear separation of concerns

**Cons:**
- More complex: Two-level trait dispatch
- Performance cost: Virtual call + optional check
- More types to understand for users

**Recommendation:** This approach for maximum flexibility

### Option 4: Hybrid - Conditional + Trait (Best of Both Worlds)

```rust
pub struct DetailedSpanEnricher {
    use_smoother_trait: bool,  // Toggle between approaches
}

impl DetailedSpanEnricher {
    pub fn new(use_smoother_trait: bool) -> Self {
        Self {
            use_smoother_trait,
        }
    }
}

impl SpanEnricher for DetailedSpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        // ... common enrichment ...
        
        // Use trait-based if configured
        if self.use_smoother_trait {
            if let (Some(ref smoother_state), Some(ref smoother_enricher)) = (
                &context.extensions.smoother_state,
                &context.smoother_enricher,
            ) {
                if smoother_enricher.is_enabled() {
                    smoother_enricher.enrich_smoother(span, smoother_state);
                }
            }
        } else {
            // Fallback: Conditional enrichment
            if let Some(ref smoother_state) = context.extensions.smoother_state {
                span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
            }
        }
    }
}
```

**Pros:**
- Flexible: Choose approach per use case
- Backward compatible with both approaches
- Migration path: Start with conditional, move to trait later

**Cons:**
- Most complex of all options
- Configuration complexity
- Harder to explain to users

**Recommendation:** Start with Option 2 (Conditional), migrate to Option 3 (Trait) when needed

## When is Smoother Optional?

### Scenarios where smoother is NOT present:

1. **Simple policy-based rate limiting:**
   - Only policies exist, no smoothing
   - Direct governor checks on policies
   - `OriginRateLimiter` created without smoother

2. **Custom limiter implementations:**
   - User-defined rate limiting logic
   - No smoothing needed for their use case
   - Only policy enforcement

3. **Disabled smoothing:**
   - Configuration explicitly disables smoothing
   - Performance optimization
   - Predictable rate limits (no smoothing artifacts)

### Scenarios where smoother IS present:

1. **Standard reqgov usage:**
   - Default configuration includes smoother
   - Burst protection and velocity control
   - Most common production setup

2. **Variable request rates:**
   - Request rates fluctuate rapidly
   - Smoother prevents sharp rate limit hits

## New Enricher Types

### 1. PolicyOnlyEnricher (For smoother-free scenarios)

```rust
pub struct PolicyOnlyEnricher;

impl SpanEnricher for PolicyOnlyEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        // Only enrich policies (no smoother)
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name).as_str();
            span.record(remaining_key, state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name).as_str();
            span.record(quota_key, state.quota);
        }
    }
}
```

**Use case:** Rate limiters without smoothing (minimal performance overhead)

### 2. SmootherEnricher (Standard)

```rust
pub struct StandardSmootherEnricher;

impl SmootherEnricher for StandardSmootherEnricher {
    fn enrich_smoother(&self, span: &Span, state: &SmootherState) {
        span.record("rate_limit.smoother.remaining_per_interval", state.remaining_per_interval);
        span.record("rate_limit.smoother.micro_interval_secs", state.micro_interval_secs);
        span.record("rate_limit.smoother.velocity", state.velocity);
    }
}
```

**Use case:** Standard production with smoothing enabled

### 3. DetailedSmootherEnricher

```rust
pub struct DetailedSmootherEnricher;

impl SmootherEnricher for DetailedSmootherEnricher {
    fn enrich_smoother(&self, span: &Span, state: &SmootherState) {
        span.record("rate_limit.smoother.remaining_per_interval", state.remaining_per_interval);
        span.record("rate_limit.smoother.micro_interval_secs", state.micro_interval_secs);
        span.record("rate_limit.smoother.velocity", state.velocity);
        span.record("rate_limit.smoother.base_window_secs", state.base_window_secs);
        span.record("rate_limit.smoother.quota_burst_capacity", state.quota_burst_capacity);
    }
}
```

**Use case:** Debugging and detailed observability

### 4. AdaptiveEnricher (Adjusts based on components present)

```rust
pub struct AdaptiveEnricher;

impl SpanEnricher for AdaptiveEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        span.record("rate_limit.all_passed", context.all_passed);
        span.record("rate_limit.duration_ms", context.total_duration.as_millis());
        
        // Enrich based on what components are present
        if let Some(ref policy) = context.limiting_policy {
            span.record("rate_limit.limiting_policy", policy.as_str());
        }
        
        // Optional: Enrich smoother if present
        if let Some(ref smoother_state) = context.extensions.smoother_state {
            span.record("rate_limit.smoother.remaining_per_interval", smoother_state.remaining_per_interval);
        }
        
        // Always: Enrich policies (required component)
        for (name, state) in &context.extensions.policy_states {
            let remaining_key = format!("rate_limit.policy.{}.remaining", name).as_str();
            span.record(remaining_key, state.remaining);
            let quota_key = format!("rate_limit.policy.{}.quota", name).as_str();
            span.record(quota_key, state.quota);
        }
    }
}
```

**Use case:** Single enricher for all scenarios (simplest user experience)

## Implementation Steps

### Phase 1: Update Existing Enrichers (Option 2 - Recommended)

1. Update `DetailedSpanEnricher` with `should_enrich_smoother()` guard
2. Add `enrich_smoother()` method to `DetailedSpanEnricher`
3. Update `StandardSpanEnricher` with smoother guard (no smoother data currently)
4. Add `enrich_smoother()` method if needed
5. Update tests for both enrichers to handle `None` smoother state

### Phase 2: Create SmootherEnricher Trait (Option 3 - Future)

1. Define `SmootherEnricher` trait
2. Implement `StandardSmootherEnricher`
3. Implement `DetailedSmootherEnricher`
4. Add `smoother_enricher: Option<Arc<dyn SmootherEnricher>>` to `SpanContext`
5. Update `DetailedSpanEnricher` to use smoother enricher
6. Add tests for smoother enrichers
7. Update configuration to support smoother enricher selection

### Phase 3: Create New Enrichers

1. Implement `PolicyOnlyEnricher` for smoother-free scenarios
2. Implement `AdaptiveEnricher` for all-in-one scenarios
3. Update configuration to choose between enrichers
4. Add tests for new enrichers
5. Add documentation examples for each enricher type

### Phase 4: Update OriginRateLimiter

1. Add `smoother_enricher` field to `OriginRateLimiter` (if using trait approach)
2. Add constructor to configure smoother enricher
3. Update `check()` to populate `smoother_enricher` in `SpanContext`
4. Update `state()` to propagate smoother enricher
5. Add tests for optional smoother scenarios

### Phase 5: Documentation

1. Document when smoother is optional
2. Provide examples of scenarios without smoother
3. Explain each enricher's behavior with/without smoother
4. Add migration guide for existing users
5. Update configuration examples with new enrichers

## Configuration Examples

### Example 1: Production with Smoother (Current Default)

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(DetailedSpanEnricher),
};

// Smoother enabled, detailed enrichment
// Result: rate_limit.smoother.remaining_per_interval, policy states, etc.
```

### Example 2: Production without Smoother

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(PolicyOnlyEnricher),
    // No smoother configured (hypothetical future)
};

// Smoother disabled, policy-only enrichment
// Result: rate_limit.policy.burst.remaining, policy.burst.quota, etc.
```

### Example 3: Adaptive Enrichment

```rust
let config = SmootherConfig {
    micro_interval_secs: 1,
    velocity: 1.5,
    span_enricher: Box::new(AdaptiveEnricher),
};

// Enriches whatever is present
// With smoother: rate_limit.smoother.*, rate_limit.policy.*
// Without smoother: rate_limit.policy.* only
```

## Benefits

### 1. Handles Optional Components

- Enrichers gracefully handle missing smoother
- No panics on `None` smoother state
- Works with future optional components

### 2. Backward Compatible

- Existing `DetailedSpanEnricher` still works when smoother present
- No breaking changes for existing users
- Migration path: Existing code continues to work

### 3. Consistent Patterns

- Guard pattern: Check presence before enrichment
- Trait pattern: Extensible for optional components
- Adaptive pattern: Single enricher for all scenarios

### 4. Clear Use Case Mapping

- `PolicyOnlyEnricher` → Smoother-free rate limiting
- `StandardSmootherEnricher` → Production with smoothing
- `DetailedSmootherEnricher` → Debugging with smoothing
- `AdaptiveEnricher` → Single enricher for all scenarios

## Tradeoffs

### Guard-Based Approach (Option 2)

**Pros:**
- Simple: Easy to understand
- Fast: No trait dispatch overhead
- Compatible: Works with existing code

**Cons:**
- Boilerplate: Repeated guard logic
- Inconsistent: Smoother sometimes enriched, sometimes not
- Less testable: Harder to isolate smoother tests

**Recommendation:** Start with this, migrate to trait when needed

### Trait-Based Approach (Option 3)

**Pros:**
- Extensible: Add new enrichers easily
- Independent: Test smoother enrichers separately
- Clean: Clear separation of concerns

**Cons:**
- Complex: Two-level trait dispatch
- Overhead: Virtual function calls
- Learning curve: More types to understand

**Recommendation:** Use for maximum flexibility needs

### Adaptive Approach (Option 4)

**Pros:**
- Simplest: Single enricher for all scenarios
- User-friendly: No configuration decisions needed
- Predictable: Always works

**Cons:**
- Coupled: SpanEnricher knows about optional components
- Harder to extend: New components require changing enricher
- Less granular: Can't choose enrichment level independently

**Recommendation:** Use for simple use cases, trait-based for complex

## Success Criteria

- [ ] Existing enrichers updated to handle optional smoother
- [ ] `should_enrich_smoother()` method added to `DetailedSpanEnricher`
- [ ] `enrich_smoother()` method added to `DetailedSpanEnricher`
- [ ] Tests pass with `smoother_state: None`
- [ ] `SmootherEnricher` trait defined and documented (future phase)
- [ ] `PolicyOnlyEnricher` implemented (future phase)
- [ ] `AdaptiveEnricher` implemented (future phase)
- [ ] Configuration examples for all enricher types
- [ ] Documentation updated with optional component handling
- [ ] Migration guide provided
- [ ] All tests passing (including new optional smoother tests)

## Related Work

- **PLAN-span-enrichment.md**: Trait-based granular span attribute capture (completed)
- **Ticket archive-list-ekc**: Trait-based span enrichment implementation (completed)
- **Ticket archive-list-lo6**: Per-limiter tracing spans (epic, pending)
