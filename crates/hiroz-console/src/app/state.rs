//! Application state types and constants

use std::collections::VecDeque;
use std::time::Instant;

use serde::{Deserialize, Serialize};

// Constants
pub const HISTORY_LENGTH: usize = 60;
pub const HASH_TRUNCATE_LEN: usize = 16;
pub const PAGE_SCROLL_AMOUNT: usize = 10;
pub const LIST_PANE_PERCENTAGE: u16 = 40;
pub const DETAIL_PANE_PERCENTAGE: u16 = 60;
pub const BYTES_PER_KB: f64 = 1024.0;
pub const POLL_TIMEOUT_MS: u64 = 50;
pub const QUICK_MEASURE_DURATION_SECS: u64 = 2;
pub const RATE_THRESHOLD_KHZ: f64 = 1000.0;
pub const RATE_THRESHOLD_HZ: f64 = 100.0;
pub const RATE_THRESHOLD_DHZ: f64 = 10.0;
pub const STATUS_MESSAGE_TIMEOUT_MS: u64 = 2000;
pub const EMA_SMOOTHING_FACTOR: f64 = 0.3; // Higher = more responsive, lower = smoother

pub const DEFAULT_STATUS_MESSAGE: &str = "j/k:nav Enter:drill-in Esc:back r:rate m:measure /:filter Tab:panel e:export w:record ?:help q:quit";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub cache_ttl_seconds: u64,
    pub rate_cache_ttl_seconds: u64,
    pub graph_cache_update_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_ttl_seconds: 30,
            rate_cache_ttl_seconds: 30,
            graph_cache_update_ms: 100,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Panel {
    #[default]
    Topics,
    Services,
    Nodes,
    Measure,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum FocusPane {
    #[default]
    List,
    Detail,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum DetailSection {
    #[default]
    Publishers,
    Subscribers,
    Clients,
}

#[derive(Clone, Debug)]
pub struct TopicRateCache {
    pub rate: f64,
    pub last_updated: Instant,
}

#[derive(Clone, Debug)]
pub struct DetailState {
    pub publishers_expanded: bool,
    pub subscribers_expanded: bool,
    pub clients_expanded: bool,
    pub selected_section: DetailSection,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            publishers_expanded: true,
            subscribers_expanded: true,
            clients_expanded: true,
            selected_section: DetailSection::Publishers,
        }
    }
}

#[derive(Default)]
pub struct LiveMetrics {
    pub msgs_sec: f64,
    pub bytes_sec: f64,
    pub avg_payload: u64,
    pub samples: VecDeque<(Instant, usize)>,
    pub rate_history: VecDeque<u64>, // msg/s history for sparkline
    pub bandwidth_history: VecDeque<u64>, // KB/s * 10 history for sparkline (scaled for precision)
}

#[derive(Clone, Default)]
pub enum ConnectionStatus {
    #[default]
    Connected,
    Disconnected,
}

/// Per-topic metrics for multi-topic monitoring
#[derive(Clone, Default)]
pub struct TopicMetrics {
    pub msg_count: u64,
    pub byte_count: u64,
    pub last_msg_time: Option<Instant>,
    pub rate_history: VecDeque<u64>, // msg/s history (raw per-second counts)
    pub bandwidth_history: VecDeque<u64>, // KB/s * 10 history (raw per-second)
    pub current_rate: f64,           // Smoothed rate (EMA)
    pub current_bandwidth: f64,      // Smoothed bandwidth (EMA)
    pub samples_collected: u64,      // Track if we have enough data for EMA
}
