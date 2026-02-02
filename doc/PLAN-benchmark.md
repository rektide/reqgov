# Benchmark Plan: Middleware and Tracer Performance

## Overview

This plan establishes a comprehensive benchmarking suite to measure the performance impact of different middleware and tracer configurations in req-gov. The goal is to provide data-driven insights for users to make informed decisions about which components to enable in production.

## Objectives

1. **Quantify overhead** of each middleware component (rate limiting, concurrency, header parsing)
2. **Measure tracer costs** for different observability levels
3. **Identify bottlenecks** in the middleware chain
4. **Track performance regressions** over time
5. **Provide configuration guidance** for different use cases

## Tooling

### Primary: cargo-nextest

Why `cargo-nextest`?
- Faster test execution than `cargo test` (2-10x speedup)
- Better parallelization and test isolation
- Supports test retries and flaky test detection
- History tracking via `--archive` flag
- Better reporting and visualization

Installation:
```bash
cargo install cargo-nextest
```

History tracking:
```bash
# Run with archiving
cargo nextest run --archive benchmark --all-features

# Compare with previous run
cargo nextest run --archive benchmark --all-features --archive-id <previous-id>
```

### Secondary: Criterion for Micro-benchmarks

For precise, fine-grained measurements:
```toml
[dev-dependencies]
criterion = "0.5"
pprof = { version = "0.13", features = ["criterion", "flamegraph"] }
```

### Load Testing: k6 or hey

For macro-benchmarks (full HTTP request cycles):
```bash
# Install hey
go install github.com/rakyll/hey@latest

# Run load test
hey -n 10000 -c 100 http://localhost:8080/api
```

## Benchmark Matrix

### Middleware Configurations

We test different combinations of middlewares to understand their individual and combined impact:

| Config | Rate Limiting | Concurrency | Header Parsing | Tracers |
|--------|---------------|-------------|----------------|---------|
| **base** | ❌ | ❌ | ❌ | ❌ |
| **rl-only** | ✅ | ❌ | ❌ | ❌ |
| **conc-only** | ❌ | ✅ | ❌ | ❌ |
| **rl-conc** | ✅ | ✅ | ❌ | ❌ |
| **rl-header** | ✅ | ❌ | ✅ | ❌ |
| **conc-header** | ❌ | ✅ | ✅ | ❌ |
| **rl-conc-header** | ✅ | ✅ | ✅ | ❌ |
| **full** | ✅ | ✅ | ✅ | ✅ |

### Tracer Configurations

Test each tracer individually and in combination:

| Config | StatusTracer | PolicyTracer | SmootherTracer | ConcurrencyTracer |
|--------|--------------|--------------|----------------|------------------|
| **no-tracer** | ❌ | ❌ | ❌ | ❌ |
| **status-only** | ✅ | ❌ | ❌ | ❌ |
| **policy-only** | ❌ | ✅ | ❌ | ❌ |
| **smoother-only** | ❌ | ❌ | ✅ | ❌ |
| **conc-only** | ❌ | ❌ | ❌ | ✅ |
| **all-tracers** | ✅ | ✅ | ✅ | ✅ |

## Test Scenarios

### Scenario 1: Cold Start

**Goal**: Measure overhead when no limiters exist yet (first request to origin)

**Setup**:
- No pre-initialized limiters
- Single origin
- Sequential requests

**Metrics**:
- First request latency (with limiter creation)
- Subsequent request latency (limiter cached)
- Memory allocation on first request

**Expected Impact**:
- Base: ~0μs (no overhead)
- rl-only: High first request (limiter creation), low subsequent
- conc-only: Medium first request (semaphore creation), low subsequent

### Scenario 2: Warm Cache

**Goal**: Measure steady-state performance with cached limiters

**Setup**:
- Pre-warm all limiters with initial policies
- Single origin
- High throughput (1000+ req/s)

**Metrics**:
- Requests per second (RPS)
- Latency percentiles (p50, p90, p99, p99.9)
- CPU utilization
- Memory usage

**Expected Impact**:
- Base: Max RPS (baseline)
- rl-only: Minor latency (governor::check is fast)
- conc-only: Semaphore acquisition (fast, but adds overhead)
- With tracers: Incremental cost per tracer

### Scenario 3: Rate Limit Hit

**Goal**: Measure overhead when requests are blocked

**Setup**:
- Configure limits to trigger (e.g., 1 request/sec)
- Send requests faster than limit allows
- Measure blocked vs allowed requests

**Metrics**:
- Time to detect rate limit exceeded
- Time to compute wait duration
- CPU during wait calculation
- Memory pressure from wait state

**Expected Impact**:
- rl-only: Medium (policy iteration + check)
- rl-header: Higher (no wait, but parsing happens)
- With tracers: Extra cost for tracing blocked requests

### Scenario 4: Mixed Origins

**Goal**: Measure overhead with multiple origins

**Setup**:
- 10 different origins
- Round-robin requests across origins
- Each origin has different policies

**Metrics**:
- Hashmap lookup performance (DashMap)
- Lock contention on shared state
- Memory usage per origin
- Cache hit rate

**Expected Impact**:
- Base: Minimal (URL parsing overhead)
- rl-only: Higher (more limiters, more DashMap lookups)
- conc-only: Medium (per-domain semaphores)
- Combined: Additive overhead

### Scenario 5: Policy Update

**Goal**: Measure cost of dynamic reconfiguration

**Setup**:
- Send requests continuously
- Update policies every 100 requests
- Measure update operation impact

**Metrics**:
- Time to update policies
- Impact on in-flight requests
- Memory churn from governor rebuilds
- CPU spike during updates

**Expected Impact**:
- rl-only: Medium (update vector + rebuild governors)
- rl-header: Higher (parse headers + update)
- With tracers: Extra cost during update

### Scenario 6: High Concurrency

**Goal**: Measure lock contention and scalability

**Setup**:
- 1000 concurrent requests
- Single origin
- Rate limit not hit (all requests allowed)

**Metrics**:
- Max concurrent requests handled
- Lock wait time (DashMap::lock)
- Semaphore acquisition time
- Thread pool utilization

**Expected Impact**:
- conc-only: High contention on semaphores
- rl-only: Medium contention on limiter mutexes
- Combined: Additive contention

## Implementation Structure

### Directory Layout

```
benches/
├── mod.rs                    # Benchmark main entry
├── scenarios/
│   ├── cold_start.rs         # Cold start benchmarks
│   ├── warm_cache.rs         # Warm cache benchmarks
│   ├── rate_limit_hit.rs     # Blocked request benchmarks
│   ├── mixed_origins.rs      # Multi-origin benchmarks
│   ├── policy_update.rs      # Dynamic config benchmarks
│   └── high_concurrency.rs   # Scalability benchmarks
├── middleware/
│   ├── rate_limiter.rs       # Rate limiting microbenchmarks
│   ├── concurrency.rs        # Concurrency microbenchmarks
│   ├── header_parser.rs      # Header parsing microbenchmarks
│   └── composite.rs          # Combined middleware benchmarks
├── tracers/
│   ├── status_tracer.rs      # StatusTracer microbenchmarks
│   ├── policy_tracer.rs      # PolicyTracer microbenchmarks
│   ├── smoother_tracer.rs    # SmootherTracer microbenchmarks
│   └── concurrency_tracer.rs # ConcurrencyTracer microbenchmarks
└── utils/
    ├── config.rs             # Benchmark configurations
    ├── metrics.rs            # Metrics collection
    └── setup.rs              # Test harness setup
```

### Benchmark Categories

#### Category 1: Unit Benchmarks (Criterion)

Fine-grained measurements of individual components:

```rust
// benches/middleware/rate_limiter.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_origin_limiter_check(c: &mut Criterion) {
    let limiter = OriginLimiter::builder()
        .build();
    limiter.update_policies(vec![policy]).await;

    c.bench_function("origin_limiter_check", |b| {
        b.iter(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                black_box(limiter.check().await)
            })
        })
    });
}

criterion_group!(benches, bench_origin_limiter_check);
criterion_main!(benches);
```

#### Category 2: Integration Benchmarks (Nextest)

Full middleware chain benchmarks:

```rust
// benches/scenarios/warm_cache.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_middleware_config(c: &mut Criterion, config: &str) {
    let client = build_client(config);

    c.bench_function(format!("warm_cache_{}", config), |b| {
        b.iter(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                black_box(make_request(&client).await)
            })
        })
    });
}

fn bench_all_configs(c: &mut Criterion) {
    for config in ALL_CONFIGS {
        benchmark_middleware_config(c, config);
    }
}

criterion_group!(benches, bench_all_configs);
criterion_main!(benches);
```

#### Category 3: Load Tests (External)

Full HTTP request cycles with realistic workloads:

```bash
# scripts/load_test.sh
#!/bin/bash
for config in base rl-only full; do
    echo "Testing config: $config"
    hey -n 10000 -c 100 -m GET \
        -H "X-Config: $config" \
        http://localhost:8080/api
done
```

## Configuration Matrix Implementation

### Build Configuration

```toml
# Cargo.toml (partial)
[features]
bench-base = []
bench-rate-limiting = []
bench-concurrency = []
bench-tracing = []
bench-full = ["bench-rate-limiting", "bench-concurrency", "bench-tracing"]
```

### Benchmark Harness

```rust
// benches/utils/config.rs
pub struct BenchmarkConfig {
    pub name: String,
    pub use_rate_limiting: bool,
    pub use_concurrency: bool,
    pub use_header_parsing: bool,
    pub tracers: Vec<String>,
}

pub const BENCHMARK_CONFIGS: &[BenchmarkConfig] = &[
    BenchmarkConfig {
        name: "base".to_string(),
        use_rate_limiting: false,
        use_concurrency: false,
        use_header_parsing: false,
        tracers: vec![],
    },
    BenchmarkConfig {
        name: "rl-only".to_string(),
        use_rate_limiting: true,
        use_concurrency: false,
        use_header_parsing: false,
        tracers: vec![],
    },
    // ... more configs
];

pub fn build_client(config: &BenchmarkConfig) -> reqwest_middleware::Client {
    let client = reqwest::Client::new();
    let mut client_builder = reqwest_middleware::ClientBuilder::new(client);

    if config.use_rate_limiting {
        client_builder = client_builder.with(OriginRateLimiter::new());
    }

    if config.use_concurrency {
        client_builder = client_builder.with(ConcurrencyLimiter::new());
    }

    // ... apply tracers

    client_builder.build()
}
```

## Metrics Collection

### Primary Metrics

1. **Throughput**
   - Requests per second (RPS)
   - Concurrency (active requests)
   - Success/failure rate

2. **Latency**
   - p50, p90, p95, p99, p99.9 percentiles
   - Average latency
   - Min/max latency

3. **Resource Usage**
   - CPU utilization (user/system)
   - Memory usage (RSS, heap size)
   - I/O operations
   - Network usage

4. **Overhead Breakdown**
   - Middleware chain latency
   - Individual component latency
   - Lock wait time
   - Context switch rate

### Tools for Metrics

```rust
// benches/utils/metrics.rs
use pprof::ProfilerGuard;

pub struct MetricsCollector {
    profiler: Option<ProfilerGuard>,
    start_time: std::time::Instant,
}

impl MetricsCollector {
    pub fn start() -> Self {
        let profiler = ProfilerGuard::new(100).ok();
        Self {
            profiler,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn stop(self) -> BenchmarkMetrics {
        let elapsed = self.start_time.elapsed();

        if let Some(profiler) = self.profiler {
            if let Ok(report) = profiler.report().build() {
                report.flamegraph(&format!("flamegraph.svg"));
            }
        }

        BenchmarkMetrics {
            duration: elapsed,
            // ... more metrics
        }
    }
}
```

## Continuous Integration

### CI Pipeline

```yaml
# .github/workflows/bench.yml
name: Benchmarks

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install nextest
        run: cargo install cargo-nextest

      - name: Run benchmarks
        run: |
          cargo nextest run --archive benchmark --all-features

      - name: Compare with baseline
        run: |
          cargo nextest run --archive-id <baseline-id> --all-features

      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: target/nextest/bench/
```

### Performance Regression Detection

```yaml
- name: Check for regressions
  run: |
    # Fail if p99 latency increased by >10%
    scripts/check_regressions.py \
      --baseline target/nextest/bench/baseline.json \
      --current target/nextest/bench/current.json \
      --threshold 0.1
```

## Reporting

### Automated Reports

Generate HTML reports after each benchmark run:

```bash
# scripts/generate_report.sh
cargo bench -- --save-baseline main
critcmp main --save-baseline main --save-baseline pr
```

### Historical Trends

Track metrics over time using `cargo-nextest` archive:

```bash
# Run benchmark with archiving
cargo nextest run --archive benchmark --all-features

# List archived runs
cargo nextest archive list

# Compare runs
cargo nextest archive diff <run-id-1> <run-id-2>
```

### Dashboard

Use tools like:
- **Grizzly** for benchmark visualization
- **Benchmarks** web UI
- Custom Grafana dashboard from CSV exports

## Baselines and Targets

### Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| Base latency | < 1ms | No middleware overhead |
| Rate limiting overhead | < 100μs | Check operation |
| Concurrency overhead | < 50μs | Semaphore acquisition |
| Header parsing | < 500μs | Parse + update |
| Tracer overhead | < 10μs each | Minimal per-tracer |
| Memory per origin | < 10KB | Steady state |
| Max RPS (base) | > 10,000 | Reqwest baseline |
| Max RPS (full) | > 5,000 | All middlewares |

### Establishing Baselines

Run benchmarks on each PR:
```bash
# Baseline run
cargo nextest run --archive benchmark --all-features \
  --archive-id baseline-$(date +%Y%m%d)

# PR run
cargo nextest run --archive benchmark --all-features \
  --archive-id pr-$(git rev-parse --short HEAD)
```

## Execution Strategy

### Phase 1: Setup (Week 1)

1. Set up `cargo-nextest`
2. Create benchmark directory structure
3. Implement harness utilities
4. Add criterion dependency
5. Write first benchmark (cold start)

### Phase 2: Middleware Benchmarks (Week 2)

1. Implement all middleware unit benchmarks
2. Implement integration benchmarks
3. Add metrics collection
4. Run initial baseline
5. Document results

### Phase 3: Tracer Benchmarks (Week 3)

1. Implement all tracer unit benchmarks
2. Test tracer combinations
3. Measure tracing overhead
4. Optimize if needed

### Phase 4: Scenario Benchmarks (Week 4)

1. Implement all 6 scenarios
2. Load testing setup
3. Generate comprehensive reports
4. Document configuration guidance

### Phase 5: CI Integration (Week 5)

1. Add benchmark workflow
2. Set up regression detection
3. Configure artifact storage
4. Create dashboard

## Open Questions

1. **Should we use Criterion or just nextest for all benchmarks?**
   - Criterion: Better for microbenchmarks, statistical analysis
   - Nextest: Better for integration benchmarks, faster execution
   - Recommendation: Use both - Criterion for unit, Nextest for integration

2. **How to handle flaky benchmarks?**
   - Rate limiting behavior is inherently variable
   - Need sufficient iterations for statistical significance
   - Consider outliers handling

3. **Should benchmarks run on every PR or just nightly?**
   - Every PR: Better regression detection, slower CI
   - Nightly: Faster PRs, delayed detection
   - Recommendation: Fast benchmarks on PR, full benchmarks nightly

4. **How to visualize tracer overhead?**
   - Per-tracer breakdown
   - Cumulative cost
   - Recommendation: Heat map showing tracer combinations

5. **Should we benchmark in different environments?**
   - Local dev vs CI
   - Different OS (Linux, macOS, Windows)
   - Different CPU architectures
   - Recommendation: Linux in CI (primary), manual cross-platform validation

## Success Criteria

1. ✅ All 6 scenarios implemented and benchmarked
2. ✅ All middleware combinations tested
3. ✅ All tracer combinations tested
4. ✅ Performance targets met
5. ✅ CI pipeline running benchmarks
6. ✅ Performance regression detection working
7. ✅ Documentation with configuration guidance
8. ✅ Historical tracking established

## References

- [cargo-nextest documentation](https://nexte.st)
- [Criterion.rs documentation](https://bheisler.github.io/criterion.rs/book/)
- [Profiling with pprof](https://docs.rs/pprof/)
- [Load testing with hey](https://github.com/rakyll/hey)
- [Reqwest middleware docs](https://docs.rs/reqwest-middleware/)
