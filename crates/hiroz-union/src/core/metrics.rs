use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricRecord {
    pub timestamp: u64,
    pub topic: String,
    pub rate_hz: f64,
    pub bandwidth_kbps: f64,
    pub avg_payload_bytes: u64,
}

#[derive(Default)]
pub struct MetricsCollector {
    pub history: Vec<MetricRecord>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }
}
