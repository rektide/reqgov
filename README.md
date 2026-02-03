# reqgov

> Multi-timebase HTTP API rate limiting middleware for reqwest - auto-detects IETF draft, GitHub, GitLab, and custom rate limit headers

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

    let rate_limiter = HttpApiRateLimiter::builder()
        .smoother(smoother_config)
        .build();

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(reqwest_ratelimit::all(rate_limiter))
        .build();

    let response = client.get("https://api.example.com/data").send().await?;
    println!("Status: {}", response.status());

    Ok(())
}
```

### Processing URL Lists with Backpressure

Throw thousands of URLs at the library without overwhelming memory or the server:

```rust
use reqgov::{HttpApiRateLimiter, ResponseAdapter};
use reqwest_middleware::ClientBuilder;
use futures::stream::{self, StreamExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rate_limiter = HttpApiRateLimiter::builder().build();

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(reqwest_ratelimit::all(rate_limiter))
        .with(ResponseAdapter)
        .build();

    let urls = vec![
        "https://api.github.com/repos/rust-lang/rust",
        "https://api.github.com/repos/tokio-rs/tokio",
        // ... thousands more
    ];

    stream::iter(urls)
        .map(|url| async move {
            client.get(url).send().await
        })
        .buffer_unordered(10)  // ← Max 10 concurrent requests
        .for_each(|result| async move {
            match result {
                Ok(resp) => println!("{}: {}", resp.url(), resp.status()),
                Err(e) => eprintln!("Error: {}", e),
            }
        })
        .await;

    Ok(())
}
```

**`buffer_unordered(10)` provides two layers of control:**
- **Backpressure**: Only 10 futures exist at any time, preventing memory explosion
- **Concurrency limiting**: Rough equivalent to `max_concurrent_global(10)` semaphore

Both patterns work — choose `buffer_unordered` for stream-based workflows or the semaphore for explicit control.

### Understanding Rate Limit Headers

This library automatically adapts to rate limit headers from any API:

#### IETF Draft Spec (Preferred)
If the API provides [IETF draft](https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/) headers:

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

#### Auto-Detection (Common Formats)
For APIs that use custom headers, the library probes and adapts:

**GitHub style:**
```
X-RateLimit-Remaining: 4999
X-RateLimit-Limit: 5000
X-RateLimit-Reset: 1704067200
```

**GitLab style:**
```
RateLimit-Remaining: 1999
RateLimit-Limit: 2000
RateLimit-Reset: 1704067200
```

The library supports case-insensitive matching and multiple common header patterns.

### Per-Origin Rate Limiting

Each API domain gets its own rate limiter:

```rust
use reqgov::{HttpApiRateLimiter, SmootherConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rate_limiter = HttpApiRateLimiter::builder()
        .smoother(SmootherConfig::default())
        .build();

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

Enrich reqwest-tracing spans with rate limit state using composable middleware:

```rust
use reqgov::{OriginRateLimiter, RateLimitTracing, PolicyTracing, SmootherTracing, StatusTracing};
use reqwest_middleware::ClientBuilder;
use reqwest_tracing::TracingMiddleware;
use std::sync::Arc;

let limiter = Arc::new(OriginRateLimiter::new());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(TracingMiddleware::default())  // Creates HTTP request spans
    .with(RateLimitTracing::new(limiter.clone()))  // Registers limiter in extensions
    .with(PolicyTracing)   // Records policy quotas and remaining counts
    .with(SmootherTracing) // Records smoother velocity and intervals
    .with(StatusTracing)   // Records allowed/blocked status
    .build();
```

Each tracing middleware is optional and composable — include only what you need:

| Middleware | Span Attributes |
|------------|-----------------|
| `RateLimitTracing` | Registers limiter for other middleware (no attributes) |
| `PolicyTracing` | `rate_limit.policy.{name}.remaining`, `.quota`, `.window_secs` |
| `SmootherTracing` | `rate_limit.smoother.velocity`, `.micro_interval_secs`, `.remaining` |
| `StatusTracing` | `rate_limit.status` ("allowed" or "blocked") |

For concurrency limiting telemetry:

```rust
use reqgov::{HttpApiRateLimiter, ConcurrencyTracing, ConcurrencyTracingMiddleware};

let limiter = Arc::new(
    HttpApiRateLimiter::builder()
        .smoother(SmootherConfig::default())
        .max_concurrent_global(100)   // max 100 global concurrent requests
        .max_concurrent_per_domain(10) // max 10 per domain
        .build()
);

let client = ClientBuilder::new(reqwest::Client::new())
    .with(ConcurrencyTracing::new(limiter.clone()))
    .with(ConcurrencyTracingMiddleware)
    .build();
```

### Auto-Detection Middleware

The `ResponseAdapter` middleware automatically detects and configures rate limits from response headers:

```rust
use reqgov::{HttpApiRateLimiter, ResponseAdapter};
use reqwest_middleware::ClientBuilder;
use std::sync::Arc;

let limiter = Arc::new(HttpApiRateLimiter::builder().build());

let client = ClientBuilder::new(reqwest::Client::new())
    .with(reqwest_ratelimit::all(limiter.clone()))
    .with(ResponseAdapter)  // Auto-detect GitHub, GitLab, IETF headers
    .build();
```

  ## API

### ResponseAdapter

Middleware that auto-detects rate limit headers and updates limiters.

```rust
use reqgov::ResponseAdapter;
use reqwest_middleware::ClientBuilder;

let client = ClientBuilder::new(reqwest::Client::new())
    .with(ResponseAdapter)  // Auto-detect and adapt
    .build();
```

- Probes responses for rate limit patterns (IETF, GitHub, GitLab, custom)
- Automatically configures limiters when no policies exist
- Case-insensitive header matching
- Continues using detected configuration once established

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
| `builder()` | Create a new builder |
| `set_url(url: Url)` | Set the current request URL |
| `acquire_permit()` | Wait until rate limit allows (trait method) |

#### `HttpApiRateLimiterBuilder`

| Method | Description |
|--------|-------------|
| `smoother(config: SmootherConfig)` | Configure smoother settings |
| `registry(registry: Arc<OriginRegistry>)` | Use a shared registry |
| `max_concurrent_global(max: usize)` | Set max global concurrent requests |
| `max_concurrent_per_domain(max: usize)` | Set max per-domain concurrent requests |
| `build()` | Build the rate limiter |

### `OriginRegistry`

Thread-safe registry mapping domains to rate limiters.

| Method | Description |
|--------|-------------|
| `builder()` | Create a new builder |
| `get_limiter(url: &Url)` | Get or create limiter for origin |
| `update_from_response(url: &Url, headers: &HeaderMap)` | Update limits from HTTP response |

#### `OriginRegistryBuilder`

| Method | Description |
|--------|-------------|
| `smoother(config: SmootherConfig)` | Configure smoother settings |
| `max_concurrent_global(max: usize)` | Set max global concurrent requests |
| `max_concurrent_per_domain(max: usize)` | Set max per-domain concurrent requests |
| `build()` | Build the registry |

## Background

### The Problem

Every API communicates rate limits differently:

- GitHub uses `X-RateLimit-Remaining` / `X-RateLimit-Limit` / `X-RateLimit-Reset`
- GitLab uses `RateLimit-Remaining` / `RateLimit-Limit` / `RateLimit-Reset`
- Some use `retry-after` or `reset-after`
- The IETF draft spec uses `RateLimit-Policy` and `RateLimit`
- Many APIs don't tell you anything until you hit 429

### The Solution

The library implements the [IETF draft specification](https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/) **and** auto-detects common custom formats:

**IETF Draft** — "Here are my rules" and "Here's where you stand":
```
RateLimit-Policy: "burst";q=100;w=60, "daily";q=1000;w=86400
RateLimit: "burst";r=45;t=30, "daily";r=850
```

**Auto-Detection** — Probes and adapts to any header format:
```
X-RateLimit-Remaining: 45
X-RateLimit-Limit: 60
X-RateLimit-Reset: 1704067200
```

### How It Works

1. **Request comes in** → Parse URL to extract origin (domain)
2. **Check rate limiters** → Each policy must allow (burst AND daily)
3. **Apply smoothing** → Prevent micro-bursts with 2-second intervals
4. **Send request** → Proceed when all limits pass
5. **Update from headers** → Reconfigure based on API response
6. **Auto-probe** → If no policies exist, detect header format and configure automatically

### Probing Behavior

- **Initial requests**: Library probes response headers for rate limit patterns
- **Detection**: GitHub, GitLab, and common formats automatically recognized
- **Caching**: Once detected, policies persist for that origin
- **Fallback**: If headers absent, requests proceed without limiting (safe default)

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
│  │  ┌─────────────────────┐  ┌────────────────────────────┐   │  │
│  │  │   OriginRegistry    │  │   HeaderDetector          │   │  │
│  │  │  (domain → Limiter) │  │   (auto-detect patterns)  │   │  │
│  │  └─────────┬───────────┘  └────────────┬───────────────┘   │  │
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

### Tracing Architecture

Tracing uses the reqwest-middleware `Extensions` pattern for zero-copy state access:

1. `RateLimitTracing` registers `Arc<OriginRateLimiter>` in request extensions
2. Downstream middleware (`PolicyTracing`, `SmootherTracing`, etc.) read directly from the limiter
3. No intermediate state structs — tracing reads live data from the source

This design eliminates state duplication and allows each tracing concern to be independently enabled.

### Governor Constraints

Governor's architecture imposes constraints on what we can observe:

- **No internal permit count**: Governor doesn't expose remaining permits directly. We derive counts from the `StateInformationMiddleware` snapshot after each `check()`.

- **No queue depth visibility**: When multiple requests call `until_ready()`, they queue internally, but queue length isn't accessible.

- **Clock opacity**: Governor's `DefaultClock` doesn't expose refill timing. We can't calculate "X permits will refill in Y seconds."

- **Rebuilds destroy history**: When `PolicySlot` rebuilds its governor (on quota drops >20%), old state is discarded.

### Testing Considerations

- **Middleware ordering**: `TracingMiddleware` → `RateLimitTracing` → tracing plugins ensures spans exist before enrichment.

- **Span isolation**: Use `tracing_subscriber::Registry()` with default layer to avoid test contamination.

- **Header parsing edge cases**: Test with malformed IETF headers to ensure tracing handles errors gracefully.

### Auto-Detection Implementation

**Header detection** - Automatically recognize and configure for various rate limit header formats:

- **Supported formats**: IETF draft spec, GitHub (`X-RateLimit-*`), GitLab (`RateLimit-*`), and common variants
- **Case-insensitive matching**: Works regardless of header name casing
- **Probing behavior**: Continues detecting until policies are established, then uses detected format
- **Fallback**: Requests proceed without limiting if headers absent (safe default)
- **Window inference**: Automatically infers time windows from reset timestamps (60s, 3600s, 86400s)

This allows the library to work with any API that provides rate limit headers, not just those following the IETF draft spec.

 ## Contributing

Contributions welcome! Please feel free to submit a Pull Request.

## License

MIT © [rektide](https://github.com/rektide)
