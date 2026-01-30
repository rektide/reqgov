#[cfg(test)]
mod tests {
    use super::*;
    use http::Extensions;
    use std::sync::Arc;

    #[test]
    fn test_rate_limit_span_backend_enriches_span() {
        let span = tracing::Span::current();
        let state = RateLimitState {
            origin: Some("api.example.com".to_string()),
            smoother: Some(SmootherState {
                remaining_per_interval: 1.5,
                micro_interval_secs: 2,
                velocity: 1.5,
                base_window_secs: 60,
            }),
            policies: vec![PolicyState {
                name: "burst".to_string(),
                quota: 100,
                remaining: 75,
                window_secs: 60,
                reset_at: None,
            }],
            will_throttle: true,
            throttle_wait_duration: Some(std::time::Duration::from_secs(2)),
        };

        MinimalSpanBackend.enrich_span(&state);
        let _ = span;
    }

    #[test]
    fn test_standard_span_backend_enriches_span() {
        let span = tracing::Span::current();
        let state = RateLimitState {
            origin: Some("api.github.com".to_string()),
            smoother: Some(SmootherState {
                remaining_per_interval: 2.0,
                micro_interval_secs: 2,
                velocity: 1.5,
                base_window_secs: 60,
            }),
            policies: vec![
                PolicyState {
                    name: "burst".to_string(),
                    quota: 100,
                    remaining: 45,
                    window_secs: 60,
                    reset_at: None,
                },
                PolicyState {
                    name: "daily".to_string(),
                    quota: 10000,
                    remaining: 8500,
                    window_secs: 86400,
                    reset_at: Some(
                        std::time::Instant::now() + std::time::Duration::from_secs(3600),
                    ),
                },
            ],
            will_throttle: false,
            throttle_wait_duration: None,
        };

        StandardSpanBackend.enrich_span(&state);
        let _ = span;
    }

    #[test]
    fn test_detailed_span_backend_enriches_span() {
        let span = tracing::Span::current();
        let reset_time = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let state = RateLimitState {
            origin: Some("api.gitlab.com".to_string()),
            smoother: Some(SmootherState {
                remaining_per_interval: 2.5,
                micro_interval_secs: 3,
                velocity: 2.0,
                base_window_secs: 60,
            }),
            policies: vec![PolicyState {
                name: "burst".to_string(),
                quota: 500,
                remaining: 250,
                window_secs: 300,
                reset_at: Some(reset_time),
            }],
            will_throttle: true,
            throttle_wait_duration: Some(std::time::Duration::from_millis(1500)),
        };

        DetailedSpanBackend.enrich_span(&state);
        let _ = span;
    }

    #[test]
    fn test_noop_span_backend_does_nothing() {
        let state = RateLimitState {
            origin: None,
            smoother: None,
            policies: vec![],
            will_throttle: false,
            throttle_wait_duration: None,
        };

        NoOpSpanBackend.enrich_span(&state);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use http::Extensions;

    /// Integration test demonstrating RateLimitTelemetry middleware
    /// This test shows how RateLimitTelemetry enriches spans created by reqwest-tracing
    #[test]
    fn test_rate_limit_telemetry_enriches_reqwest_tracing_spans() {
        // Create subscriber that captures spans
        let subscriber = tracing_subscriber::fmt()
            .with_test_writer(tracing_subscriber::TestWriter::new())
            .finish();

        tracing::subscriber::set_global_default(subscriber);

        // Create rate limiter
        let rateLimiterState = RateLimitState {
            origin: Some("api.example.com".to_string()),
            smoother: Some(SmootherState {
                remaining_per_interval: 1.5,
                micro_interval_secs: 2,
                velocity: 1.5,
                base_window_secs: 60,
            }),
            policies: vec![PolicyState {
                name: "burst".to_string(),
                quota: 100,
                remaining: 75,
                window_secs: 60,
                reset_at: None,
            }],
            will_throttle: true,
            throttle_wait_duration: Some(std::time::Duration::from_secs(2)),
        };

        // Test that span backends can enrich spans
        // In a real integration, reqwest-tracing would create the span
        // and RateLimitTelemetry would enrich it with governor state

        let span = tracing::Span::current();
        assert!(span.is_none());

        // Create a span and test enrichment
        let span = tracing::span!(test_span, origin = "api.example.com");
        let _guard = span.enter();

        // Simulate what RateLimitTelemetry would do
        StandardSpanBackend.enrich_span(&RateLimiterState);

        let _ = span;
    }

    /// Helper struct for simulating OriginRateLimiter::state() return
    struct RateLimiterState {
        origin: Option<String>,
        smoother: Option<SmootherState>,
        policies: Vec<PolicyState>,
        will_throttle: bool,
        throttle_wait_duration: Option<std::time::Duration>,
    }
}
