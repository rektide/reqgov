use crate::origin::header_detector::detect_rate_limits;
use crate::origin::origin_limiter::OriginLimiter;
use crate::origin::parsing::parse_limit_header;
use crate::origin::parsing::parse_policy_header;
use http::Extensions;
use reqwest_middleware::{Middleware, Next, Result};
use std::sync::Arc;

pub struct ResponseAdapter;

#[async_trait::async_trait]
impl Middleware for ResponseAdapter {
    async fn handle(
        &self,
        req: reqwest_middleware::reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<reqwest_middleware::reqwest::Response> {
        let response = next.run(req, extensions).await?;

        if let Some(limiter) = extensions.get::<Arc<OriginLimiter>>() {
            let headers = response.headers();

            let detected_policies = parse_policy_header(headers);
            let detected_limits = parse_limit_header(headers);

            let slots = limiter.slots.lock().await;

            let has_policies = !slots.is_empty();
            drop(slots);

            if has_policies {
                if let Some(policies) = detected_policies {
                    limiter.update_policies(policies).await;
                }
                if let Some(limits) = detected_limits {
                    limiter.update_limits(limits).await;
                }
            } else if let Some(detected) = detect_rate_limits(headers) {
                limiter.update_policies(detected.policies).await;
                limiter.update_limits(detected.limits).await;
            }
        }

        Ok(response)
    }
}
