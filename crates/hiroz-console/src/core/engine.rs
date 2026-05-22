use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use hiroz::{Builder, context::ZContext, dynamic::DynSub, graph::Graph, node::ZNode};
use parking_lot::Mutex;
use tokio::sync::broadcast;

use super::{events::SystemEvent, metrics::MetricsCollector};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    RmwZenoh,
    Ros2Dds,
}

pub struct CoreEngine {
    pub session: Arc<zenoh::Session>,
    pub graph: Arc<Mutex<Graph>>,
    pub metrics: Arc<Mutex<MetricsCollector>>,
    pub event_tx: broadcast::Sender<SystemEvent>,
    pub domain_id: usize,
    pub router_addr: String,
    pub backend: Backend,
    pub is_connected: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub context: Arc<ZContext>,
    pub node: Arc<ZNode>,
}

impl CoreEngine {
    pub async fn new(
        router_addr: &str,
        domain_id: usize,
        backend: impl Into<Backend>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let backend = backend.into();

        // Initialize Zenoh session in client mode connected to the given router.
        // Client mode is required for correct liveliness propagation with rmw_zenoh_cpp
        // publishers: peer mode with multicast scouting does not reliably see liveliness
        // tokens from rmw_zenoh_cpp nodes connected to the same router.
        let mut config = zenoh::Config::default();
        config.insert_json5("mode", "\"client\"")?;
        config.insert_json5("connect/endpoints", &format!("[\"{}\"]", router_addr))?;
        config.insert_json5("scouting/multicast/enabled", "false")?;

        let session = zenoh::open(config.clone())
            .await
            .map_err(|e| format!("Failed to initialize Zenoh session: {}", e))?;
        let session = Arc::new(session);

        // Initialize graph with backend-specific liveliness pattern and parser
        let format = match backend {
            Backend::RmwZenoh => hiroz_protocol::KeyExprFormat::RmwZenoh,
            Backend::Ros2Dds => hiroz_protocol::KeyExprFormat::Ros2Dds,
        };

        let (_liveliness_pattern, graph) = match backend {
            Backend::RmwZenoh => {
                // RmwZenoh format: @ros2_lv/{domain_id}/**
                let pattern = format!("@ros2_lv/{domain_id}/**");
                tracing::info!("Graph liveliness pattern (RmwZenoh): {}", pattern);
                let fmt = format;
                let g = Graph::new_with_pattern(&session, domain_id, pattern.clone(), move |ke| {
                    fmt.parse_liveliness(ke)
                })?;
                (pattern, g)
            }
            Backend::Ros2Dds => {
                // Ros2Dds format: @/<zenoh_id>/@ros2_lv/**
                let pattern = "@/*/@ros2_lv/**".to_string();
                tracing::info!("Graph liveliness pattern (Ros2Dds): {}", pattern);
                let fmt = format;
                let g = Graph::new_with_pattern(&session, domain_id, pattern.clone(), move |ke| {
                    fmt.parse_liveliness(ke)
                })?;
                (pattern, g)
            }
        };
        let graph = Arc::new(Mutex::new(graph));

        // Create event bus
        let (event_tx, _) = broadcast::channel(1000);

        // Initialize metrics collector
        let metrics = Arc::new(Mutex::new(MetricsCollector::new()));

        // Create ROS context for node creation
        let context = hiroz::context::ZContextBuilder::default()
            .with_domain_id(domain_id)
            .with_zenoh_config(config)
            .build()
            .map_err(|e| format!("Failed to create ROS context: {}", e))?;
        let context = Arc::new(context);

        // Create ROS node with type description service for dynamic subscriptions
        let node = context
            .create_node("hiroz_console")
            .with_type_description_service()
            .build()?;
        let node = Arc::new(node);

        Ok(Self {
            session,
            graph,
            metrics,
            event_tx,
            domain_id,
            router_addr: router_addr.to_string(),
            backend,
            is_connected: Arc::new(AtomicBool::new(true)),
            context,
            node,
        })
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<SystemEvent> {
        self.event_tx.subscribe()
    }

    /// Create a dynamic subscriber for a topic with automatic schema discovery
    ///
    /// # Arguments
    ///
    /// * `topic` - Topic name to subscribe to
    /// * `discovery_timeout` - Maximum time to wait for schema discovery
    ///
    /// # Errors
    ///
    /// Returns error if schema discovery fails or subscriber creation fails
    pub async fn create_dynamic_subscriber(
        &self,
        topic: &str,
        discovery_timeout: Duration,
    ) -> Result<DynSub, Box<dyn std::error::Error + Send + Sync>> {
        self.node
            .create_dyn_sub_auto(topic, discovery_timeout)
            .await
    }

    pub async fn start_monitoring(&self) {
        // Background task: Monitor graph changes
        let graph = self.graph.clone();
        let event_tx = self.event_tx.clone();
        let session = self.session.clone();
        let is_connected = self.is_connected.clone();

        tokio::spawn(async move {
            let mut last_state: HashMap<String, String> = HashMap::new();

            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Check connection status by looking at routers info
                let info = session.info();
                let routers = info.routers_zid().await.count();
                let connected = routers > 0;
                is_connected.store(connected, Ordering::SeqCst);

                let g = graph.lock();
                let current_topics = g.get_topic_names_and_types();

                // Detect new topics
                for (topic, type_name) in &current_topics {
                    if !last_state.contains_key(topic) {
                        let _ = event_tx.send(SystemEvent::TopicDiscovered {
                            topic: topic.clone(),
                            type_name: type_name.clone(),
                            timestamp: SystemTime::now(),
                        });
                    }
                }

                // Detect removed topics
                for topic in last_state.keys() {
                    if !current_topics.iter().any(|(t, _)| t == topic) {
                        let _ = event_tx.send(SystemEvent::TopicRemoved {
                            topic: topic.clone(),
                            timestamp: SystemTime::now(),
                        });
                    }
                }

                last_state = current_topics.into_iter().collect();
            }
        });
    }
}
