mod integration_tests {
    use reqgov::{
        DetailedSpanBackend, MinimalSpanBackend, NoOpSpanBackend, PolicySlotState,
        RateLimitSpanBackend, RateLimitState, SmootherState, StandardSpanBackend,
    };

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
            policies: vec![PolicySlotState {
                name: "burst".to_string(),
                quota: 100,
                remaining: 75,
                window_secs: 60,
                reset_at: None,
            }],
            will_throttle: true,
            throttle_wait_duration: Some(std::time::Duration::from_secs(2)),
            concurrency: None,
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
                PolicySlotState {
                    name: "burst".to_string(),
                    quota: 100,
                    remaining: 45,
                    window_secs: 60,
                    reset_at: None,
                },
                PolicySlotState {
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
            concurrency: None,
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
            policies: vec![PolicySlotState {
                name: "burst".to_string(),
                quota: 500,
                remaining: 250,
                window_secs: 300,
                reset_at: Some(reset_time),
            }],
            will_throttle: true,
            throttle_wait_duration: Some(std::time::Duration::from_millis(1500)),
            concurrency: None,
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
            concurrency: None,
        };

        NoOpSpanBackend.enrich_span(&state);
        // No assertion needed - just verify no panic occurs
    }
}
