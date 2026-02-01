use crate::policies::slot::{PolicySlot, PolicySlotState};
use crate::smoothing::smoother::{Smoother, SmootherState};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CheckMetrics {
    pub duration: Duration,
    pub passed: bool,
    pub wait_duration: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateMode {
    Historical,
    Fresh,
}

#[derive(Debug, Clone)]
pub struct SpanMetadata {
    pub timestamp: Instant,
    pub duration: Duration,
    pub mode: StateMode,
}

#[derive(Debug, Clone, Default)]
pub struct ConcurrencyMetrics {
    pub global_active: Option<usize>,
    pub global_max: Option<usize>,
    pub domain_active: Option<usize>,
    pub domain_max: Option<usize>,
    pub wait_duration: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct SpanExtensions {
    pub smoother_state: Option<SmootherState>,
    pub policy_states: Vec<(String, PolicySlotState)>,
    pub attributes: HashMap<&'static str, AttributeValue>,
    pub concurrency: ConcurrencyMetrics,
}

impl SpanExtensions {
    pub fn new() -> Self {
        Self {
            smoother_state: None,
            policy_states: Vec::new(),
            attributes: HashMap::new(),
            concurrency: ConcurrencyMetrics::default(),
        }
    }

    pub fn set(&mut self, key: &'static str, value: AttributeValue) {
        self.attributes.insert(key, value);
    }
}

#[derive(Debug, Clone)]
pub enum AttributeValue {
    Bool(bool),
    U64(u64),
    I64(i64),
    Str(String),
    Float(f64),
    Duration(Duration),
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        AttributeValue::Bool(value)
    }
}

impl From<u64> for AttributeValue {
    fn from(value: u64) -> Self {
        AttributeValue::U64(value)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        AttributeValue::I64(value)
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        AttributeValue::Str(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        AttributeValue::Str(value.to_string())
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        AttributeValue::Float(value)
    }
}

impl From<Duration> for AttributeValue {
    fn from(value: Duration) -> Self {
        AttributeValue::Duration(value)
    }
}

#[derive(Debug, Clone)]
pub struct SpanContext {
    pub all_passed: bool,
    pub limiting_policy: Option<String>,
    pub total_duration: Duration,
    pub smoother_metrics: Option<CheckMetrics>,
    pub policy_metrics: Vec<(String, CheckMetrics)>,
    pub extensions: SpanExtensions,
    pub metadata: SpanMetadata,
}
