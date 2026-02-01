# Enricher Composition Plan

## Overview

Design a builder pattern for composing multiple `SpanEnricher` implementations, allowing granular control over which enrichment is applied. Current implementation only supports a single enricher, but users need to combine multiple enrichers (e.g., `SmootherEnricher` + `StandardSpanEnricher`).

## Problem Statement

**Current limitation:**
```rust
pub struct OriginRateLimiter {
    span_enricher: Arc<dyn SpanEnricher + Send + Sync>,  // SINGLE enricher only
}
```

**User need:**
```rust
// Want to combine enrichers
let combined = SmootherEnricher
    .and_then(StandardSpanEnricher)
    .and_then(PolicyOnlyEnricher);

limiter.set_enricher(combined);
```

**Why composition is needed:**
1. **Granular control**: Choose exactly which enrichers to apply
2. **Mix & match**: Combine single-purpose enrichers
3. **Flexible ordering**: Control enrichment sequence
4. **Reusable patterns**: Common combinations as presets
5. **Easy testing**: Compose and test enrichers independently

## Proposed Architecture

### Option 1: Chained Enrichers (Recommended for simplicity)

```rust
pub struct ChainedEnricher {
    enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>,
}

impl SpanEnricher for ChainedEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        for enricher in &self.enrichers {
            if enricher.is_enabled() {
                enricher.enrich(span, context);
            }
        }
    }
}

impl ChainedEnricher {
    pub fn new() -> Self {
        Self {
            enrichers: Vec::new(),
        }
    }
    
    pub fn with_enricher<E: SpanEnricher + 'static>(mut self, enricher: E) -> Self {
        self.enrichers.push(Arc::new(enricher));
        self
    }
    
    pub fn with_enrichers<E>(mut self, enrichers: Vec<E>) -> Self {
        for enricher in enrichers {
            self.enrichers.push(Arc::new(enricher));
        }
        self
    }
}
```

**Usage:**
```rust
let combined = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher);

let config = SmootherConfig {
    span_enricher: Box::new(combined),
};
```

**Pros:**
- Simple: Easy to understand and use
- Fast: Linear iteration through enrichers
- Flexible: Add/remove enrichers dynamically
- No allocation: `Vec<Arc<>>` only allocated once

**Cons:**
- All enrichers always called (even if some disabled)
- No short-circuit: Can't skip enrichers based on conditions
- All-or-nothing: All enrichers or nothing

**Recommendation:** Use this for production simplicity

### Option 2: Conditional Chaining

```rust
pub struct ConditionalEnricher {
    condition: Box<dyn Fn(&SpanContext) -> bool + Send + Sync>,
    enricher: Arc<dyn SpanEnricher + Send + Sync>,
}

impl SpanEnricher for ConditionalEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        if (self.condition)(context) {
            self.enricher.enrich(span, context);
        }
    }
}

pub struct ConditionalEnricherBuilder {
    condition: Option<Box<dyn Fn(&SpanContext) -> bool + Send + Sync>>,
    enricher: Option<Arc<dyn SpanEnricher + Send + Sync>>,
}
```

**Usage:**
```rust
let combined = ChainedEnricher::new()
    .with_enricher(ConditionalEnricher::new(
        |ctx| ctx.all_passed,
        Box::new(SmootherEnricher)
    ))
    .with_enricher(ConditionalEnricher::new(
        |ctx| ctx.limiting_policy.is_some(),
        Box::new(StandardSpanEnricher)
    ));
```

**Pros:**
- Selective: Only enrich when conditions met
- Efficient: Skip unnecessary enrichers
- Flexible: Arbitrary conditions

**Cons:**
- Complex: Box<dyn Fn> overhead
- Harder to test: Conditions in closure
- Less predictable: Harder to visualize composition

**Recommendation:** Use for conditional enrichment needs

### Option 3: Enum-Based Composition (Type-safe)

```rust
pub enum CompositeEnricher {
    Single(Arc<dyn SpanEnricher + Send + Sync>),
    Pair(Arc<dyn SpanEnricher + Send + Sync>, Arc<dyn SpanEnricher + Send + Sync>),
    Triple(Arc<dyn SpanEnricher + Send + Sync>, Arc<dyn SpanEnricher + Send + Sync>, Arc<dyn SpanEnricher + Send + Sync>),
    Chain(Arc<ChainedEnricher>),
}

impl SpanEnricher for CompositeEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext) {
        match self {
            CompositeEnricher::Single(e) => e.enrich(span, context),
            CompositeEnricher::Pair(e1, e2) => {
                e1.enrich(span, context);
                e2.enrich(span, context);
            }
            CompositeEnricher::Triple(e1, e2, e3) => {
                e1.enrich(span, context);
                e2.enrich(span, context);
                e3.enrich(span, context);
            }
            CompositeEnricher::Chain(e) => e.enrich(span, context),
        }
    }
}
```

**Usage:**
```rust
let combined = CompositeEnricher::Pair(
    Arc::new(SmootherEnricher),
    Arc::new(StandardSpanEnricher),
);
```

**Pros:**
- Type-safe: Compile-time validation of arity
- No allocation: Fixed-size variants
- Clear: Easy to see composition structure
- Fast: Direct pattern matching

**Cons:**
- Inflexible: Can't have 4+ enrichers at compile time
- Boilerplate: Need new variant for each arity
- Limited: Can't add/remove at runtime

**Recommendation:** Use for fixed composition patterns

### Option 4: Builder Pattern with Fluent API (Most Flexible)

```rust
pub struct SpanEnricherBuilder {
    enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>,
}

impl SpanEnricherBuilder {
    pub fn new() -> Self {
        Self {
            enrichers: Vec::new(),
        }
    }
    
    pub fn with_enricher<E: SpanEnricher + 'static>(mut self, enricher: E) -> Self {
        self.enrichers.push(Arc::new(enricher));
        self
    }
    
    pub fn with_smoother(mut self) -> Self {
        self.with_enricher(SmootherEnricher)
    }
    
    pub fn with_standard(mut self) -> Self {
        self.with_enricher(StandardSpanEnricher)
    }
    
    pub fn with_detailed(mut self) -> Self {
        self.with_enricher(DetailedSpanEnricher)
    }
    
    pub fn with_policies(mut self) -> Self {
        self.with_enricher(PolicyOnlyEnricher)
    }
    
    pub fn build(mut self) -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(ChainedEnricher {
            enrichers: std::mem::take(&mut self.enrichers),
        })
    }
}
```

**Usage:**
```rust
let combined = SpanEnricherBuilder::new()
    .with_smoother()
    .with_standard()
    .build();

let config = SmootherConfig {
    span_enricher: combined,
};
```

**Pros:**
- Fluent: Clean, readable API
- Flexible: Add/remove enrichers at runtime
- Type-safe: Compile-time methods for common enrichers
- Extensible: Easy to add new builder methods
- Testable: Build step creates final enricher

**Cons:**
- Complex: Builder pattern adds complexity
- Overhead: Vec allocation, Box<dyn>
- Learning curve: New pattern to learn

**Recommendation:** Use this for maximum flexibility

### Option 5: Preset-Based Composition (Best of Both Worlds)

```rust
pub struct EnricherPresets;

impl EnricherPresets {
    pub fn minimal() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(MinimalSpanEnricher)
    }
    
    pub fn standard() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(StandardSpanEnricher)
    }
    
    pub fn detailed() -> Box<dyn SpanEnricher + Send + Sync> {
        Box::new(DetailedSpanEnricher)
    }
    
    pub fn production() -> Box<dyn SpanEnricher + Send + Sync> {
        SpanEnricherBuilder::new()
            .with_smoother()
            .with_standard()
            .build()
    }
    
    pub fn debug() -> Box<dyn SpanEnricher + Send + Sync> {
        SpanEnricherBuilder::new()
            .with_smoother()
            .with_detailed()
            .with_standard()
            .build()
    }
    
    pub fn custom(enrichers: Vec<Arc<dyn SpanEnricher + Send + Sync>>) -> Box<dyn SpanEnricher + Send + Sync> {
        SpanEnricherBuilder::new()
            .with_enrichers(enrichers)
            .build()
    }
}
```

**Usage:**
```rust
// Presets for common use cases
let config = SmootherConfig {
    span_enricher: EnricherPresets::production(),
};

// Custom composition
let custom = EnricherPresets::custom(vec![
    Arc::new(SmootherEnricher),
    Arc::new(PolicyOnlyEnricher),
]);
```

**Pros:**
- Simple: Choose preset for common needs
- Flexible: Custom composition for complex needs
- Discoverable: IDE completion shows available presets
- Future-proof: Easy to add new presets

**Cons:**
- Limited: Presets may not match exact needs
- Two APIs: Presets + custom (confusing)

**Recommendation:** Use for production presets + custom flexibility

## Comparison of Strategies

| Strategy | Flexibility | Performance | Complexity | Type Safety | Recommendation |
|-----------|--------------|--------------|--------------|---------------|----------------|
| Chained | Medium | High | Low | Low | Production simplicity |
| Conditional | High | High (with skips) | Medium | Low | Conditional needs |
| Enum-Based | Low (compile-time) | Very High | Medium | High | Fixed patterns |
| Builder | Very High | Medium | High | Medium | Maximum flexibility |
| Preset | Medium | Medium | Low | Medium | Common use cases |

## Implementation Steps

### Phase 1: Create ChainedEnricher

1. Define `ChainedEnricher` struct
2. Implement `SpanEnricher` trait with iteration logic
3. Add `new()`, `with_enricher()`, `with_enrichers()` methods
4. Add tests for chaining behavior
5. Add documentation with examples

### Phase 2: Create ConditionalEnricher

1. Define `ConditionalEnricher` struct
2. Implement `SpanEnricher` trait with condition check
3. Add `new()` constructor with closure
4. Add tests for conditional enrichment
5. Add documentation with condition examples

### Phase 3: Create Builder Pattern

1. Define `SpanEnricherBuilder` struct
2. Implement fluent API (`with_smoother()`, `with_standard()`, etc.)
3. Implement `build()` method creating `ChainedEnricher`
4. Add tests for builder composition
5. Add documentation with builder examples

### Phase 4: Create Presets

1. Define `EnricherPresets` struct
2. Implement preset methods (`minimal()`, `standard()`, etc.)
3. Implement `custom()` for arbitrary composition
4. Add tests for preset correctness
5. Add documentation with preset examples

### Phase 5: Update Configuration

1. Update `SmootherConfig` to accept composed enrichers
2. Add convenience constructors to `OriginRateLimiter`
3. Update documentation with composition examples
4. Add migration guide for single enricher users

### Phase 6: Add Tests

1. Test chaining 2+ enrichers
2. Test conditional enrichment
3. Test builder with all fluent methods
4. Test preset correctness
5. Test custom composition
6. Performance benchmarks for each strategy

## Configuration Examples

### Example 1: Minimal Enrichment

```rust
// Simplest: No detailed data
let config = SmootherConfig {
    span_enricher: Box::new(MinimalSpanEnricher),
};
```

### Example 2: Smoother + Standard (Production)

```rust
// Using ChainedEnricher
let combined = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher);

// Or using presets
let config = SmootherConfig {
    span_enricher: EnricherPresets::production(),
};
```

### Example 3: Smoother Only (Debugging)

```rust
// Just smoothing data
let config = SmootherConfig {
    span_enricher: Box::new(SmootherEnricher),
};
```

### Example 4: Full Details (Development)

```rust
// Everything available
let config = SmootherConfig {
    span_enricher: Box::new(DetailedSpanEnricher),
};
```

### Example 5: Custom Composition

```rust
// Using builder
let custom = SpanEnricherBuilder::new()
    .with_smoother()
    .with_enricher(PolicyOnlyEnricher)
    .build();

// Using preset
let custom = EnricherPresets::custom(vec![
    Arc::new(SmootherEnricher),
    Arc::new(MinimalSpanEnricher),
]);
```

### Example 6: Conditional Enrichment

```rust
// Enrich smoother only when smoothing is active
let combined = ChainedEnricher::new()
    .with_enricher(ConditionalEnricher::new(
        |ctx| ctx.extensions.smoother_state.is_some(),
        Box::new(SmootherEnricher)
    ))
    .with_enricher(StandardSpanEnricher);

let config = SmootherConfig {
    span_enricher: combined,
};
```

## Benefits

### 1. Granular Control

Users can choose exactly which enrichment to apply:
- Smoother only for debugging smoothing
- Policies only for policy-focused analysis
- Combined for production observability
- Custom for organization-specific needs

### 2. Mix & Match

Combine any single-purpose enrichers:
- `SmootherEnricher` + `StandardSpanEnricher`
- `PolicyOnlyEnricher` + `MinimalSpanEnricher`
- Any combination of built-in and custom enrichers

### 3. Flexible Ordering

Control sequence of enrichment:
- Policies before smoothing (for correlation)
- Smoothing before metadata (for context)
- Custom order for specific needs

### 4. Reusable Patterns

Common combinations as presets:
- `production()` = smoother + standard
- `debug()` = smoother + detailed + standard
- `minimal()` = just pass/fail

### 5. Backward Compatible

Existing single enricher still works:
```rust
// Old code still works
let config = SmootherConfig {
    span_enricher: Box::new(StandardSpanEnricher),
};
```

### 6. Testable Components

Each enricher independently testable:
- Test `SmootherEnricher` in isolation
- Test chaining behavior
- Test builder composition
- Test conditional logic

## Tradeoffs

### ChainedEnricher

**Pros:**
- Simple and clear
- Fast linear iteration
- Flexible: Add/remove at runtime
- No special cases

**Cons:**
- All enrichers called (no short-circuit)
- Vec allocation on build
- No conditional execution

**Recommendation:** Use for production

### Builder Pattern

**Pros:**
- Fluent, readable API
- Maximum flexibility
- Type-safe common methods
- Easy to extend

**Cons:**
- Most complex approach
- Box<dyn> allocation
- Learning curve for users

**Recommendation:** Use for complex needs

### Preset-Based

**Pros:**
- Simple for common cases
- Discoverable via IDE
- Future-proof (add new presets)

**Cons:**
- Limited to defined presets
- Two APIs (presets + custom)
- May not match exact needs

**Recommendation:** Use for production defaults

## Success Criteria

- [ ] `ChainedEnricher` struct defined and implemented
- [ ] `SpanEnricher` trait works with chained enrichers
- [ ] `SpanEnricherBuilder` with fluent API defined
- [ ] Builder has methods: `with_smoother()`, `with_standard()`, `with_detailed()`, `with_policies()`, `build()`
- [ ] `EnricherPresets` with preset methods defined
- [ ] Configuration examples for all strategies
- [ ] Tests for chaining 2+ enrichers
- [ ] Tests for builder composition
- [ ] Tests for preset correctness
- [ ] Tests for conditional enrichment
- [ ] Performance benchmarks for each strategy
- [ ] Documentation updated with composition examples
- [ ] Migration guide provided
- [ ] All existing tests pass with new composition

## Related Work

- **PLAN-span-enrichment.md**: Trait-based granular span attribute capture (completed)
- **PLAN-smoother-enricher.md**: Optional smoother enrichment design (completed)
- **Ticket archive-list-ekc**: Trait-based span enrichment implementation (completed)
