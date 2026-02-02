use url::Url;

/// Extract origin key (scheme + host) from a URL.
///
/// This creates a standardized identifier for rate limiting based on the
/// scheme (http/https) and hostname, which is useful for grouping requests
/// by origin/domain.
///
/// # Arguments
/// * `url` - The URL to extract the origin from
///
/// # Returns
/// A string in the format "scheme://host" (e.g., "https://api.example.com")
///
/// # Examples
/// ```
/// # use reqgov::origin_key;
/// # use url::Url;
/// let url = Url::parse("https://api.example.com/path").unwrap();
/// assert_eq!(origin_key(&url), "https://api.example.com");
/// ```
pub fn origin_key(url: &Url) -> String {
    format!("{}://{}", url.scheme(), url.host_str().unwrap_or("unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_origin_key_https() {
        let url = Url::parse("https://api.example.com/path").unwrap();
        let key = origin_key(&url);
        assert_eq!(key, "https://api.example.com");
    }

    #[test]
    fn test_origin_key_http() {
        let url = Url::parse("http://example.com").unwrap();
        let key = origin_key(&url);
        assert_eq!(key, "http://example.com");
    }

    #[test]
    fn test_origin_key_with_port() {
        let url = Url::parse("https://localhost:8080").unwrap();
        let key = origin_key(&url);
        assert_eq!(key, "https://localhost");
    }

    #[test]
    fn test_origin_key_unknown_host() {
        let url = Url::parse("file:///path/to/file").unwrap();
        let key = origin_key(&url);
        assert_eq!(key, "file://unknown");
    }
}
