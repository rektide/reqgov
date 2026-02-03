use crate::origin::policies::{Policy, QuotaUnit, ServiceLimit};
use http::HeaderMap;

#[derive(Debug, Clone)]
pub struct DetectedRateLimits {
    pub policies: Vec<Policy>,
    pub limits: Vec<ServiceLimit>,
}

pub fn detect_rate_limits(headers: &HeaderMap) -> Option<DetectedRateLimits> {
    let remaining = detect_remaining(headers)?;
    let limit = detect_limit(headers)?;
    let reset_secs = detect_reset(headers);

    let window_secs = infer_window_from_reset(&reset_secs);

    Some(DetectedRateLimits {
        policies: vec![Policy {
            name: "default".to_string(),
            quota: limit,
            window_secs,
            quota_unit: QuotaUnit::Requests,
            partition_key: None,
        }],
        limits: vec![ServiceLimit {
            name: "default".to_string(),
            remaining,
            reset_secs,
            partition_key: None,
        }],
    })
}

const REMAINING_HEADERS: &[&str] = &[
    "x-ratelimit-remaining",
    "ratelimit-remaining",
    "x-rate-limit-remaining",
    "rate-limit-remaining",
    "x-ratelimit-requests-remaining",
];

const LIMIT_HEADERS: &[&str] = &[
    "x-ratelimit-limit",
    "ratelimit-limit",
    "x-rate-limit-limit",
    "rate-limit-limit",
    "x-ratelimit-requests-limit",
];

const RESET_HEADERS: &[&str] = &[
    "x-ratelimit-reset",
    "ratelimit-reset",
    "x-rate-limit-reset",
    "rate-limit-reset",
    "x-ratelimit-reset-after",
    "retry-after",
];

fn detect_remaining(headers: &HeaderMap) -> Option<u32> {
    for candidate in REMAINING_HEADERS {
        if let Some(value) = find_header_case_insensitive(headers, candidate) {
            if let Ok(num) = value.parse::<u32>() {
                return Some(num);
            }
        }
    }
    None
}

fn detect_limit(headers: &HeaderMap) -> Option<u32> {
    for candidate in LIMIT_HEADERS {
        if let Some(value) = find_header_case_insensitive(headers, candidate) {
            if let Ok(num) = value.parse::<u32>() {
                return Some(num);
            }
        }
    }
    None
}

fn detect_reset(headers: &HeaderMap) -> Option<u32> {
    for candidate in RESET_HEADERS {
        if let Some(value) = find_header_case_insensitive(headers, candidate) {
            if let Some(secs) = parse_reset_value(&value, candidate) {
                return Some(secs);
            }
        }
    }
    None
}

fn find_header_case_insensitive(headers: &HeaderMap, name: &str) -> Option<String> {
    let normalized = normalize_header_name(name);

    for (key, value) in headers.iter() {
        if normalize_header_name(key.as_str()) == normalized {
            if let Ok(s) = value.to_str() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn normalize_header_name(name: &str) -> String {
    name.to_lowercase()
        .replace('-', "_")
        .trim_start_matches("x_")
        .to_string()
}

fn parse_reset_value(value: &str, header_name: &str) -> Option<u32> {
    if let Ok(ts) = value.parse::<i64>() {
        if ts > 1_000_000_000 {
            let now = chrono::Utc::now().timestamp();
            let reset_secs = (ts - now).max(0) as u32;
            return Some(reset_secs);
        } else if header_name.contains("after") || ts < 86400 {
            return Some(ts as u32);
        }
    }
    None
}

fn infer_window_from_reset(reset_secs: &Option<u32>) -> Option<u32> {
    reset_secs.map(|r| {
        if r <= 60 {
            60
        } else if r <= 3600 {
            3600
        } else {
            86400
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderMap, HeaderName, HeaderValue};

    fn make_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    #[test]
    fn test_github_style_headers() {
        let headers = make_headers(&[
            ("x-ratelimit-remaining", "4999"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", "1704067200"),
        ]);

        let detected = detect_rate_limits(&headers).unwrap();
        assert_eq!(detected.limits[0].remaining, 4999);
        assert_eq!(detected.policies[0].quota, 5000);
    }

    #[test]
    fn test_gitlab_style_headers() {
        let headers = make_headers(&[
            ("ratelimit-remaining", "1999"),
            ("ratelimit-limit", "2000"),
            ("ratelimit-reset", "1704067200"),
        ]);

        let detected = detect_rate_limits(&headers).unwrap();
        assert_eq!(detected.limits[0].remaining, 1999);
        assert_eq!(detected.policies[0].quota, 2000);
    }

    #[test]
    fn test_no_headers_returns_none() {
        let headers = HeaderMap::new();
        assert!(detect_rate_limits(&headers).is_none());
    }

    #[test]
    fn test_incomplete_headers_returns_none() {
        let headers = make_headers(&[("x-ratelimit-remaining", "100")]);
        assert!(detect_rate_limits(&headers).is_none());
    }

    #[test]
    fn test_reset_after_style() {
        let headers = make_headers(&[
            ("x-ratelimit-remaining", "50"),
            ("x-ratelimit-limit", "100"),
            ("x-ratelimit-reset-after", "30"),
        ]);

        let detected = detect_rate_limits(&headers).unwrap();
        assert_eq!(detected.limits[0].reset_secs, Some(30));
    }

    #[test]
    fn test_case_insensitive() {
        let headers = make_headers(&[
            ("X-RateLimit-Remaining", "100"),
            ("X-RATELIMIT-LIMIT", "200"),
            ("X-RATELIMIT-RESET-AFTER", "60"),
        ]);

        let detected = detect_rate_limits(&headers).unwrap();
        assert_eq!(detected.limits[0].remaining, 100);
        assert_eq!(detected.policies[0].quota, 200);
    }
}
