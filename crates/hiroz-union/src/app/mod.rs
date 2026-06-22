//! TUI Application module
//!
//! This module contains the App struct and all TUI-related functionality,
//! split into submodules:
//! - `state`: Types, enums, and constants
//! - `render`: UI rendering methods
//! - `input`: Filter and input handling

pub mod input;
pub mod render;
pub mod state;

use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use ratatui::widgets::ScrollbarState;
use rusqlite::Connection;

use crate::core::engine::{Backend, CoreEngine};
use crate::plugin::wasm::WasmPlugin;

pub use state::*;

pub struct App {
    pub core: Arc<CoreEngine>,

    // Core dependencies
    pub session: Arc<zenoh::Session>,
    pub connection_status: ConnectionStatus,
    pub config: Config,
    pub db_conn: Arc<Mutex<Connection>>,
    pub backend: Backend,

    // Panel state
    pub current_panel: Panel,
    pub selected_index: usize,
    pub quit: bool,

    // Focus state
    pub focus_pane: FocusPane,
    pub detail_state: DetailState,

    // Measurement
    pub live_metrics: Arc<Mutex<LiveMetrics>>,

    // Cached graph data for rendering (to reduce lock contention)
    pub cached_topics: Vec<(String, String)>, // (topic_name, type_name)
    pub cached_nodes: Vec<(String, String)>,  // (node_name, namespace)
    pub cached_services: Vec<(String, String)>, // (service_name, type_name)

    pub cache_timestamp: std::time::Instant,

    // Status
    pub status_message: String,
    pub status_message_time: Option<Instant>,
    pub spinner_frame: usize,

    // Screenshot
    pub take_screenshot: bool,

    // Help
    pub show_help: bool,

    // Detail scrolling
    pub detail_scroll: usize,
    pub detail_scroll_state: ScrollbarState,

    // Filter state
    pub filter_mode: bool,
    pub filter_input: String,
    pub filter_cursor: usize,

    // Rate monitoring (lazy approach)
    pub rate_cache: HashMap<String, TopicRateCache>,
    pub rate_cache_ttl: Duration,

    // Multi-topic measurement
    pub measuring_topics: HashSet<String>,
    pub topic_metrics: Arc<Mutex<HashMap<String, TopicMetrics>>>,
    pub multi_subscribers: Vec<zenoh::pubsub::Subscriber<()>>,
    pub measure_selected_index: usize,

    // Recording state
    pub recording_active: bool,
    pub recording_id: Option<i64>,

    // WASM plugin state
    pub wasm_plugins: Vec<WasmPlugin>,
    pub plugin_selected_index: usize,
}

impl App {
    pub async fn new(
        core: Arc<CoreEngine>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Initialize database
        let db_conn = Connection::open("hiroz-metrics.db")?;
        db_conn.execute(
            "CREATE TABLE IF NOT EXISTS metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                msgs_sec REAL,
                bytes_sec REAL,
                avg_payload INTEGER
            )",
            [],
        )?;

        db_conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                payload BLOB,
                size INTEGER
            )",
            [],
        )?;

        // Table for recording measuring topics over time
        db_conn.execute(
            "CREATE TABLE IF NOT EXISTS recordings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                topics TEXT NOT NULL
            )",
            [],
        )?;

        db_conn.execute(
            "CREATE TABLE IF NOT EXISTS recording_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recording_id INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                rate REAL,
                bandwidth REAL,
                FOREIGN KEY (recording_id) REFERENCES recordings(id)
            )",
            [],
        )?;

        let db_conn = Arc::new(Mutex::new(db_conn));

        // Load configuration
        let config = Self::load_config();

        Ok(Self {
            core: core.clone(),
            session: core.session.clone(),
            connection_status: ConnectionStatus::Connected,
            config: config.clone(),
            db_conn,
            backend: core.backend,
            current_panel: Panel::Topics,
            selected_index: 0,
            quit: false,
            focus_pane: FocusPane::List,
            detail_state: DetailState::default(),
            live_metrics: Arc::new(Mutex::new(LiveMetrics::default())),
            cached_topics: Vec::new(),
            cached_nodes: Vec::new(),
            cached_services: Vec::new(),
            cache_timestamp: std::time::Instant::now(),
            status_message: DEFAULT_STATUS_MESSAGE.to_string(),
            status_message_time: None,
            spinner_frame: 0,
            take_screenshot: false,
            show_help: false,
            detail_scroll: 0,
            detail_scroll_state: ScrollbarState::default(),
            filter_mode: false,
            filter_input: String::new(),
            filter_cursor: 0,
            rate_cache: HashMap::new(),
            rate_cache_ttl: Duration::from_secs(config.rate_cache_ttl_seconds),
            measuring_topics: HashSet::new(),
            topic_metrics: Arc::new(Mutex::new(HashMap::new())),
            multi_subscribers: Vec::new(),
            measure_selected_index: 0,
            recording_active: false,
            recording_id: None,
            wasm_plugins: Vec::new(),
            plugin_selected_index: 0,
        })
    }

    fn load_config() -> Config {
        let config_paths = ["hiroz-console.json", ".hiroz-console.json"];

        for path in &config_paths {
            if let Ok(content) = fs::read_to_string(path)
                && let Ok(config) = serde_json::from_str(&content)
            {
                return config;
            }
        }

        Config::default()
    }

    /// Generate key expression for a topic based on the selected backend
    fn topic_key_expr(&self, topic: &str) -> String {
        let topic_name = topic.trim_start_matches('/');

        match self.backend {
            Backend::RmwZenoh => {
                // rmw_zenoh format: <domain_id>/<topic>/**
                format!("{}/{}/**", self.core.domain_id, topic_name)
            }
        }
    }

    pub fn update_graph_cache(&mut self) {
        let graph = self.core.graph.lock();
        self.cached_topics = graph.get_topic_names_and_types();
        self.cached_nodes = graph.get_node_names();
        self.cached_services = graph.get_service_names_and_types();
        self.cache_timestamp = std::time::Instant::now();
    }

    pub fn select_next(&mut self) {
        match self.current_panel {
            Panel::Measure => {
                let max = self.measuring_topics.len();
                if max > 0 && self.measure_selected_index < max - 1 {
                    self.measure_selected_index += 1;
                }
            }
            Panel::Plugins => {
                let max = self.wasm_plugins.len();
                if max > 0 && self.plugin_selected_index < max - 1 {
                    self.plugin_selected_index += 1;
                }
            }
            _ => {
                let max = match self.current_panel {
                    Panel::Topics => self.cached_topics.len(),
                    Panel::Nodes => self.cached_nodes.len(),
                    Panel::Services => self.cached_services.len(),
                    _ => 0,
                };
                if max > 0 && self.selected_index < max - 1 {
                    self.selected_index += 1;
                    self.detail_scroll = 0;
                }
            }
        }
    }

    pub fn select_previous(&mut self) {
        match self.current_panel {
            Panel::Measure => {
                if self.measure_selected_index > 0 {
                    self.measure_selected_index -= 1;
                }
            }
            Panel::Plugins => {
                if self.plugin_selected_index > 0 {
                    self.plugin_selected_index -= 1;
                }
            }
            _ => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                    self.detail_scroll = 0;
                }
            }
        }
    }

    pub fn scroll_detail_up(&mut self) {
        if self.detail_scroll > 0 {
            self.detail_scroll -= 1;
        }
    }

    pub fn scroll_detail_down(&mut self) {
        self.detail_scroll += 1;
    }

    /// Set a temporary status message that will auto-reset after timeout
    pub fn set_temp_status(&mut self, message: String) {
        self.status_message = message;
        self.status_message_time = Some(Instant::now());
    }

    /// Reset status message to default
    pub fn reset_status(&mut self) {
        self.status_message = DEFAULT_STATUS_MESSAGE.to_string();
        self.status_message_time = None;
    }

    /// Check and reset status message if timeout elapsed
    pub fn check_status_timeout(&mut self) {
        if let Some(time) = self.status_message_time
            && time.elapsed() > Duration::from_millis(STATUS_MESSAGE_TIMEOUT_MS)
        {
            self.reset_status();
        }
    }

    pub fn cleanup_rate_cache(&mut self) {
        let now = Instant::now();
        self.rate_cache
            .retain(|_, cache| now.duration_since(cache.last_updated) < self.rate_cache_ttl);
    }

    pub fn export_metrics(
        &self,
        filename: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Export rate cache data
        let mut csv_content = String::from("topic,rate_hz,last_updated_seconds\n");

        for (topic, cache) in &self.rate_cache {
            let last_updated_secs = cache.last_updated.elapsed().as_secs_f64();
            csv_content.push_str(&format!(
                "{},{:.2},{:.1}\n",
                topic, cache.rate, last_updated_secs
            ));
        }

        fs::write(filename, csv_content)?;
        Ok(())
    }

    /// Start recording measuring topics to database
    pub fn start_recording(&mut self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if self.measuring_topics.is_empty() {
            return Err("No topics to record. Add topics with 'm' in Topics tab first.".into());
        }

        let conn = self.db_conn.lock();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        // Store topics as JSON array
        let topics_json = serde_json::to_string(&self.measuring_topics.iter().collect::<Vec<_>>())?;

        conn.execute(
            "INSERT INTO recordings (start_time, topics) VALUES (?1, ?2)",
            rusqlite::params![timestamp, topics_json],
        )?;

        let recording_id = conn.last_insert_rowid();
        drop(conn);

        self.recording_id = Some(recording_id);
        self.recording_active = true;

        Ok(self.measuring_topics.len())
    }

    /// Stop recording and finalize
    pub fn stop_recording(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(recording_id) = self.recording_id {
            let conn = self.db_conn.lock();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;

            conn.execute(
                "UPDATE recordings SET end_time = ?1 WHERE id = ?2",
                rusqlite::params![timestamp, recording_id],
            )?;
        }

        self.recording_active = false;
        self.recording_id = None;
        Ok(())
    }

    /// Toggle recording on/off
    pub fn toggle_recording(&mut self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if self.recording_active {
            self.stop_recording()?;
            Ok("Recording stopped".to_string())
        } else {
            let count = self.start_recording()?;
            Ok(format!("Recording {} topics", count))
        }
    }

    /// Write current metrics sample to database (called during update when recording)
    fn record_sample(&self) {
        if !self.recording_active {
            return;
        }

        let Some(recording_id) = self.recording_id else {
            return;
        };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let metrics = self.topic_metrics.lock();

        if let Some(conn) = self.db_conn.try_lock() {
            for (topic, tm) in metrics.iter() {
                let _ = conn.execute(
                    "INSERT INTO recording_samples (recording_id, timestamp, topic, rate, bandwidth)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        recording_id,
                        timestamp,
                        topic,
                        tm.current_rate,
                        tm.current_bandwidth
                    ],
                );
            }
        }
    }

    pub fn update_metrics(&mut self) {
        // Check if temporary status message should be reset
        self.check_status_timeout();

        // Update connection status from CoreEngine
        use std::sync::atomic::Ordering;
        if self.core.is_connected.load(Ordering::SeqCst) {
            self.connection_status = ConnectionStatus::Connected;
        } else {
            self.connection_status = ConnectionStatus::Disconnected;
        }

        // Cleanup expired rate cache entries periodically
        self.cleanup_rate_cache();

        // Update spinner for progress indicators
        self.spinner_frame = (self.spinner_frame + 1) % 4;

        let now = std::time::Instant::now();
        let window_start = now - std::time::Duration::from_secs(1);

        let mut metrics = self.live_metrics.lock();

        // Calculate 1-second rate
        let (msg_count, byte_sum): (usize, usize) = metrics
            .samples
            .iter()
            .filter(|(time, _)| *time >= window_start)
            .map(|(_, size)| *size)
            .fold((0, 0), |(count, sum), size| (count + 1, sum + size));

        metrics.msgs_sec = msg_count as f64;
        metrics.bytes_sec = byte_sum as f64 / BYTES_PER_KB; // KB/s

        metrics.avg_payload = byte_sum.checked_div(msg_count).unwrap_or(0) as u64;

        // Add to history for sparklines
        let rate_val = metrics.msgs_sec as u64;
        let bw_val = (metrics.bytes_sec * 10.0) as u64; // * 10 to preserve decimal precision

        if metrics.rate_history.len() >= HISTORY_LENGTH {
            metrics.rate_history.pop_front();
        }
        metrics.rate_history.push_back(rate_val);

        if metrics.bandwidth_history.len() >= HISTORY_LENGTH {
            metrics.bandwidth_history.pop_front();
        }
        metrics.bandwidth_history.push_back(bw_val);
    }

    pub async fn quick_measure_rate(
        &mut self,
        topic: &str,
        duration_secs: u64,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let key_expr = self.topic_key_expr(topic);

        let counter = Arc::new(Mutex::new(0usize));
        let counter_clone = counter.clone();

        // Temporary subscriber for rate measurement
        let subscriber = self
            .session
            .declare_subscriber(&key_expr)
            .callback(move |_sample| {
                let mut count = counter_clone.lock();
                *count += 1;
            })
            .await?;

        // Measure for specified duration
        tokio::time::sleep(Duration::from_secs(duration_secs)).await;

        // Calculate rate
        let final_count = *counter.lock();
        let rate = final_count as f64 / duration_secs as f64;

        // Clean up subscriber
        drop(subscriber);

        // Cache the result
        self.rate_cache.insert(
            topic.to_string(),
            TopicRateCache {
                rate,
                last_updated: Instant::now(),
            },
        );

        Ok(rate)
    }

    /// Toggle a topic in the measurement list
    pub async fn toggle_measuring_topic(&mut self, topic: &str) {
        if self.measuring_topics.contains(topic) {
            // Remove from measurement
            self.measuring_topics.remove(topic);
            self.topic_metrics.lock().remove(topic);
            self.set_temp_status(format!("Removed {} from measurement", topic));
        } else {
            // Add to measurement
            self.measuring_topics.insert(topic.to_string());
            self.topic_metrics
                .lock()
                .insert(topic.to_string(), TopicMetrics::default());
            self.set_temp_status(format!("Added {} to measurement", topic));
        }
        // Restart multi-topic monitoring with updated list
        self.restart_multi_monitoring().await;
    }

    /// Clear all topics from measurement
    pub fn clear_measuring_topics(&mut self) {
        self.measuring_topics.clear();
        self.topic_metrics.lock().clear();
        self.stop_multi_monitoring();
        self.set_temp_status("Cleared all measurements".to_string());
    }

    /// Stop multi-topic monitoring
    pub fn stop_multi_monitoring(&mut self) {
        self.multi_subscribers.clear();
    }

    /// Restart multi-topic monitoring with current topic list
    pub async fn restart_multi_monitoring(&mut self) {
        // Stop existing subscribers
        self.stop_multi_monitoring();

        if self.measuring_topics.is_empty() {
            return;
        }

        // Create subscribers for each topic
        for topic in self.measuring_topics.clone() {
            let key_expr = self.topic_key_expr(&topic);

            let metrics = self.topic_metrics.clone();
            let topic_clone = topic.clone();

            if let Ok(subscriber) = self
                .session
                .declare_subscriber(&key_expr)
                .callback(move |sample| {
                    let payload_len = sample.payload().len();
                    let mut m = metrics.lock();
                    if let Some(tm) = m.get_mut(&topic_clone) {
                        tm.msg_count += 1;
                        tm.byte_count += payload_len as u64;
                        tm.last_msg_time = Some(Instant::now());
                    }
                })
                .await
            {
                self.multi_subscribers.push(subscriber);
            }
        }
    }

    /// Update metrics for all monitored topics (called in update loop)
    pub fn update_multi_metrics(&mut self) {
        let mut metrics = self.topic_metrics.lock();

        for (_topic, tm) in metrics.iter_mut() {
            // Raw per-second values
            let instant_rate = tm.msg_count as f64;
            let instant_bw = tm.byte_count as f64 / BYTES_PER_KB;

            // Update history with raw values (for sparkline)
            if tm.rate_history.len() >= HISTORY_LENGTH {
                tm.rate_history.pop_front();
            }
            tm.rate_history.push_back(instant_rate as u64);

            if tm.bandwidth_history.len() >= HISTORY_LENGTH {
                tm.bandwidth_history.pop_front();
            }
            tm.bandwidth_history.push_back((instant_bw * 10.0) as u64);

            // Apply Exponential Moving Average for display values
            // EMA = alpha * new_value + (1 - alpha) * old_value
            tm.samples_collected += 1;
            if tm.samples_collected == 1 {
                // First sample: initialize with instant value
                tm.current_rate = instant_rate;
                tm.current_bandwidth = instant_bw;
            } else {
                // Apply EMA smoothing
                tm.current_rate = EMA_SMOOTHING_FACTOR * instant_rate
                    + (1.0 - EMA_SMOOTHING_FACTOR) * tm.current_rate;
                tm.current_bandwidth = EMA_SMOOTHING_FACTOR * instant_bw
                    + (1.0 - EMA_SMOOTHING_FACTOR) * tm.current_bandwidth;
            }

            // Reset counters for next second
            tm.msg_count = 0;
            tm.byte_count = 0;
        }

        // Release the lock before recording
        drop(metrics);

        // Record sample to database if recording is active
        self.record_sample();
    }

    /// Check if a topic is being measured
    pub fn is_measuring(&self, topic: &str) -> bool {
        self.measuring_topics.contains(topic)
    }

    /// Get context-sensitive status hint based on current focus and panel
    pub fn get_status_hint(&self) -> String {
        match self.current_panel {
            Panel::Measure => {
                if self.measuring_topics.is_empty() {
                    "Go to Topics (1) and press 'm' to add topics | ?:help q:quit".to_string()
                } else if self.recording_active {
                    "j/k:select w:stop recording r:clear | [REC] ?:help q:quit".to_string()
                } else {
                    "j/k:select w:record r:clear | 1-5:panels ?:help q:quit".to_string()
                }
            }
            Panel::Plugins => {
                if self.wasm_plugins.is_empty() {
                    "No WASM plugins found. Set HU_PLUGIN_PATH to a dir with .wasm files | ?:help q:quit".to_string()
                } else {
                    "j/k:select t:tick plugin | 1-5:panels ?:help q:quit".to_string()
                }
            }
            _ => {
                match self.focus_pane {
                    FocusPane::List => {
                        let panel_hints = match self.current_panel {
                            Panel::Topics => "r:rate m:measure",
                            Panel::Services | Panel::Nodes | Panel::Measure | Panel::Plugins => "",
                        };
                        let base = "j/k:nav l:detail Enter:drill-in /:filter";
                        if panel_hints.is_empty() {
                            format!("{} | Tab:panel ?:help q:quit", base)
                        } else {
                            format!("{} {} | Tab:panel ?:help q:quit", base, panel_hints)
                        }
                    }
                    FocusPane::Detail => {
                        match self.current_panel {
                            Panel::Nodes => {
                                // Nodes panel: toggle type name visibility
                                "Enter/Space:show/hide types h:list Esc:back PgUp/Dn:scroll | ?:help"
                                    .to_string()
                            }
                            _ => {
                                // Topics/Services: section navigation with toggle
                                "j/k:sections Enter/Space:toggle h:list Esc:back PgUp/Dn:scroll | ?:help"
                                    .to_string()
                            }
                        }
                    }
                }
            }
        }
    }
}
