# File Tree Refactoring Plan - Final Structure

## Overview

Refactor `src/` directory for clear separation of concerns. The project now has two main limiting systems:
- **origin/** - Per-domain rate limiting with policy slots and smoothing
- **concurrency/** - Concurrent request limiting with semaphores

## Current State

### Problem Areas

**Scattered concerns:**
- `parsing/` - Header parsing only used by origin
- `policies/` - Policy types only used by origin
- `smoothing/` - Smoother only used by origin
- `registry/origin.rs` - Old registry, no longer used
- `tracing/` - Tracer middleware for both systems

### Current Architecture

**Origin Rate Limiting:**
- `OriginRegistry` - Manages domain→limiter mapping, implements Middleware
- `OriginRateLimiter` - Single domain rate limiter
  - `Option<Smoother>` - Per-domain smoothing
  - `DashMap<policy_name, PolicySlot>` - Policy-based rate limiting

**Concurrency Rate Limiting:**
- `ConcurrencyRegistry` - Manages semaphores
- `ConcurrencyRateLimiter` - Implements Middleware

## Proposed Final File Structure

```
src/
├── lib.rs
├── origin/
│   ├── mod.rs
│   ├── origin.rs         # OriginRateLimiter (single domain)
│   ├── registry.rs       # OriginRegistry (domain→limiter mapping, Middleware)
│   ├── state.rs          # RateLimitViolation
│   ├── policies.rs       # Policy, QuotaUnit, ServiceLimit (moved from policies/)
│   ├── slots.rs          # PolicySlot (moved from policies/)
│   ├── smoother.rs       # Smoother, SmootherConfig (moved from smoothing/)
│   └── parsing.rs       # Header parsing functions (moved from parsing/)
├── concurrency/
│   ├── mod.rs
│   ├── limiter.rs        # ConcurrencyRateLimiter
│   └── registry.rs       # ConcurrencyRegistry
└── tracing/
    ├── mod.rs
    ├── policy.rs         # PolicyTracer
    ├── smoother.rs       # SmootherTracer
    ├── status.rs         # StatusTracer
    └── concurrency.rs    # ConcurrencyTracer
```

## Detailed Breakdown

### 1. `origin/` Folder

All per-domain rate limiting functionality grouped together.

#### `origin/mod.rs`
**Contains:**
- Module exports
- Re-exports: OriginRateLimiter, OriginRateLimiterBuilder, OriginRegistry, OriginRegistryBuilder
- Re-exports: RateLimitViolation, Policy, QuotaUnit, ServiceLimit, PolicySlot
- Re-exports: Smoother, SmootherConfig
- Re-exports: parse_policy_header, parse_limit_header

#### `origin/origin.rs` (~300 lines)
**Contains:**
- `OriginRateLimiter` struct
- `OriginRateLimiterBuilder` struct
- `check()`, `wait()` methods
- `update_policies()`, `update_limits()` methods
- Single domain rate limiting logic

**Current state:**
- Single domain limiter
- `Arc<RwLock<Option<Smoother>>>` for smoother
- `Arc<DashMap<String, PolicySlot>>` for policy slots
- `ArcSwap<Option<String>>` for fastest policy tracking

#### `origin/registry.rs` (~150 lines)
**Contains:**
- `OriginRegistry` struct (implements Middleware)
- `OriginRegistryBuilder` struct
- `get_limiter()` - Get or create per-domain limiter
- `update_from_response()` - Update from HTTP headers
- `origin_key()` - Generate domain key from URL

**Key behavior:**
- Middleware implementation extracts domain from request URL
- Creates per-domain OriginRateLimiter instances on demand
- Updates limiters from HTTP response headers

#### `origin/state.rs` (~50 lines)
**Contains:**
- `RateLimitViolation` enum

#### `origin/policies.rs` (~100 lines)
**Contains:**
- `Policy` struct
- `QuotaUnit` enum
- `ServiceLimit` struct

**Moves from:**
- `policies/policy.rs`

#### `origin/slots.rs` (~150 lines)
**Contains:**
- `PolicySlot` struct
- Policy slot rate limiting logic

**Moves from:**
- `policies/slot.rs`

#### `origin/smoother.rs` (~200 lines)
**Contains:**
- `Smoother` struct
- `SmootherConfig` struct
- Smoothing algorithm

**Moves from:**
- `smoothing/smoother.rs`

#### `origin/parsing.rs` (~100 lines)
**Contains:**
- `parse_policy_header()` function
- `parse_limit_header()` function

**Moves from:**
- `parsing/headers.rs`

### 2. `concurrency/` Folder

Concurrent request limiting with semaphores.

#### `concurrency/mod.rs`
**Contains:**
- Module exports
- Re-exports: ConcurrencyRateLimiter, ConcurrencyRateLimiterBuilder
- Re-exports: ConcurrencyRegistry, ConcurrencyRegistryBuilder

#### `concurrency/limiter.rs` (~175 lines)
**Contains:**
- `ConcurrencyRateLimiter` struct
- `ConcurrencyRateLimiterBuilder` struct
- `acquire_permit()` implementation
- `set_url()`, `get_global_semaphore()`, `get_domain_semaphore()`
- Middleware implementation

**Current state:**
- Uses internal `Arc<ConcurrencyRateLimiterInner>` for cheap cloning
- Stores `current_url: Arc<RwLock<Option<Url>>>`
- Delegates to ConcurrencyRegistry

#### `concurrency/registry.rs` (~150 lines)
**Contains:**
- `ConcurrencyRegistry` struct
- `ConcurrencyRegistryBuilder` struct
- `get_global_semaphore()`
- `get_domain_semaphore()`
- `max_concurrent_global()`, `max_concurrent_per_domain()`

**Current state:**
- Manages global and per-domain semaphores
- Creates semaphores on demand per domain

### 3. `tracing/` Folder

Tracer middleware for both systems.

#### `tracing/mod.rs`
**Contains:**
- Module exports
- Re-exports: PolicyTracer, SmootherTracer, StatusTracer, ConcurrencyTracer

#### `tracing/policy.rs` (~50 lines)
**Contains:**
- `PolicyTracer` struct
- Middleware implementation
- Records policy-related span data

#### `tracing/smoother.rs` (~30 lines)
**Contains:**
- `SmootherTracer` struct
- Middleware implementation
- Records smoother state to spans

#### `tracing/status.rs` (~45 lines)
**Contains:**
- `StatusTracer` struct
- Middleware implementation
- Records rate limit status to spans

#### `tracing/concurrency.rs` (~30 lines)
**Contains:**
- `ConcurrencyTracer` struct
- Middleware implementation
- Records concurrency metrics

## Implementation Steps

### Phase 1: Move Files into `origin/`

1. **Move `policies/policy.rs` → `origin/policies.rs`**
   - Update all imports to `crate::origin::policies`
   - Update `lib.rs` exports

2. **Move `policies/slot.rs` → `origin/slots.rs`**
   - Update all imports to `crate::origin::slots`
   - Update internal imports (PolicySlot → origin::policies)

3. **Move `smoothing/smoother.rs` → `origin/smoother.rs`**
   - Update all imports to `crate::origin::smoother`
   - Update internal imports (Smoother → origin::smoother)

4. **Move `parsing/headers.rs` → `origin/parsing.rs`**
   - Update all imports to `crate::origin::parsing`
   - Update internal imports

5. **Update `origin/mod.rs`**
   - Add: `mod policies;`, `mod slots;`, `mod smoother;`, `mod parsing;`
   - Re-export all public types

6. **Update `lib.rs`**
   - Remove references to old paths
   - Export from new origin module

7. **Run tests**
   - Verify all imports updated correctly

### Phase 2: Clean Up Old Directories

1. **Remove `src/policies/` directory**
   - Delete folder after successful move

2. **Remove `src/smoothing/` directory**
   - Delete folder after successful move

3. **Remove `src/parsing/` directory**
   - Delete folder after successful move

4. **Remove `src/registry/origin.rs`**
   - Delete old unused registry file
   - Remove `src/registry/mod.rs` if only exporting old registry

5. **Update `lib.rs`**
   - Remove old registry exports if still present

6. **Run tests**
   - Ensure no broken references

### Phase 3: Final Verification

1. **Check all imports**
   - Run `cargo check` for errors
   - Fix any remaining references

2. **Run full test suite**
   - `cargo test`
   - All tests should pass

3. **Verify exports**
   - `cargo doc --open`
   - Check public API is intact

4. **Update documentation**
   - Update README with new structure
   - Update AGENTS.md if needed

## Benefits

### Immediate Wins

1. **Logical grouping**
   - All origin-related code in `origin/`
   - Clear ownership: parsing, policies, smoothing belong to origin

2. **Reduced directory depth**
   - From `origin/policies/policy.rs` to `origin/policies.rs`
   - Simpler navigation

3. **Clearer architecture**
   - Two main systems: origin and concurrency
   - Each is self-contained

4. **Single responsibility**
   - `origin/` - Per-domain rate limiting
   - `concurrency/` - Concurrent request limiting
   - `tracing/` - Telemetry for both

### Long-term Benefits

1. **Easier onboarding**
   - New contributors find related code together
   - Clear boundaries between systems

2. **Better maintainability**
   - Changes to origin are isolated to `origin/`
   - Changes to concurrency are isolated to `concurrency/`

3. **Simpler refactoring**
   - Moving between files in same directory is easier
   - Module structure is flatter

## Risks and Mitigations

### Risk: Breaking Imports

**Mitigation:**
- Update imports as we move each file
- Use `cargo check` after each move
- Keep tests updated

### Risk: Test Failures

**Mitigation:**
- Run tests after each phase
- Update test imports as code moves
- Keep test modules in same file as implementation

### Risk: Circular Dependencies

**Mitigation:**
- Careful import structure
- Use `crate::origin::*` pattern where possible
- Test after each move

## Success Criteria

- [ ] `origin/` contains all origin-related code
- [ ] `concurrency/` contains all concurrency-related code
- [ ] `tracing/` contains all tracers
- [ ] Old directories removed (policies/, smoothing/, parsing/, registry/)
- [ ] All imports updated
- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] Public API unchanged (same exports from lib.rs)
- [ ] README updated with new structure

## Related Work

- **README.md** - Needs updates to reflect new structure
- **AGENTS.md** - May need updates to reflect module paths
