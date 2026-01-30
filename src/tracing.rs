use http::Extensions;
use reqwest_middleware::reqwest::{Request, Response};
use reqwest_middleware::Result;
use tracing::Span;

pub use crate::rate_limit_span::NoOpSpanBackend;
pub use crate::rate_limit_span::RateLimitSpanBackend;

/// Extension type to override tracing verbosity for a specific request
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TracingVerbosity {
    Minimal,
    Standard,
    Detailed,
}

impl Default for TracingVerbosity {
    fn default() -> Self {
        Self::Standard
    }
}

pub struct MinimalSpanBackend;

impl RateLimitSpanBackend for MinimalSpanBackend {
    fn enrich_span(_req: &Request, outcome: &Result<Response>, _extension: &Extensions) {
        let span = Span::current();
        
        if let Ok(res) = outcome {
            span.record("rate_limit.http_status", res.status().as_u16());
        }
    }
}

pub struct StandardSpanBackend;

impl RateLimitSpanBackend for StandardSpanBackend {
    fn enrich_span(req: &Request, outcome: &Result<Response>, _extension: &Extensions) {
        let span = Span::current();
        
        let url = req.url();
        span.record("rate_limit.scheme", &*url.scheme().to_string());
        span.record("rate_limit.host", url.host_str().unwrap_or(""));
        
        if let Ok(res) = outcome {
            span.record("rate_limit.http_status", res.status().as_u16());
            
            if let Some(headers) = res.headers().get("x-ratelimit-remaining") {
                if let Ok(remaining) = headers.to_str() {
                    span.record("rate_limit.x_ratelimit_remaining", remaining);
                }
            }
            if let Some(headers) = res.headers().get("RateLimit-Remaining") {
                if let Ok(remaining) = headers.to_str() {
                    span.record("rate_limit.ratelimit_remaining", remaining);
                }
            }
        }
    }
}

pub struct DetailedSpanBackend;

impl RateLimitSpanBackend for DetailedSpanBackend {
    fn enrich_span(req: &Request, outcome: &Result<Response>, _extension: &Extensions) {
        let span = Span::current();
        
        let url = req.url();
        span.record("rate_limit.scheme", &*url.scheme().to_string());
        span.record("rate_limit.host", url.host_str().unwrap_or(""));
        span.record("rate_limit.path", url.path());
        
        if let Ok(res) = outcome {
            span.record("rate_limit.http_status", res.status().as_u16());
            
            let headers = res.headers();
            
            if let Some(h) = headers.get("x-ratelimit-remaining") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.x_ratelimit_remaining", v); }
            }
            if let Some(h) = headers.get("x-ratelimit-limit") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.x_ratelimit_limit", v); }
            }
            if let Some(h) = headers.get("x-ratelimit-reset") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.x_ratelimit_reset", v); }
            }
            if let Some(h) = headers.get("x-ratelimit-used") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.x_ratelimit_used", v); }
            }
            if let Some(h) = headers.get("x-ratelimit-resource") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.x_ratelimit_resource", v); }
            }
            
            if let Some(h) = headers.get("RateLimit-Remaining") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.ratelimit_remaining", v); }
            }
            if let Some(h) = headers.get("RateLimit-Limit") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.ratelimit_limit", v); }
            }
            if let Some(h) = headers.get("RateLimit-Reset") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.ratelimit_reset", v); }
            }
            
            if let Some(h) = headers.get("retry-after") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.retry_after", v); }
            }
            
            if let Some(h) = headers.get("RateLimit-Policy") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.ratelimit_policy", v); }
            }
            if let Some(h) = headers.get("RateLimit") {
                if let Ok(v) = h.to_str() { span.record("rate_limit.ratelimit", v); }
            }
        }
        
        if let Err(e) = outcome {
            span.record("rate_limit.error", e.to_string());
        }
    }
}
    }
}

pub struct StandardSpanBackend;

impl RateLimitSpanBackend for StandardSpanBackend {
    fn enrich_span(req: &Request, outcome: &Result<Response>, extension: &Extensions) {
        let span = Span::current();

        let url = req.url();
        span.record("rate_limit.scheme", url.scheme().as_str());
        span.record("rate_limit.host", url.host_str().unwrap_or(""));

        if let Ok(res) = outcome {
            span.record("rate_limit.http_status", res.status().as_u16());

            if let Some(headers) = res.headers().get("x-ratelimit-remaining") {
                if let Ok(remaining) = headers.to_str() {
                    span.record("rate_limit.x_ratelimit_remaining", remaining);
                }
            }
            if let Some(headers) = res.headers().get("RateLimit-Remaining") {
                if let Ok(remaining) = headers.to_str() {
                    span.record("rate_limit.ratelimit_remaining", remaining);
                }
            }
        }
    }
}

pub struct DetailedSpanBackend;

impl RateLimitSpanBackend for DetailedSpanBackend {
    fn enrich_span(req: &Request, outcome: &Result<Response>, extension: &Extensions) {
        let span = Span::current();

        let url = req.url();
        span.record("rate_limit.scheme", url.scheme().as_str());
        span.record("rate_limit.host", url.host_str().unwrap_or(""));
        span.record("rate_limit.path", url.path());

        if let Ok(res) = outcome {
            span.record("rate_limit.http_status", res.status().as_u16());

            let headers = res.headers();

            if let Some(h) = headers.get("x-ratelimit-remaining") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.x_ratelimit_remaining", v);
                }
            }
            if let Some(h) = headers.get("x-ratelimit-limit") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.x_ratelimit_limit", v);
                }
            }
            if let Some(h) = headers.get("x-ratelimit-reset") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.x_ratelimit_reset", v);
                }
            }
            if let Some(h) = headers.get("x-ratelimit-used") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.x_ratelimit_used", v);
                }
            }
            if let Some(h) = headers.get("x-ratelimit-resource") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.x_ratelimit_resource", v);
                }
            }

            if let Some(h) = headers.get("RateLimit-Remaining") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.ratelimit_remaining", v);
                }
            }
            if let Some(h) = headers.get("RateLimit-Limit") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.ratelimit_limit", v);
                }
            }
            if let Some(h) = headers.get("RateLimit-Reset") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.ratelimit_reset", v);
                }
            }

            if let Some(h) = headers.get("retry-after") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.retry_after", v);
                }
            }

            if let Some(h) = headers.get("RateLimit-Policy") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.ratelimit_policy", v);
                }
            }
            if let Some(h) = headers.get("RateLimit") {
                if let Ok(v) = h.to_str() {
                    span.record("rate_limit.ratelimit", v);
                }
            }
        }

        if let Err(e) = outcome {
            span.record("rate_limit.error", e.to_string());
        }
    }
}

#[macro_export]
macro_rules! rate_limit_span_fields {
    () => {{
        use $crate::tracing::SpanExt;
        $crate::rate_limit_span!()
    }};
    (minimal) => {{
        use $crate::tracing::SpanExt;
        $crate::rate_limit_span!(rate_limit.http_status = tracing::field::Empty)
    }};
    (standard) => {{
        use $crate::tracing::SpanExt;
        $crate::rate_limit_span!(
            rate_limit.scheme = tracing::field::Empty,
            rate_limit.host = tracing::field::Empty,
            rate_limit.http_status = tracing::field::Empty,
            rate_limit.x_ratelimit_remaining = tracing::field::Empty,
            rate_limit.ratelimit_remaining = tracing::field::Empty
        )
    }};
    (detailed) => {{
        use $crate::tracing::SpanExt;
        $crate::rate_limit_span!(
            rate_limit.scheme = tracing::field::Empty,
            rate_limit.host = tracing::field::Empty,
            rate_limit.path = tracing::field::Empty,
            rate_limit.http_status = tracing::field::Empty,
            rate_limit.x_ratelimit_remaining = tracing::field::Empty,
            rate_limit.x_ratelimit_limit = tracing::field::Empty,
            rate_limit.x_ratelimit_reset = tracing::field::Empty,
            rate_limit.x_ratelimit_used = tracing::field::Empty,
            rate_limit.x_ratelimit_resource = tracing::field::Empty,
            rate_limit.ratelimit_remaining = tracing::field::Empty,
            rate_limit.ratelimit_limit = tracing::field::Empty,
            rate_limit.ratelimit_reset = tracing::field::Empty,
            rate_limit.retry_after = tracing::field::Empty,
            rate_limit.ratelimit_policy = tracing::field::Empty,
            rate_limit.ratelimit = tracing::field::Empty,
            rate_limit.error = tracing::field::Empty
        )
    }};
}
