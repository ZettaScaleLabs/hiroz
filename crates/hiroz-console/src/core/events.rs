use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SystemEvent {
    TopicDiscovered {
        topic: String,
        type_name: String,
        timestamp: SystemTime,
    },
    TopicRemoved {
        topic: String,
        timestamp: SystemTime,
    },
    RateMeasured {
        topic: String,
        rate_hz: f64,
        bandwidth_kbps: f64,
        timestamp: SystemTime,
    },
    NodeDiscovered {
        namespace: String,
        name: String,
        timestamp: SystemTime,
    },
    NodeRemoved {
        namespace: String,
        name: String,
        timestamp: SystemTime,
    },
    ServiceDiscovered {
        service: String,
        type_name: String,
        timestamp: SystemTime,
    },
    MetricsSnapshot {
        total_topics: usize,
        total_nodes: usize,
        total_services: usize,
        timestamp: SystemTime,
    },
}

impl SystemEvent {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn to_human_readable(&self) -> String {
        match self {
            Self::TopicDiscovered {
                topic, type_name, ..
            } => {
                format!("Topic discovered: {} ({})", topic, type_name)
            }
            Self::TopicRemoved { topic, .. } => {
                format!("Topic removed: {}", topic)
            }
            Self::RateMeasured {
                topic,
                rate_hz,
                bandwidth_kbps,
                ..
            } => {
                format!(
                    "Topic {} rate: {:.1} Hz, {:.2} KB/s",
                    topic, rate_hz, bandwidth_kbps
                )
            }
            Self::NodeDiscovered {
                namespace, name, ..
            } => {
                format!("Node discovered: {}/{}", namespace, name)
            }
            Self::NodeRemoved {
                namespace, name, ..
            } => {
                format!("Node removed: {}/{}", namespace, name)
            }
            Self::ServiceDiscovered {
                service, type_name, ..
            } => {
                format!("Service discovered: {} ({})", service, type_name)
            }
            Self::MetricsSnapshot {
                total_topics,
                total_nodes,
                total_services,
                ..
            } => {
                format!(
                    "Snapshot: {} topics, {} nodes, {} services",
                    total_topics, total_nodes, total_services
                )
            }
        }
    }
}
