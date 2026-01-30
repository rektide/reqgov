# reqgov

Multi-timebase HTTP API rate limiting middleware for reqwest. Implements [IETF draft-ietf-httpapi-ratelimit-headers](https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/) with governor backend.

## Features

- **IETF Standard Headers**: Parses `RateLimit-Policy` and `RateLimit` headers
- **Multiple Time Windows**: Handles burst, hourly, and daily limits simultaneously
- **Request Smoothing**: Distributes requests evenly using micro-intervals
- **Per-Origin Isolation**: Each API domain gets its own rate limiter
- **Reqwest Integration**: Drop-in middleware for reqwest-based clients

## Quick Start

```rust
use reqwest_middleware::ClientBuilder;
use reqgov::{HttpApiRateLimiter, SmootherConfig};

#[tokio::main]
async fn main() {
    let config = SmootherConfig::default();
    let rate_limiter = HttpApiRateLimiter::new(config);
    
    let client = ClientBuilder::new(reqwest::Client::new())
        .with(reqwest_ratelimit::all(rate_limiter))
        .build();
    
    // First request discovers policies from headers
    let resp = client.get("https://api.example.com/data").send().await?;
}
```

## How It Works

When you make requests to an API, the middleware:

1. **Pre-request**: Waits for all policy governors to allow the request
2. **Post-response**: Parses `RateLimit-Policy` and `RateLimit` headers
3. **Updates**: Reconfigures governors based on remaining quota and reset times

The **Smoother** divides the fastest window into micro-intervals (default 2s) to prevent bursting, spreading requests evenly across the time window.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    OriginRegistry                           │
│  ┌─────────────────┐  ┌─────────────────┐                  │
│  │ api.github.com  │  │ api.gitlab.com  │                  │
│  │ OriginLimiter   │  │ OriginLimiter   │                  │
│  │ ┌─────────────┐ │  │ ┌─────────────┐ │                  │
│  │ │ PolicySlot  │ │  │ │ PolicySlot  │ │                  │
│  │ │  burst/60s  │ │  │ │ burst/60s   │ │                  │
│  │ └─────────────┘ │  │ └─────────────┘ │                  │
│  │ ┌─────────────┐ │  │ ┌─────────────┐ │                  │
│  │ │ PolicySlot  │ │  │ │ PolicySlot  │ │                  │
│  │ │ daily/24hr  │ │  │ │ hourly/1hr  │ │                  │
│  │ └─────────────┘ │  │ └─────────────┘ │                  │
│  │ ┌─────────────┐ │  │                 │                  │
│  │ │  Smoother   │ │  │                 │                  │
│  │ │  2s/vel=1.5 │ │  │                 │                  │
│  │ └─────────────┘ │  │                 │                  │
│  └─────────────────┘  └─────────────────┘                  │
└─────────────────────────────────────────────────────────────┘
```

## Configuration

```rust
use reqgov::SmootherConfig;

let config = SmootherConfig {
    micro_interval_secs: 2,  // Divide window into 2-second chunks
    velocity: 1.5,           // Race ahead 33% (finish early)
};
```

| Velocity | Behavior |
|----------|----------|
| 0.5 | Conservative: use half the quota rate |
| 1.0 | Even spread: finish at window end |
| 1.5 | Default: finish 33% early (buffer) |
| 2.0 | Aggressive: finish 50% early |

## Header Parsing

Parses RFC 8941 structured field syntax:

```http
RateLimit-Policy: "burst";q=100;w=60, "daily";q=1000;w=86400
RateLimit: "burst";r=45;t=30, "daily";r=850
```

## License

MIT
