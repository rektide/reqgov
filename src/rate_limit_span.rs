use http::Extensions;
use reqwest_middleware::reqwest::{Request, Response};
use reqwest_middleware::Result;

pub trait RateLimitSpanBackend: Send + Sync {
    fn enrich_span(req: &Request, outcome: &Result<Response>, extension: &Extensions);
}

#[macro_export]
macro_rules! rate_limit_span {
    ($($field:tt)*) => {
        tracing::span!(
            tracing::Level::INFO,
            "rate_limit_check",
            $($field)*
        )
    };
}

pub struct NoOpSpanBackend;

impl RateLimitSpanBackend for NoOpSpanBackend {
    fn enrich_span(_req: &Request, _outcome: &Result<Response>, _extension: &Extensions) {}
}
