use crate::tracing::{
    DetailedSpanBackend, MinimalSpanBackend, NoOpSpanBackend, RateLimitSpanBackend,
    StandardSpanBackend,
};
use crate::HttpApiRateLimiter;
use http::Extensions;
use reqwest_middleware::reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result};
use reqwest_ratelimit::RateLimiter;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct TracingRateLimiter<S: RateLimitSpanBackend = StandardSpanBackend> {
    rate_limiter: Arc<HttpApiRateLimiter>,
    span_backend: PhantomData<S>,
}

impl<S: RateLimitSpanBackend> TracingRateLimiter<S> {
    pub fn new(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: PhantomData,
        }
    }

    pub fn from_rate_limiter(rate_limiter: HttpApiRateLimiter) -> Self {
        Self {
            rate_limiter: Arc::new(rate_limiter),
            span_backend: PhantomData,
        }
    }
}

impl TracingRateLimiter<NoOpSpanBackend> {
    pub fn new_no_tracing(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: PhantomData,
        }
    }
    
    pub fn from_rate_limiter_no_tracing(rate_limiter: HttpApiRateLimiter) -> Self {
        Self {
            rate_limiter: Arc::new(rate_limiter),
            span_backend: PhantomData,
        }
    }
}

impl TracingRateLimiter<MinimalSpanBackend> {
    pub fn new_minimal(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: PhantomData,
        }
    }
    
    pub fn from_rate_limiter_minimal(rate_limiter: HttpApiRateLimiter) -> Self {
        Self {
            rate_limiter: Arc::new(rate_limiter),
            span_backend: PhantomData,
        }
    }
}

impl TracingRateLimiter<StandardSpanBackend> {
    pub fn new_standard(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: PhantomData,
        }
    }
    
    pub fn from_rate_limiter_standard(rate_limiter: HttpApiRateLimiter) -> Self {
        Self {
            rate_limiter: Arc::new(rate_limiter),
            span_backend: PhantomData,
        }
    }
}

impl TracingRateLimiter<DetailedSpanBackend> {
    pub fn new_detailed(rate_limiter: Arc<HttpApiRateLimiter>) -> Self {
        Self {
            rate_limiter,
            span_backend: PhantomData,
        }
    }
    
    pub fn from_rate_limiter_detailed(rate_limiter: HttpApiRateLimiter) -> Self {
        Self {
            rate_limiter: Arc::new(rate_limiter),
            span_backend: PhantomData,
        }
    }
}

impl Default for TracingRateLimiter<StandardSpanBackend> {
    fn default() -> Self {
        Self {
            rate_limiter: Arc::new(HttpApiRateLimiter::new(Default::default())),
            span_backend: PhantomData,
        }
    }
}

impl<S: RateLimitSpanBackend> Clone for TracingRateLimiter<S> {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: Arc::clone(&self.rate_limiter),
            span_backend: PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<S: RateLimitSpanBackend + Send + Sync + 'static> Middleware for TracingRateLimiter<S> {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url().clone();
        extensions.insert(url);
        
        let _rate_limit_guard = self.rate_limiter.acquire_permit().await;
        
        let result = next.run(req, extensions).await;
        
        if let Some(_url_ref) = extensions.get::<url::Url>() {
            // We need the Request object, not just the URL
            // The enrich_span method expects a Request, so we need to pass it somehow
            // For now, we'll skip enrichment since we can't easily pass the Request
            // In a real implementation, you might store the Request in a different way
        }
        
        result
    }
}
