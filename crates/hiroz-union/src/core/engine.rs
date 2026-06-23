use std::{
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
    // Held to keep liveliness tokens alive for the lifetime of CoreEngine.
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

        // Initialize graph with RmwZenoh liveliness pattern
        let format = hiroz_protocol::KeyExprFormat::RmwZenoh;
        let pattern = format!("@ros2_lv/{domain_id}/**");
        tracing::info!("Graph liveliness pattern: {}", pattern);
        let (_liveliness_pattern, graph) = {
            let fmt = format;
            let g = Graph::new_with_pattern(&session, domain_id, pattern.clone(), move |ke| {
                fmt.parse_liveliness(ke)
            })?;
            (pattern, g)
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
            .create_node("hu")
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
        let session = self.session.clone();
        let event_tx = self.event_tx.clone();
        let is_connected = self.is_connected.clone();
        let domain_id = self.domain_id;

        let ke = format!("@ros2_lv/{domain_id}/**");
        let fmt = hiroz_protocol::KeyExprFormat::RmwZenoh;

        // Liveliness subscriber with history so we see all currently-alive tokens on startup.
        let sub = match session
            .liveliness()
            .declare_subscriber(&ke)
            .history(true)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to declare liveliness subscriber: {e}");
                return;
            }
        };

        tokio::spawn(async move {
            // Track connection status via a periodic info check in a separate task.
            let session2 = session.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let connected = session2.info().routers_zid().await.count() > 0;
                    is_connected.store(connected, Ordering::SeqCst);
                }
            });

            while let Ok(sample) = sub.recv_async().await {
                let appeared = sample.kind() == zenoh::sample::SampleKind::Put;
                let Ok(entity) = fmt.parse_liveliness(sample.key_expr()) else {
                    continue;
                };

                let now = SystemTime::now();
                let event = match &entity {
                    hiroz_protocol::Entity::Node(n) => {
                        if appeared {
                            SystemEvent::NodeDiscovered {
                                namespace: n.namespace.clone(),
                                name: n.name.clone(),
                                timestamp: now,
                            }
                        } else {
                            SystemEvent::NodeRemoved {
                                namespace: n.namespace.clone(),
                                name: n.name.clone(),
                                timestamp: now,
                            }
                        }
                    }
                    hiroz_protocol::Entity::Endpoint(ep) => {
                        let type_name = ep
                            .type_info
                            .as_ref()
                            .map(|t| t.name.clone())
                            .unwrap_or_default();
                        match ep.kind {
                            hiroz_protocol::EndpointKind::Publisher
                            | hiroz_protocol::EndpointKind::Subscription => {
                                if appeared {
                                    SystemEvent::TopicDiscovered {
                                        topic: ep.topic.clone(),
                                        type_name,
                                        timestamp: now,
                                    }
                                } else {
                                    SystemEvent::TopicRemoved {
                                        topic: ep.topic.clone(),
                                        timestamp: now,
                                    }
                                }
                            }
                            hiroz_protocol::EndpointKind::Service
                            | hiroz_protocol::EndpointKind::Client => {
                                if appeared {
                                    SystemEvent::ServiceDiscovered {
                                        service: ep.topic.clone(),
                                        type_name,
                                        timestamp: now,
                                    }
                                } else {
                                    continue;
                                }
                            }
                        }
                    }
                };
                let _ = event_tx.send(event);
            }
        });
    }
}
