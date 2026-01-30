use std::fmt;

/// Parsed from: `RateLimit-Policy: "burst";q=100;w=60`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub name: String,
    pub quota: u32,
    pub window_secs: Option<u32>,
    pub quota_unit: QuotaUnit,
    pub partition_key: Option<Vec<u8>>,
}

impl fmt::Display for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}/{}s",
            self.name,
            self.quota,
            self.window_secs.unwrap_or(60)
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum QuotaUnit {
    #[default]
    Requests,
    ContentBytes,
    ConcurrentRequests,
}

/// Parsed from: `RateLimit: "burst";r=45;t=55`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLimit {
    pub name: String,
    pub remaining: u32,
    pub reset_secs: Option<u32>,
    pub partition_key: Option<Vec<u8>>,
}

impl fmt::Display for ServiceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} remaining{}",
            self.name,
            self.remaining,
            self.reset_secs
                .map(|t| format!(", resets in {}s", t))
                .unwrap_or_default()
        )
    }
}
