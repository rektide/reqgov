# reqgov

> Multi-timebase HTTP API rate limiting middleware for reqwest - supports IETF draft, GitHub, and GitLab

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [API](#api)
- [Background](#background)
- [Contributing](#contributing)

## Install

Add this to your `Cargo.toml`:

```toml
[dependencies]
reqgov = "0.1"
reqwest = "0.13"
reqwest-middleware = "0.5"
reqwest-ratelimit = "0.5"
tokio = { version = "1", features = ["full"] }
```

## Usage

### Basic Setup

Create a rate-limited HTTP client:

```rust
use reqgov::{HttpApiRateLimiter, SmootherConfig};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let smoother_config = SmootherConfig {
        micro_interval_secs: 2,
        velocity: 1.5,
    };

    let rate_limiter = HttpApiRateLimiter::new(smoother_config);

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(reqwest_ratelimit::all(rate_limiter))
        .build();

    let response = client.get("https://api.example.com/data").send().await?;
    println!("Status: {}", response.status());

    Ok(())
}
```

### Understanding Rate Limit Headers

This library implements the [IETF draft specification](https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/) for HTTP rate limit headers:

**`RateLimit-Policy`** — Static configuration from the API:
```
RateLimit-Policy: "burst";q=100;w=60, "daily";q=1000;w=86400
```
Translation: 100 requests per minute (burst) and 1000 per day (daily).

**`RateLimit`** — Dynamic state of your usage:
```
RateLimit: "burst";r=45;t=30, "daily";r=850
```
Translation: 45 burst requests remaining (resets in 30s), 850 daily remaining.

### Per-Origin Rate Limiting

Each API domain gets its own rate limiter:

```rust
use reqgov::{HttpApiRateLimiter, SmootherConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rate_limiter = HttpApiRateLimiter::new(SmootherConfig::default());

    // GitHub API
    rate_limiter.set_url(url::Url::parse("https://api.github.com/repos").unwrap()).await;
    // ... requests to github.com are rate-limited based on GitHub's headers

    // GitLab API - separate limiter
    rate_limiter.set_url(url::Url::parse("https://gitlab.com/api/v4/projects").unwrap()).await;
    // ... requests to gitlab.com use GitLab's rate limits

    Ok(())
}
```

### Advanced Configuration

#### Smoother Settings

The smoother divides time windows into micro-intervals to prevent bursting:

```rust
use reqgov::SmootherConfig;

let config = SmootherConfig {
    micro_interval_secs: 2,  // Check every 2 seconds
    velocity: 1.5,           // Race at 1.5x speed (finish 33% early)
};

// Velocity examples:
// 0.5 = Conservative: use only half the quota rate
// 1.0 = Even spread: finish exactly at window end
// 1.5 = Default: race ahead, finish 33% early
 // 2.0 = Aggressive: finish 50% early
 ```

### Tracing Integration

Enrich reqwest-tracing spans with governor rate limit state:

```rust
use reqgov::{HttpApiRateLimiter, RateLimitTelemetry};
use reqwest_middleware::ClientBuilder;
use reqwest_tracing::TracingMiddleware;

let rate_limiter = Arc::new(HttpApiRateLimiter::default());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(TracingMiddleware::default())  // Creates HTTP request spans
    .with(reqwest_ratelimit::all(rate_limiter.clone()))  // Apply rate limiting
    .with(RateLimitTelemetry::new_standard(rate_limiter))  // Enrich spans
    .build();
```

Four verbosity levels:
- `NoOpSpanBackend` - Disabled (zero-cost)
- `MinimalSpanBackend` - Only status flags
- `StandardSpanBackend` - Basic state (velocity, remaining)
- `DetailedSpanBackend` - Full timing (reset times, throttle duration)

Span attributes: `rate_limit.enabled`, `rate_limit.will_throttle`, `rate_limit.origin`, `rate_limit.smoother.velocity`, `rate_limit.policy.{name}.remaining`, etc.

 ## API

### `SmootherConfig`

Configuration for request smoothing.

| Field | Type | Default | Description |
|-------|-------|---------|-------------|
| `micro_interval_secs` | `u32` | `2` | Seconds between micro-interval checks |
| `velocity` | `f64` | `1.5` | Velocity multiplier for pacing |

### `HttpApiRateLimiter`

Main rate limiter that implements `reqwest_ratelimit::RateLimiter`.

| Method | Description |
|--------|-------------|
| `new(config: SmootherConfig)` | Create a new rate limiter |
| `with_registry(registry: Arc<OriginRegistry>)` | Use a shared registry |
| `set_url(url: Url)` | Set the current request URL |
| `acquire_permit()` | Wait until rate limit allows (trait method) |

### `OriginRegistry`

Thread-safe registry mapping domains to rate limiters.

| Method | Description |
|--------|-------------|
| `new(config: SmootherConfig)` | Create a new registry |
| `get_limiter(url: &Url)` | Get or create limiter for origin |
| `update_from_response(url: &Url, headers: &HeaderMap)` | Update limits from HTTP response |

## Background

### The Problem

Every API communicates rate limits differently:

- GitHub uses `x-ratelimit-remaining`
- GitLab uses `RateLimit-Remaining`
- Some use `retry-after`
- Many APIs don't tell you anything until you hit 429

### The Solution

The IETF draft standardizes this with two headers that work together:

**`RateLimit-Policy`** — "Here are my rules" (static configuration)
```
RateLimit-Policy: "burst";q=100;w=60, "daily";q=1000;w=86400
```

**`RateLimit`** — "Here's where you stand" (dynamic state)
```
RateLimit: "burst";r=45;t=30, "daily";r=850
```

### How It Works

1. **Request comes in** → Parse URL to extract origin (domain)
2. **Check rate limiters** → Each policy must allow (burst AND daily)
3. **Apply smoothing** → Prevent micro-bursts with 2-second intervals
4. **Send request** → Proceed when all limits pass
5. **Update from headers** → Reconfigure based on API response

### Multiple Time Windows

APIs often have overlapping limits at different scales:

```
┌─────────────────────────────────────────────────────────────────────┐
│                           24 hours                                  │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                    Daily limit: 10,000                        │  │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐         ┌─────────┐      │  │
│  │  │  Hour 1 │ │  Hour 2 │ │  Hour 3 │   ...   │ Hour 24 │      │  │
│  │  │ 500/hr  │ │ 500/hr  │ │ 500/hr  │         │ 500/hr  │      │  │
│  │  │┌──┐┌──┐ │ │         │ │         │         │         │      │  │
│  │  ││  ││  │ │ │         │ │         │         │         │      │  │
│  │  │└──┘└──┘ │ │         │ │         │         │         │      │  │
│  │  │ 60/min  │ │         │ │         │         │         │      │  │
│  │  └─────────┘ └─────────┘ └─────────┘         └─────────┘      │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

A request must pass **ALL** limits to proceed.

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     reqwest-middleware                          │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │              HttpApiRateLimitMiddleware                   │  │
│  │  ┌─────────────────────┐  ┌────────────────────────────┐  │  │
│  │  │   OriginRegistry    │  │   RateLimitHeaderParser    │  │  │
│  │  │  (domain → Limiter) │  │   (parse Policy/RateLimit) │  │  │
│  │  └─────────┬───────────┘  └────────────┬───────────────┘  │  │
│  │            │                           │                   │  │
│  │            ▼                           ▼                   │  │
│  │  ┌─────────────────────────────────────────────────────┐  │  │
│  │  │              OriginRateLimiter                      │  │  │
│  │  │  ┌────────────┐  ┌────────────┐  ┌──────────────┐   │  │  │
│  │  │  │ PolicySlot │  │ PolicySlot │  │  PolicySlot  │   │  │  │
│  │  │  │  "burst"   │  │  "daily"   │  │   "custom"   │   │  │  │
│  │  │  │  100/60s   │  │  1000/day  │  │   500/hour   │   │  │  │
│  │  │  └─────┬──────┘  └──────┬─────┘  └──────┬───────┘   │  │  │
│  │  │        │                │               │           │  │  │
│  │  │        ▼                ▼               ▼           │  │  │
│  │  │  ┌──────────────────────────────────────────────┐   │  │  │
│  │  │  │                   Smoother                   │   │  │  │
│  │  │  │   (divides fastest window into micro-slots)  │   │  │  │
│  │  │  │         governor @ 2s intervals              │   │  │  │
│  │  │  └──────────────────────────────────────────────┘   │  │  │
│  │  └─────────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
 └─────────────────────────────────────────────────────────────────┘
 ```

 ## Technical Notes

### Constraints

Governor's architecture imposes constraints on telemetry accuracy:

- **No internal permit count exposure**: Governor doesn't provide direct access to remaining permits per limiter. State snapshots are derived from rate and window calculations, not actual counters.

- **Approximate smoother state**: Smoother's micro-interval pacing doesn't expose granular permit tracking. `SmootherState.remaining_per_interval` is a velocity-based approximation, not an exact count.

- **Race conditions with concurrent requests**: State snapshots (`state()` methods) capture a point-in-time view. Multiple concurrent requests may see stale state before governor updates internal counters.

- **No visibility into permit queue depth**: Governor queues waiting requests but doesn't expose queue length. Cannot report "X requests waiting in queue" via spans.

- **Policy reset time uncertainty**: `PolicySlotState.reset_at` is set from HTTP header timestamps, not actual governor reset times. May drift from governor's internal clock.

- **No per-request wait duration**: Governor signals when permits are available but doesn't report how long requests waited. Can't record "acquired after 150ms" in spans.

### Possible Enhancements

#### Span Enrichment Extensions

Current span enrichment provides governor state snapshots, but several enhancements would improve observability:

**Permit queue depth telemetry** - Add histogram metric tracking number of requests waiting in governor's queue. Requires governor modifications to expose `queue.len()`.

**Per-request wait duration** - Instrument `acquire_permit()` to measure and record wait time in spans. Example attribute: `rate_limit.wait_duration_ms = 150`.

**Dynamic attribute selection** - Allow users to specify which span attributes they want via configuration, reducing telemetry overhead for unneeded fields.

**Span context propagation** - Pass governor state from request to response span, enabling correlation of rate limit state with HTTP status codes.

**Policy violation spans** - Create new span when rate limit would be exceeded, recording which policy failed and by how much (e.g., "burst quota exceeded by 5 requests").

**Smoother histogram metrics** - Track smoother behavior over time: micro-intervals utilized, velocity adjustments, throttling events.

**Throttle event telemetry** - When throttling occurs, create span with context: which policy triggered it, expected wait duration, and reason (burst vs daily limit).

### Testing Considerations

Tracing integration testing presents unique challenges:

- **Span isolation**: Tests must ensure span contexts don't leak between test functions. `tracing_subscriber::Registry()` with default layer avoids test contamination.

- **Mock governor behavior**: Tests require controllable governor state to verify span attributes. Consider test utilities that set specific permit counts, velocities, and reset times.

- **Concurrency testing**: Span enrichment with concurrent requests needs careful synchronization to verify state consistency. May need barriers to control request timing.

- **Backend completeness**: Each backend (NoOp, Minimal, Standard, Detailed) needs distinct test coverage. Verify correct attributes are recorded/not-recorded per backend.

- **Middleware ordering**: Tests should validate middleware ordering: TracingMiddleware → reqwest_ratelimit → RateLimitTelemetry ensures spans exist before enrichment.

- **Performance benchmarks**: Measure NoOp vs Standard vs Detailed overhead. Governor calls are O(1), but span recording costs vary with attribute count.

- **Header parsing edge cases**: Test with malformed IETF headers, missing policies, and duplicate policy names to ensure span enrichment handles errors gracefully.

- **Origin isolation**: Verify span attributes correctly distinguish multiple origins (e.g., github.com vs gitlab.com) in concurrent requests.

 ## Contributing

Contributions welcome! Please feel free to submit a Pull Request.

## License

MIT © [rektide](https://github.com/rektide)
