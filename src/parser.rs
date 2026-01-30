use crate::policy::{Policy, QuotaUnit, ServiceLimit};
use http::HeaderMap;

/// Parse `RateLimit-Policy: "name";q=100;w=60`
pub fn parse_policy_header(headers: &HeaderMap) -> Option<Vec<Policy>> {
    let value = headers.get("ratelimit-policy")?.to_str().ok()?;
    Some(parse_structured_list(value, parse_policy_item))
}

/// Parse `RateLimit: "name";r=45;t=55`
pub fn parse_limit_header(headers: &HeaderMap) -> Option<Vec<ServiceLimit>> {
    let value = headers.get("ratelimit")?.to_str().ok()?;
    Some(parse_structured_list(value, parse_limit_item))
}

fn parse_structured_list<T, F>(value: &str, parse_item: F) -> Vec<T>
where
    F: Fn(&str) -> Option<T>,
{
    value
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|item| parse_item(item))
        .collect()
}

fn parse_policy_item(item: &str) -> Option<Policy> {
    let mut parts = item.split(';');
    let name = parts.next()?.trim().trim_matches('"').to_string();

    let mut policy = Policy {
        name,
        quota: 0,
        window_secs: None,
        quota_unit: QuotaUnit::default(),
        partition_key: None,
    };

    for param in parts {
        let param = param.trim();
        if let Some((key, value)) = param.split_once('=') {
            match key.trim() {
                "q" => policy.quota = value.parse().ok()?,
                "w" => policy.window_secs = value.parse().ok(),
                "qu" => policy.quota_unit = parse_quota_unit(value),
                "pk" => policy.partition_key = parse_base64(value),
                _ => {}
            }
        }
    }

    Some(policy)
}

fn parse_limit_item(item: &str) -> Option<ServiceLimit> {
    let mut parts = item.split(';');
    let name = parts.next()?.trim().trim_matches('"').to_string();

    let mut limit = ServiceLimit {
        name,
        remaining: 0,
        reset_secs: None,
        partition_key: None,
    };

    for param in parts {
        let param = param.trim();
        if let Some((key, value)) = param.split_once('=') {
            match key.trim() {
                "r" => limit.remaining = value.parse().ok()?,
                "t" => limit.reset_secs = value.parse().ok(),
                "pk" => limit.partition_key = parse_base64(value),
                _ => {}
            }
        }
    }

    Some(limit)
}

fn parse_quota_unit(value: &str) -> QuotaUnit {
    let value = value.trim().trim_matches('"').to_lowercase();
    match value.as_str() {
        "requests" => QuotaUnit::Requests,
        "content-bytes" => QuotaUnit::ContentBytes,
        "concurrent-requests" => QuotaUnit::ConcurrentRequests,
        _ => QuotaUnit::Requests,
    }
}

fn parse_base64(value: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_policy_single() {
        let input = r#""burst";q=100;w=60"#;
        let result = parse_policy_item(input).unwrap();
        assert_eq!(result.name, "burst");
        assert_eq!(result.quota, 100);
        assert_eq!(result.window_secs, Some(60));
    }

    #[test]
    fn test_parse_policy_with_quota_unit() {
        let input = r#""daily";q=1000;w=86400;qu="requests""#;
        let result = parse_policy_item(input).unwrap();
        assert_eq!(result.name, "daily");
        assert_eq!(result.quota, 1000);
        assert_eq!(result.window_secs, Some(86400));
        assert_eq!(result.quota_unit, QuotaUnit::Requests);
    }

    #[test]
    fn test_parse_limit_single() {
        let input = r#""burst";r=45;t=30"#;
        let result = parse_limit_item(input).unwrap();
        assert_eq!(result.name, "burst");
        assert_eq!(result.remaining, 45);
        assert_eq!(result.reset_secs, Some(30));
    }

    #[test]
    fn test_parse_structured_list() {
        let input = r#""burst";q=100;w=60, "daily";q=1000;w=86400"#;
        let result = parse_structured_list(input, parse_policy_item);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "burst");
        assert_eq!(result[1].name, "daily");
    }
}
