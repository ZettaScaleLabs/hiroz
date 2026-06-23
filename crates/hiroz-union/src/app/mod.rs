//! TUI Application module
//!
//! This module contains the App struct and all TUI-related functionality,
//! split into submodules:
//! - `state`: Types, enums, and constants
//! - `render`: UI rendering methods
//! - `input`: Filter and input handling

pub mod input;
pub mod monitor;
pub mod plugins;
pub mod recorder;
pub mod render;
pub mod state;

use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

use ratatui::widgets::ScrollbarState;

use crate::core::engine::CoreEngine;
use monitor::TopicMonitor;
use plugins::PluginManager;
use recorder::Recorder;

pub use state::*;

pub struct App {
    pub core: Arc<CoreEngine>,

    // Core dependencies
    pub connection_status: ConnectionStatus,
    pub config: Config,
    pub recorder: Recorder,

    // Panel state
    pub current_panel: Panel,
    pub selected_index: usize,

    // Focus state
    pub focus_pane: FocusPane,
    pub detail_state: DetailState,

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

    // Multi-topic measurement
    pub monitor: TopicMonitor,
    pub measure_selected_index: usize,

    // WASM plugin state
    pub plugin_mgr: PluginManager,
}

impl App {
    pub async fn new(
        core: Arc<CoreEngine>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let recorder = Recorder::open("hiroz-metrics.db")?;

        // Load configuration
        let config = Self::load_config();

        let monitor = TopicMonitor::new(
            core.session.clone(),
            core.domain_id,
            core.backend,
            Duration::from_secs(config.rate_cache_ttl_seconds),
        );

        Ok(Self {
            core: core.clone(),
            connection_status: ConnectionStatus::Connected,
            config: config.clone(),
            recorder,
            current_panel: Panel::Topics,
            selected_index: 0,
            focus_pane: FocusPane::List,
            detail_state: DetailState::default(),
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
            monitor,
            measure_selected_index: 0,
            plugin_mgr: PluginManager::new(),
        })
    }

    fn load_config() -> Config {
        // Legacy hiroz-console.json is accepted as a deprecated fallback.
        let config_paths = [
            "hu.json",
            ".hu.json",
            "hiroz-console.json",
            ".hiroz-console.json",
        ];

        for path in &config_paths {
            if let Ok(content) = fs::read_to_string(path)
                && let Ok(config) = serde_json::from_str(&content)
            {
                return config;
            }
        }

        Config::default()
    }

    pub fn plugin_count(&self) -> usize {
        self.plugin_mgr.count()
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
                let max = self.monitor.measuring_topics.len();
                if max > 0 && self.measure_selected_index < max - 1 {
                    self.measure_selected_index += 1;
                }
            }
            Panel::Plugins => {
                self.plugin_mgr.select_next();
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
                self.plugin_mgr.select_prev();
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

    pub fn export_metrics(
        &self,
        filename: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut csv_content = String::from("topic,rate_hz,last_updated_seconds\n");
        for (topic, cache) in &self.monitor.rate_cache {
            let last_updated_secs = cache.last_updated.elapsed().as_secs_f64();
            csv_content.push_str(&format!(
                "{},{:.2},{:.1}\n",
                topic, cache.rate, last_updated_secs
            ));
        }
        fs::write(filename, csv_content)?;
        Ok(())
    }

    /// Toggle recording on/off
    pub fn toggle_recording(&mut self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let topics: Vec<String> = self.monitor.measuring_topics.iter().cloned().collect();
        self.recorder.toggle(&topics)
    }

    fn record_sample(&self) {
        if !self.recorder.is_active() {
            return;
        }
        let metrics = self.monitor.topic_metrics.lock();
        for (topic, tm) in metrics.iter() {
            self.recorder
                .record_sample(topic, tm.current_rate, tm.current_bandwidth);
        }
    }

    pub fn update_metrics(&mut self) {
        self.check_status_timeout();
        use std::sync::atomic::Ordering;
        if self.core.is_connected.load(Ordering::SeqCst) {
            self.connection_status = ConnectionStatus::Connected;
        } else {
            self.connection_status = ConnectionStatus::Disconnected;
        }
        self.monitor.cleanup_rate_cache();
        self.spinner_frame = (self.spinner_frame + 1) % 4;
    }

    pub async fn quick_measure_rate(
        &mut self,
        topic: &str,
        duration_secs: u64,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let rate = self
            .monitor
            .quick_measure_rate(topic, Duration::from_secs(duration_secs))
            .await?;
        self.monitor.set_rate_cache(topic, rate);
        Ok(rate)
    }

    pub async fn toggle_measuring_topic(&mut self, topic: &str) {
        if self.monitor.measuring_topics.contains(topic) {
            self.monitor.remove_topic(topic).await;
            self.set_temp_status(format!("Removed {} from measurement", topic));
        } else {
            self.monitor.add_topic(topic).await;
            self.set_temp_status(format!("Added {} to measurement", topic));
        }
    }

    pub fn clear_measuring_topics(&mut self) {
        self.monitor.clear();
        self.set_temp_status("Cleared all measurements".to_string());
    }

    pub fn update_multi_metrics(&mut self) {
        self.monitor.update();
        self.record_sample();
    }

    pub fn is_measuring(&self, topic: &str) -> bool {
        self.monitor.measuring_topics.contains(topic)
    }

    /// Get context-sensitive status hint based on current focus and panel
    pub fn get_status_hint(&self) -> String {
        match self.current_panel {
            Panel::Measure => {
                if self.monitor.measuring_topics.is_empty() {
                    "Go to Topics (1) and press 'm' to add topics | ?:help q:quit".to_string()
                } else if self.recorder.is_active() {
                    "j/k:select w:stop recording r:clear | [REC] ?:help q:quit".to_string()
                } else {
                    "j/k:select w:record r:clear | 1-5:panels ?:help q:quit".to_string()
                }
            }
            Panel::Plugins => {
                if self.plugin_count() == 0 && self.plugin_mgr.failed.is_empty() {
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
