# File Tree Refactoring Plan - Incremental Approach

## Overview

Refactor `src/` directory to address separation of concerns, focusing on breaking down the 1008-line `origin_limiter.rs` file that currently contains 42% of the codebase.

## Current State Analysis

### Problem Areas

**origin_limiter.rs (1008 lines) - Critical bottleneck**
Contains mixed concerns:
- 1 trait (`SpanEnricher`)
- 5 enricher structs (Minimal, Standard, Smoother, Detailed, Concurrency)
- `ChainedEnricher` + `EnricherPresets`
- 8+ context/type structs (SpanContext, SpanMetadata, SpanExtensions, ConcurrencyMetrics, AttributeValue, CheckMetrics, StateMode)
- `RateLimitViolation` enum
- `OriginRateLimiterState` + `OriginRateLimiter` (core logic)

**Scattered concerns:**
- `tracing.rs` + `tracing_middleware.rs` - Old `RateLimitSpanBackend` system
- `policy.rs` + `policy_slot.rs` - Related but separate
- `parser.rs` - Header parsing only used by registry
- `middleware.rs` - HTTP middleware wrapper

### Current File Sizes

| File | Lines | % of Codebase |
|-------|--------|----------------|
| origin_limiter.rs | 1008 | 42% |
| tracing_middleware.rs | 253 | 11% |
| policy_slot.rs | 242 | 10% |
| parser.rs | 206 | 9% |
| smoother.rs | 198 | 8% |
| origin_registry.rs | 173 | 7% |
| tracing.rs | 145 | 6% |
| middleware.rs | 101 | 4% |
| policy.rs | 54 | 2% |
| lib.rs | 27 | 1% |
| **Total** | **2407** | **100%** |

## Proposed File Structure (Option 3)

```
src/
├── lib.rs
├── limiter/
│   ├── mod.rs
│   ├── origin.rs         # OriginRateLimiter core logic (~300 lines)
│   ├── state.rs         # OriginRateLimiterState, RateLimitViolation (~100 lines)
│   └── context.rs       # SpanContext, SpanMetadata, SpanExtensions, ConcurrencyMetrics, CheckMetrics, StateMode, AttributeValue (~200 lines)
├── policies/
│   ├── mod.rs
│   ├── policy.rs
│   └── slot.rs
├── smoothing/
│   ├── mod.rs
│   └── smoother.rs
├── tracing/
│   ├── mod.rs
│   ├── enricher/        # Extract ALL from origin_limiter.rs (~300 lines)
│   │   ├── mod.rs
│   │   ├── trait.rs      # SpanEnricher trait
│   │   ├── minimal.rs    # MinimalSpanEnricher
│   │   ├── standard.rs   # StandardSpanEnricher
│   │   ├── smoother.rs   # SmootherEnricher
│   │   ├── detailed.rs   # DetailedSpanEnricher
│   │   ├── concurrency.rs # ConcurrencySpanEnricher
│   │   ├── chain.rs      # ChainedEnricher, EnricherPresets
│   │   └── impl.rs       # impl SpanEnricher for ChainedEnricher
│   ├── legacy.rs        # RateLimitSpanBackend, RateLimitState, all backends
│   └── middleware.rs    # Both telemetry middlewares (from tracing_middleware.rs)
├── parsing/
│   ├── mod.rs
│   └── headers.rs       # All header parsing
├── registry/
│   ├── mod.rs
│   └── origin.rs        # OriginRegistry
└── middleware/
    ├── mod.rs
    └── http.rs          # HttpApiRateLimiter
```

## Detailed Breakdown

### 1. `limiter/` Folder

#### `limiter/origin.rs` (~300 lines)
**Contains:**
- `OriginRateLimiter` struct
- Core rate limiting logic
- `check()`, `wait()`, `state()` methods
- Reconfigure logic

**Extracts from:**
- `origin_limiter.rs` - Main struct + methods

#### `limiter/state.rs` (~100 lines)
**Contains:**
- `OriginRateLimiterState` struct
- `RateLimitViolation` enum
- Related state methods

**Extracts from:**
- `origin_limiter.rs` - State types + violation enum

#### `limiter/context.rs` (~200 lines)
**Contains:**
- `SpanContext` struct
- `SpanMetadata` struct
- `SpanExtensions` struct
- `ConcurrencyMetrics` struct
- `CheckMetrics` struct
- `StateMode` enum
- `AttributeValue` enum
- Associated `From` implementations

**Extracts from:**
- `origin_limiter.rs` - All context and type definitions

### 2. `tracing/enricher/` Folder

#### `tracing/enricher/mod.rs`
**Contains:**
- Module exports
- Re-exports all enrichers and types

#### `tracing/enricher/trait.rs` (~30 lines)
**Contains:**
- `SpanEnricher` trait definition

**Extracts from:**
- `origin_limiter.rs` - Trait definition

#### `tracing/enricher/minimal.rs` (~15 lines)
**Contains:**
- `MinimalSpanEnricher` struct
- `SpanEnricher` implementation

**Extracts from:**
- `origin_limiter.rs` - Enricher implementation

#### `tracing/enricher/standard.rs` (~20 lines)
**Contains:**
- `StandardSpanEnricher` struct
- `SpanEnricher` implementation

**Extracts from:**
- `origin_limiter.rs` - Enricher implementation

#### `tracing/enricher/smoother.rs` (~15 lines)
**Contains:**
- `SmootherEnricher` struct
- `SpanEnricher` implementation

**Extracts from:**
- `origin_limiter.rs` - Enricher implementation

#### `tracing/enricher/detailed.rs` (~20 lines)
**Contains:**
- `DetailedSpanEnricher` struct
- `SpanEnricher` implementation

**Extracts from:**
- `origin_limiter.rs` - Enricher implementation

#### `tracing/enricher/concurrency.rs` (~15 lines)
**Contains:**
- `ConcurrencySpanEnricher` struct
- `SpanEnricher` implementation

**Extracts from:**
- `origin_limiter.rs` - Enricher implementation

#### `tracing/enricher/chain.rs` (~120 lines)
**Contains:**
- `ChainedEnricher` struct
- `ChainedEnricher` methods (`new()`, `with_enricher()`, `with_enrichers()`, `with_dyn_enrichers()`, `build()`)
- `EnricherPresets` struct
- `EnricherPresets` methods (`minimal()`, `standard()`, `detailed()`, `production()`, `debug()`, `custom()`, `concurrency()`)
- `impl SpanEnricher for ChainedEnricher`

**Extracts from:**
- `origin_limiter.rs` - Composition logic + presets

### 3. `tracing/legacy.rs` (~150 lines)
**Contains:**
- `RateLimitSpanBackend` trait
- `RateLimitState` struct
- `MinimalSpanBackend` struct + implementation
- `StandardSpanBackend` struct + implementation
- `DetailedSpanBackend` struct + implementation
- `NoOpSpanBackend` struct + implementation

**Moves from:**
- `tracing.rs` - All old tracing system

### 4. `tracing/middleware.rs` (~260 lines)
**Contains:**
- `RateLimitTelemetry<S>` struct
- `RateLimitTelemetry` methods
- `ConcurrencyTelemetry` struct
- `ConcurrencyTelemetry` methods
- Middleware trait implementations for both

**Moves from:**
- `tracing_middleware.rs` - All telemetry middleware

### 5. Other Folders

#### `policies/mod.rs`, `policies/policy.rs`, `policies/slot.rs`
**Moves from:**
- `policy.rs` → `policies/policy.rs`
- `policy_slot.rs` → `policies/slot.rs`

#### `smoothing/mod.rs`, `smoothing/smoother.rs`
**Moves from:**
- `smoother.rs` → `smoothing/smoother.rs`

#### `parsing/mod.rs`, `parsing/headers.rs`
**Moves from:**
- `parser.rs` → `parsing/headers.rs`

#### `registry/mod.rs`, `registry/origin.rs`
**Moves from:**
- `origin_registry.rs` → `registry/origin.rs`

#### `middleware/mod.rs`, `middleware/http.rs`
**Moves from:**
- `middleware.rs` → `middleware/http.rs`

## Implementation Steps

### Phase 1: Create New Folder Structure

1. Create all new module folders
2. Add empty `mod.rs` files
3. Add module declarations in each `mod.rs`

### Phase 2: Extract Tracing Enrichers

1. Create `tracing/enricher/trait.rs`
   - Move `SpanEnricher` trait
   - Verify trait compiles independently

2. Create individual enricher files
   - `minimal.rs` - Move `MinimalSpanEnricher`
   - `standard.rs` - Move `StandardSpanEnricher`
   - `smoother.rs` - Move `SmootherEnricher`
   - `detailed.rs` - Move `DetailedSpanEnricher`
   - `concurrency.rs` - Move `ConcurrencySpanEnricher`

3. Create `tracing/enricher/chain.rs`
   - Move `ChainedEnricher` and `EnricherPresets`
   - Test composition still works

4. Create `tracing/enricher/mod.rs`
   - Re-export all enrichers and traits
   - Ensure public API matches current

5. Update `lib.rs`
   - Update imports to use new paths
   - Verify all public exports work

6. Run tests
   - Ensure all enricher tests pass

### Phase 3: Extract Limiter Context Types

1. Create `limiter/context.rs`
   - Move all context structs from `origin_limiter.rs`
   - Move associated `From` implementations

2. Create `limiter/state.rs`
   - Move `OriginRateLimiterState`
   - Move `RateLimitViolation` enum

3. Update imports in `limiter/origin.rs`
   - Import from sibling modules

4. Run tests
   - Ensure state-related tests pass

### Phase 4: Move Other Components

1. Move `policies/` folder
2. Move `smoothing/` folder
3. Move `parsing/` folder
4. Move `registry/` folder
5. Move `middleware/` folder

6. Update all imports across codebase

7. Run all tests
   - Verify no broken imports

### Phase 5: Consolidate Tracing System

1. Move legacy tracing to `tracing/legacy.rs`
2. Move telemetry middlewares to `tracing/middleware.rs`
3. Delete old `tracing.rs` and `tracing_middleware.rs`
4. Update `lib.rs` imports
5. Run integration tests

### Phase 6: Final Cleanup

1. Delete `origin_limiter.rs` (should be empty or near-empty)
2. Verify all files are under 300 lines
3. Run full test suite
4. Update documentation (README) with new structure

## Benefits

### Immediate Wins

1. **origin_limiter.rs goes from 1008 lines to ~300**
   - Extract ~300 lines to `tracing/enricher/`
   - Extract ~200 lines to `limiter/context.rs`
   - Extract ~100 lines to `limiter/state.rs`

2. **Clearer navigation**
   - Enrichers in dedicated folder
   - Context types isolated
   - Related components grouped

3. **Easier testing**
   - Each enricher can be tested independently
   - Context types have their own test module
   - Smaller test files per concern

4. **Better code review**
   - PRs can focus on specific folders
   - Smaller diffs per file
   - Easier to review changes

### Long-term Benefits

1. **Scalable structure**
   - New enrichers go to `tracing/enricher/`
   - New policies go to `policies/`
   - Clear location for new features

2. **Maintainability**
   - Easier to find specific functionality
   - Smaller files are easier to understand
   - Reduced merge conflicts

3. **Onboarding**
   - New contributors can navigate structure faster
   - Clear separation of concerns
   - Logical grouping of related code

4. **Test organization**
   - Tests can mirror module structure
   - Clear test scope per module
   - Easier to add focused tests

## Risks and Mitigations

### Risk: Breaking Changes to Public API

**Mitigation:**
- Re-export everything through `lib.rs`
- Maintain same public paths for types
- Verify all examples in README compile

### Risk: Circular Dependencies

**Mitigation:**
- Careful import structure
- Use `mod.rs` for module-level re-exports
- Test incrementally after each move

### Risk: Test Failures

**Mitigation:**
- Run tests after each phase
- Keep tests in same file as implementation
- Update test imports as code moves

### Risk: Merge Conflicts

**Mitigation:**
- Complete refactor in single session
- Commit frequently after each phase
- Use `jj log` to track progress

## Success Criteria

- [ ] All files under 300 lines (origin_limiter.rs reduced by 70%+)
- [ ] All 73+ tests passing
- [ ] No circular dependencies
- [ ] Public API unchanged (same exports from lib.rs)
- [ ] README examples still compile
- [ ] Documentation reflects new structure
- [ ] No compiler warnings
- [ ] All imports updated correctly

## Future Considerations

After completing this refactoring, consider:

1. **Option 1 or Option 2** - Deeper restructuring if project grows
2. **Module-level documentation** - Add `//!` docs to each module
3. **Internal visibility** - Use `pub(crate)` for implementation details
4. **Test modules** - Separate `tests/` modules for integration tests
5. **Examples** - Add `examples/` folder with usage patterns

## Related Work

- **PLAN-filetree-refactor.md** - This document
- **README.md** - Needs updates to reflect new structure
- **AGENTS.md** - Agent instructions for project
