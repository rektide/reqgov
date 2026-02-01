# Enricher Composition Plan

## Overview

Document simple composition of `SpanEnricher` implementations. We already have a clean, working design - no complex strategies or over-engineering needed.

## Current Design

### We Already Have

**Single trait for all enrichers:**
```rust
pub trait SpanEnricher: Send + Sync {
    fn enrich(&self, span: &Span, context: &SpanContext);
    fn is_enabled(&self) -> bool {
        true
    }
}
```

**Single-purpose enrichers using the SAME trait:**
```rust
pub struct MinimalSpanEnricher;
impl SpanEnricher for MinimalSpanEnricher { /* ... */ }

pub struct StandardSpanEnricher;
impl SpanEnricher for StandardSpanEnricher { /* ... */ }

pub struct SmootherEnricher;
impl SpanEnricher for SmootherEnricher { /* ... */ }

pub struct DetailedSpanEnricher;
impl SpanEnricher for DetailedSpanEnricher { /* ... */ }
```

**All implement the SAME trait - no separate traits needed!**

## Simple Composition

### ChainedEnricher (All we need)

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
}
```

**That's it! Simple, clean, works.**

## Usage Examples

### Example 1: Smoother Only

```rust
let enricher = ChainedEnricher::new()
    .with_enricher(SmootherEnricher);
```

### Example 2: Smoother + Standard

```rust
let enricher = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher);
```

### Example 3: Smoother + Detailed

```rust
let enricher = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(DetailedSpanEnricher);
```

### Example 4: Standard Only (No Smoother)

```rust
let enricher = ChainedEnricher::new()
    .with_enricher(StandardSpanEnricher);
```

### Example 5: Minimal + Smoother + Standard

```rust
let enricher = ChainedEnricher::new()
    .with_enricher(MinimalSpanEnricher)
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher);
```

## Presets (Convenience Functions)

Just helper functions to create common combinations:

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
    
    pub fn with_smoother(base: Box<dyn SpanEnricher + Send + Sync>) -> Box<dyn SpanEnricher + Send + Sync> {
        ChainedEnricher::new()
            .with_enricher(SmootherEnricher)
            .with_enricher(base.into())
            .build()
    }
}
```

**Usage:**
```rust
// Production: Smoother + Standard
let config = SmootherConfig {
    span_enricher: EnricherPresets::with_smoother(
        Box::new(StandardSpanEnricher)
    ),
};
```

## What We Don't Need

❌ **Separate traits** - We already have ONE `SpanEnricher` trait
❌ **Complex strategies** - Enum-based, conditional, builder patterns all overkill
❌ **Smoothing-specific trait** - `SmootherEnricher` implements `SpanEnricher` (same trait)
❌ **Custom allocation patterns** - `Vec<Arc<>>` is sufficient
❌ **Runtime configuration** - Just use builder methods

## Benefits

### 1. Simple Design

**One trait for everything:**
```rust
trait SpanEnricher {
    fn enrich(&self, span: &Span, context: &SpanContext);
    fn is_enabled(&self) -> bool { true }
}
```

**All enrichers implement this trait:**
- `MinimalSpanEnricher`
- `StandardSpanEnricher`
- `SmootherEnricher`
- `DetailedSpanEnricher`
- Any custom enricher user creates

### 2. Easy Composition

Just chain them together:
```rust
let enriched = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher);
```

### 3. Type Safety

Compile-time validation:
- All enrichers implement `SpanEnricher`
- Can't accidentally use wrong trait
- Method signatures guaranteed by trait

### 4. Zero Learning Curve

**For users:**
1. Implement `SpanEnricher` trait
2. Use `ChainedEnricher::new().with_enricher(YourEnricher)`
3. Done!

**No need to understand:**
- Composition strategies
- Builder patterns
- Enum variants
- Conditional logic

### 5. Extensible

**Add any enricher:**
```rust
pub struct CustomEnricher { /* ... */ }
impl SpanEnricher for CustomEnricher { /* ... */ }

let enriched = ChainedEnricher::new()
    .with_enricher(CustomEnricher);
```

### 6. Testable

Each enricher independently testable:
```rust
#[test]
fn test_smoother_enricher() {
    let enricher = SmootherEnricher;
    // Test in isolation
}
```

## Implementation Steps

### Phase 1: Implement ChainedEnricher

1. Add `ChainedEnricher` struct with `enrichers: Vec<Arc<dyn SpanEnricher>>`
2. Implement `SpanEnricher` for `ChainedEnricher` (iterates and calls each)
3. Implement `new()` constructor
4. Implement `with_enricher()` method (push to Vec)
5. Add tests for chaining 2+ enrichers

### Phase 2: Create Presets

1. Add `EnricherPresets` struct
2. Implement helper functions: `minimal()`, `standard()`, `detailed()`
3. Implement `with_smoother()` helper
4. Add tests for preset correctness

### Phase 3: Update Configuration

1. Update `OriginRateLimiter` to use `ChainedEnricher` if needed
2. Or keep using single enrichers (current approach works fine)
3. Update examples in documentation
4. Add migration guide if changing from single to chained

### Phase 4: Documentation

1. Update `PLAN-span-enrichment.md` to reflect simple design
2. Document `ChainedEnricher` usage
3. Document `EnricherPresets` convenience functions
4. Remove obsolete sections about complex strategies
5. Add simple composition examples

## Configuration Examples

### Example 1: Keep Current (Single Enricher)

```rust
// What we have now - works fine
let config = SmootherConfig {
    span_enricher: Box::new(StandardSpanEnricher),
};
```

### Example 2: Chain Smoother + Standard

```rust
let combined = ChainedEnricher::new()
    .with_enricher(SmootherEnricher)
    .with_enricher(StandardSpanEnricher)
    .build();

let config = SmootherConfig {
    span_enricher: combined,
};
```

### Example 3: Using Preset

```rust
let config = SmootherConfig {
    span_enricher: EnricherPresets::with_smoother(
        Box::new(StandardSpanEnricher)
    ),
};
```

### Example 4: Custom Enricher

```rust
pub struct OrgEnricher { /* ... */ }
impl SpanEnricher for OrgEnricher { /* ... */ }

let custom = ChainedEnricher::new()
    .with_enricher(OrgEnricher)
    .with_enricher(StandardSpanEnricher)
    .build();
```

## Success Criteria

- [ ] `ChainedEnricher` struct defined and implemented
- [ ] `SpanEnricher` trait works with chained composition
- [ ] `ChainedEnricher::new()` constructor
- [ ] `ChainedEnricher::with_enricher()` method
- [ ] `EnricherPresets` with helper functions defined
- [ ] Tests for chaining 2+ enrichers
- [ ] Tests for preset correctness
- [ ] Documentation updated with simple design
- [ ] Complex strategy sections removed from docs
- [ ] Configuration examples for simple composition
- [ ] All existing tests pass
- [ ] Migration guide (if needed)

## Related Work

- **PLAN-span-enrichment.md**: Trait-based granular span attribute capture (completed)
- **PLAN-smoother-enricher.md**: Optional smoother enrichment design (completed)
- **Ticket archive-list-ekc**: Trait-based span enrichment implementation (completed)
