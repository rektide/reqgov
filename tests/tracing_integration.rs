mod integration_tests {
    use reqgov::{
        OriginRateLimiter, Policy, PolicyTracer, SmootherConfig, SmootherTracer, StatusTracer,
    };
    use std::sync::Arc;

    #[test]
    fn test_rate_limit_tracing_creation() {
        let _limiter: Arc<OriginRateLimiter> = Arc::new(OriginRateLimiter::new());
    }

    #[test]
    fn test_policy_tracing_creation() {
        let _tracing = PolicyTracer;
    }

    #[test]
    fn test_smoother_tracing_creation() {
        let _tracing = SmootherTracer;
    }

    #[test]
    fn test_status_tracing_creation() {
        let _tracing = StatusTracer;
    }

    #[test]
    fn test_limiter_with_smoother_for_tracing() {
        let limiter = Arc::new(
            OriginRateLimiter::builder()
                .smoother(SmootherConfig::default())
                .build(),
        );

        let _limiter = limiter;
    }

    #[test]
    fn test_limiter_slots_accessible_for_tracing() {
        let mut limiter = OriginRateLimiter::new();
        limiter.update_policies(vec![Policy {
            name: "burst".to_string(),
            quota: 100,
            window_secs: Some(60),
            quota_unit: reqgov::QuotaUnit::Requests,
            partition_key: None,
        }]);

        let _limiter = limiter;
    }

    #[test]
    fn test_smoother_fields_accessible_for_tracing() {
        let limiter = OriginRateLimiter::builder()
            .smoother(SmootherConfig {
                micro_interval_secs: 5,
                velocity: 2.0,
            })
            .build();

        let _limiter = limiter;
    }
}
