use parking_lot::Mutex;
use std::sync::Arc;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

use hu::plugin::types::{EventKind, Permission};

use crate::core::message_formatter::dynamic_message_to_json;
use anyhow::{Context, Result};
use hiroz::dynamic::{
    DynamicMessage, DynamicValue, FieldType, MessageSchema, get_schema,
    serialization::{deserialize_cdr, serialize_cdr},
};
use hiroz_protocol::{EndpointKind, Entity};
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker, Resource, bindgen},
};
use wasmtime_wasi::{WasiCtxBuilder, WasiCtxView, WasiView};
use zenoh::Wait;

use crate::core::engine::CoreEngine;

bindgen!({
    world: "hu-plugin",
    path: "wit/hu-plugin.wit",
});

// ─── Subscription tracking (ros interface) ────────────────────────────────────

struct SubscriptionData {
    // Retained for future debug logging / plugin introspection; not read today.
    #[allow(dead_code)]
    topic: String,
    rx: flume::Receiver<String>,
    _abort: tokio::task::AbortHandle,
}

// ─── Raw transport resource state ────────────────────────────────────────────

struct RawSubData {
    rx: flume::Receiver<Vec<u8>>,
    _abort: tokio::task::AbortHandle,
}

/// Publisher stored as (session, key_expr) to avoid zenoh lifetime issues.
struct RawPubData {
    session: Arc<zenoh::Session>,
    ke: String,
}

/// Liveliness token: a task holds the token; aborting the task undeclares it.
struct LivelinessTokenData {
    _abort: tokio::task::AbortHandle,
}

struct LivelinessSubData {
    rx: flume::Receiver<(String, bool)>,
    _abort: tokio::task::AbortHandle,
}

/// Queryable: pending map shared between the background task and the host impl.
struct QueryableData {
    rx: flume::Receiver<(u64, Vec<u8>)>,
    pending: Arc<Mutex<HashMap<u64, zenoh::query::Query>>>,
    _abort: tokio::task::AbortHandle,
}

/// Querier stored as (session, key_expr); every call uses session.get().
struct QuerierData {
    session: Arc<zenoh::Session>,
    ke: String,
}

/// Per-topic rate/bandwidth tracker for measure-hz and measure-bw.
/// The background subscriber pushes (arrival_time, byte_count) through a channel.
struct RateTrackerData {
    rx: flume::Receiver<(Instant, usize)>,
    arrivals: VecDeque<(Instant, usize)>,
    _abort: tokio::task::AbortHandle,
}

impl RateTrackerData {
    fn drain_and_trim(&mut self, window_ms: u32) {
        while let Ok(sample) = self.rx.try_recv() {
            self.arrivals.push_back(sample);
        }
        let cutoff = Instant::now() - Duration::from_millis(window_ms as u64);
        while self.arrivals.front().is_some_and(|(t, _)| *t < cutoff) {
            self.arrivals.pop_front();
        }
    }
}

/// Service client for ROS service calls via the ros interface.
struct ServiceClientData {
    session: Arc<zenoh::Session>,
    /// Zenoh key expression targeting the service queryable.
    ke: String,
    /// ROS type name of the request type for schema lookup.
    type_name: String,
}

// ─── Per-plugin state ────────────────────────────────────────────────────────

pub struct PluginState {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    engine: Arc<CoreEngine>,
    // ros interface — JSON-decoded subscriptions
    subscriptions: HashMap<u32, SubscriptionData>,
    next_sub_rep: u32,
    // session interface — named sessions declared in manifest
    sessions: HashMap<String, Arc<zenoh::Session>>,
    // session-handle resources: rep → session name
    session_handle_names: HashMap<u32, String>,
    // raw-transport resources
    raw_subs: HashMap<u32, RawSubData>,
    raw_pubs: HashMap<u32, RawPubData>,
    lv_tokens: HashMap<u32, LivelinessTokenData>,
    lv_subs: HashMap<u32, LivelinessSubData>,
    queryables: HashMap<u32, QueryableData>,
    queriers: HashMap<u32, QuerierData>,
    next_raw_rep: u32,
    // ros rate trackers: topic → tracker
    rate_trackers: HashMap<String, RateTrackerData>,
    // ros service clients
    service_clients: HashMap<u32, ServiceClientData>,
    // render interface
    pub output_lines: Arc<Mutex<Vec<String>>>,
    pub title: Arc<Mutex<String>>,
    // exit signal from render::exit()
    pub exit_code: Option<u32>,
    // permissions declared in manifest
    pub permissions: Vec<Permission>,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl PluginState {
    fn alloc_rep(&mut self) -> u32 {
        let r = self.next_raw_rep;
        self.next_raw_rep += 1;
        r
    }

    fn ensure_rate_tracker(&mut self, topic: &str) -> Result<(), String> {
        if self.rate_trackers.contains_key(topic) {
            return Ok(());
        }
        let domain_id = self.engine.domain_id;
        let topic_stripped = topic.trim_start_matches('/').to_string();
        let ke = format!("{domain_id}/{topic_stripped}/**");
        let session = self.engine.session.clone();
        let (tx, rx) = flume::bounded::<(Instant, usize)>(1024);
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("rate-tracker subscribe failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let size = sample.payload().to_bytes().len();
                let _ = tx.try_send((Instant::now(), size));
            }
        });
        self.rate_trackers.insert(
            topic.to_string(),
            RateTrackerData {
                rx,
                arrivals: VecDeque::new(),
                _abort: handle.abort_handle(),
            },
        );
        Ok(())
    }

    fn require_perm(&self, p: Permission) -> Result<(), String> {
        if self.permissions.contains(&p) {
            Ok(())
        } else {
            Err(format!(
                "permission denied: {:?} not declared in plugin manifest",
                p
            ))
        }
    }

    fn session_for_handle(
        &self,
        res: &Resource<hu::plugin::session::SessionHandle>,
    ) -> Result<Arc<zenoh::Session>, String> {
        let name = self
            .session_handle_names
            .get(&res.rep())
            .ok_or_else(|| "session handle not found".to_string())?;
        self.sessions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("session '{name}' not open"))
    }
}

// ─── types host impl ─────────────────────────────────────────────────────────

impl hu::plugin::types::Host for PluginState {}

// ─── graph host impl ─────────────────────────────────────────────────────────

impl hu::plugin::graph::Host for PluginState {
    fn list_topics(&mut self) -> Vec<hu::plugin::graph::TopicInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_topic_names_and_types()
            .into_iter()
            .map(|(name, type_name)| {
                let publishers = graph
                    .get_entities_by_topic(EndpointKind::Publisher, &name)
                    .len() as u32;
                let subscribers = graph
                    .get_entities_by_topic(EndpointKind::Subscription, &name)
                    .len() as u32;
                hu::plugin::graph::TopicInfo {
                    name,
                    type_name,
                    publishers,
                    subscribers,
                }
            })
            .collect()
    }

    fn list_nodes(&mut self) -> Vec<hu::plugin::graph::NodeInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_node_names()
            .into_iter()
            .map(|(name, namespace)| hu::plugin::graph::NodeInfo { namespace, name })
            .collect()
    }

    fn list_services(&mut self) -> Vec<hu::plugin::graph::ServiceInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_service_names_and_types()
            .into_iter()
            .map(|(name, type_name)| {
                let servers = graph.count_by_service(EndpointKind::Service, &name) as u32;
                hu::plugin::graph::ServiceInfo {
                    name,
                    type_name,
                    servers,
                }
            })
            .collect()
    }
}

// ─── ros host impl ───────────────────────────────────────────────────────────

impl hu::plugin::ros::Host for PluginState {
    fn subscribe(
        &mut self,
        topic: String,
    ) -> Result<Resource<hu::plugin::ros::Subscription>, String> {
        self.require_perm(Permission::SubscribeTopic)?;
        let rep = self.next_sub_rep;
        self.next_sub_rep += 1;

        let (tx, rx) = flume::bounded::<String>(256);
        let node = self.engine.node.clone();
        let topic_clone = topic.clone();

        let handle = tokio::spawn(async move {
            let sub = match node
                .create_dyn_sub_auto(&topic_clone, Duration::from_secs(5))
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "WASM plugin: schema discovery failed for {}: {e}",
                        topic_clone
                    );
                    return;
                }
            };
            loop {
                match sub.try_recv() {
                    Some(Ok(msg)) => {
                        let json = dynamic_message_to_json(&msg).to_string();
                        if tx.send_async(json).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => {}
                    None => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        });

        self.subscriptions.insert(
            rep,
            SubscriptionData {
                topic,
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn connect_service(
        &mut self,
        name: String,
        type_name: String,
    ) -> Result<Resource<hu::plugin::ros::ServiceClient>, String> {
        self.require_perm(Permission::CallService)?;
        let domain_id = self.engine.domain_id;
        let svc_stripped = name.trim_start_matches('/').to_string();

        // Look up type hash from the graph; fall back to wildcard.
        let ke = {
            let entities = self
                .engine
                .graph
                .lock()
                .get_entities_by_topic(EndpointKind::Service, &name);
            if let Some(ent) = entities.first() {
                if let Entity::Endpoint(ep) = ent.as_ref() {
                    if let Some(ti) = &ep.type_info {
                        let type_escaped = ti.name.replace('/', "%");
                        format!("{domain_id}/{svc_stripped}/{type_escaped}/{}", ti.hash)
                    } else {
                        let type_escaped = type_name.replace('/', "%");
                        format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
                    }
                } else {
                    let type_escaped = type_name.replace('/', "%");
                    format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
                }
            } else {
                let type_escaped = type_name.replace('/', "%");
                format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
            }
        };

        let session = self.engine.session.clone();
        let rep = self.alloc_rep();
        self.service_clients.insert(
            rep,
            ServiceClientData {
                session,
                ke,
                type_name,
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn measure_hz(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        self.require_perm(Permission::MeasureMetrics)?;
        self.ensure_rate_tracker(&topic)?;
        let tracker = self.rate_trackers.get_mut(&topic).unwrap();
        tracker.drain_and_trim(window_ms);
        let count = tracker.arrivals.len() as f64;
        let window_s = window_ms as f64 / 1000.0;
        Ok(count / window_s)
    }

    fn measure_bw(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        self.require_perm(Permission::MeasureMetrics)?;
        self.ensure_rate_tracker(&topic)?;
        let tracker = self.rate_trackers.get_mut(&topic).unwrap();
        tracker.drain_and_trim(window_ms);
        let total_bytes: usize = tracker.arrivals.iter().map(|(_, b)| b).sum();
        let window_s = window_ms as f64 / 1000.0;
        Ok(total_bytes as f64 / 1024.0 / window_s)
    }

    fn encode_yaml_to_cdr(&mut self, yaml: String, type_name: String) -> Result<Vec<u8>, String> {
        self.require_perm(Permission::PublishTopic)?;
        let schema = get_schema(&type_name)
            .ok_or_else(|| format!("schema for '{type_name}' not found in registry"))?;
        let value: serde_json::Value =
            serde_json::from_str(&yaml).map_err(|e| format!("failed to parse YAML/JSON: {e}"))?;
        let msg = json_to_dynamic_message(&value, &schema)?;
        serialize_cdr(&msg).map_err(|e| e.to_string())
    }
}

impl hu::plugin::ros::HostSubscription for PluginState {
    fn try_recv(&mut self, res: Resource<hu::plugin::ros::Subscription>) -> Option<String> {
        self.subscriptions
            .get(&res.rep())
            .and_then(|sub| sub.rx.try_recv().ok())
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::Subscription>) -> wasmtime::Result<()> {
        self.subscriptions.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::ros::HostServiceClient for PluginState {
    fn call(
        &mut self,
        res: Resource<hu::plugin::ros::ServiceClient>,
        request_json: String,
        timeout_ms: u32,
    ) -> Result<String, String> {
        let rep = res.rep();
        let Some(data) = self.service_clients.get(&rep) else {
            return Err("service client not found".to_string());
        };
        let session = data.session.clone();
        let ke = data.ke.clone();
        let req_type = data.type_name.clone();

        // Encode request JSON → CDR
        let req_schema =
            get_schema(&req_type).ok_or_else(|| format!("schema for '{req_type}' not found"))?;
        let req_value: serde_json::Value = serde_json::from_str(&request_json)
            .map_err(|e| format!("failed to parse request JSON: {e}"))?;
        let req_msg = json_to_dynamic_message(&req_value, &req_schema)?;
        let req_cdr = serialize_cdr(&req_msg).map_err(|e| e.to_string())?;

        // Call via Zenoh get
        let timeout = Duration::from_millis(timeout_ms as u64);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(req_cdr))
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;

        let reply = replies
            .recv()
            .map_err(|_| "no reply within timeout".to_string())?;
        let sample = reply.result().map_err(|e| e.to_string())?;
        let resp_cdr = sample.payload().to_bytes().into_owned();

        // Try to decode CDR response; use a response-type schema if available.
        // By convention the response type is req_type with "_Request" → "_Response".
        let resp_type = req_type.replace("_Request", "_Response");
        if let Some(resp_schema) = get_schema(&resp_type) {
            match deserialize_cdr(&resp_cdr, &resp_schema) {
                Ok(msg) => return Ok(dynamic_message_to_json(&msg).to_string()),
                Err(e) => tracing::warn!("failed to decode service response: {e}"),
            }
        }
        // Fall back to hex dump if schema not available
        Ok(format!(
            "{{\"raw\":\"{}\"}}",
            resp_cdr
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ))
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
        self.service_clients.remove(&res.rep());
        Ok(())
    }
}

// ─── raw-transport host impl ─────────────────────────────────────────────────

impl hu::plugin::raw_transport::Host for PluginState {}

impl hu::plugin::raw_transport::HostRawSubscription for PluginState {
    fn try_recv(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawSubscription>,
    ) -> Option<Vec<u8>> {
        self.raw_subs
            .get(&res.rep())
            .and_then(|s| s.rx.try_recv().ok())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawSubscription>,
    ) -> wasmtime::Result<()> {
        self.raw_subs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostRawPublisher for PluginState {
    fn publish(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawPublisher>,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let rep = res.rep();
        let Some(data) = self.raw_pubs.get(&rep) else {
            return Err("publisher not found".to_string());
        };
        let session = data.session.clone();
        let ke = data.ke.clone();
        session
            .put(&ke, zenoh::bytes::ZBytes::from(payload))
            .wait()
            .map_err(|e| e.to_string())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::RawPublisher>,
    ) -> wasmtime::Result<()> {
        self.raw_pubs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostLivelinessToken for PluginState {
    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessToken>,
    ) -> wasmtime::Result<()> {
        self.lv_tokens.remove(&res.rep()); // drops _abort → undeclares token
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostLivelinessSub for PluginState {
    fn try_recv(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessSub>,
    ) -> Option<(String, bool)> {
        self.lv_subs
            .get(&res.rep())
            .and_then(|s| s.rx.try_recv().ok())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::LivelinessSub>,
    ) -> wasmtime::Result<()> {
        self.lv_subs.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostQueryable for PluginState {
    fn try_recv_query(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
    ) -> Option<(u64, Vec<u8>)> {
        self.queryables
            .get(&res.rep())
            .and_then(|q| q.rx.try_recv().ok())
    }

    fn reply(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
        query_id: u64,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let rep = res.rep();
        let Some(qdata) = self.queryables.get(&rep) else {
            return Err("queryable not found".to_string());
        };
        let query = qdata
            .pending
            .lock()
            .remove(&query_id)
            .ok_or_else(|| format!("query {query_id} not found or already replied"))?;
        query
            .reply(
                query.key_expr().clone(),
                zenoh::bytes::ZBytes::from(payload),
            )
            .wait()
            .map_err(|e| e.to_string())
    }

    fn drop(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Queryable>,
    ) -> wasmtime::Result<()> {
        self.queryables.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::raw_transport::HostQuerier for PluginState {
    fn call(
        &mut self,
        res: Resource<hu::plugin::raw_transport::Querier>,
        payload: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, String> {
        let rep = res.rep();
        let Some(qdata) = self.queriers.get(&rep) else {
            return Err("querier not found".to_string());
        };
        let session = qdata.session.clone();
        let ke = qdata.ke.clone();
        let timeout = Duration::from_millis(timeout_ms as u64);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(payload))
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;
        match replies.recv() {
            Ok(reply) => match reply.result() {
                Ok(sample) => Ok(sample.payload().to_bytes().into_owned()),
                Err(e) => Err(e.to_string()),
            },
            Err(_) => Err("no reply received within timeout".to_string()),
        }
    }

    fn drop(&mut self, res: Resource<hu::plugin::raw_transport::Querier>) -> wasmtime::Result<()> {
        self.queriers.remove(&res.rep());
        Ok(())
    }
}

// ─── session host impl ───────────────────────────────────────────────────────

impl hu::plugin::session::Host for PluginState {
    fn get_session(
        &mut self,
        name: String,
    ) -> Result<Resource<hu::plugin::session::SessionHandle>, String> {
        self.require_perm(Permission::OpenSession)?;
        if !self.sessions.contains_key(&name) {
            return Err(format!(
                "session '{name}' not declared in manifest or failed to open"
            ));
        }
        let rep = self.alloc_rep();
        self.session_handle_names.insert(rep, name);
        Ok(Resource::new_own(rep))
    }
}

impl hu::plugin::session::HostSessionHandle for PluginState {
    fn raw_subscribe(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::RawSubscription>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<Vec<u8>>(256);
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("raw-subscribe failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let bytes = sample.payload().to_bytes().into_owned();
                if tx.send_async(bytes).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.raw_subs.insert(
            rep,
            RawSubData {
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn raw_publisher(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::RawPublisher>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let rep = self.alloc_rep();
        self.raw_pubs.insert(rep, RawPubData { session, ke });
        Ok(Resource::new_own(rep))
    }

    fn declare_liveliness(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::LivelinessToken>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let handle = tokio::spawn(async move {
            // Task holds the token. Aborting it undeclares it.
            let _token = match session.liveliness().declare_token(&ke).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("declare-liveliness failed on {ke}: {e}");
                    return;
                }
            };
            // Hold token until task is aborted.
            tokio::time::sleep(Duration::MAX).await;
        });
        let rep = self.alloc_rep();
        self.lv_tokens.insert(
            rep,
            LivelinessTokenData {
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn subscribe_liveliness(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::LivelinessSub>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<(String, bool)>(256);
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let sub = match session.liveliness().declare_subscriber(&ke_clone).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("liveliness-sub failed on {ke_clone}: {e}");
                    return;
                }
            };
            while let Ok(sample) = sub.recv_async().await {
                let key = sample.key_expr().to_string();
                let appeared = sample.kind() == zenoh::sample::SampleKind::Put;
                if tx.send_async((key, appeared)).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.lv_subs.insert(
            rep,
            LivelinessSubData {
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn declare_queryable(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::Queryable>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let (tx, rx) = flume::bounded::<(u64, Vec<u8>)>(64);
        let pending: Arc<Mutex<HashMap<u64, zenoh::query::Query>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_task = pending.clone();
        let ke_clone = ke.clone();
        let handle = tokio::spawn(async move {
            let queryable = match session.declare_queryable(&ke_clone).await {
                Ok(q) => q,
                Err(e) => {
                    tracing::warn!("declare-queryable failed on {ke_clone}: {e}");
                    return;
                }
            };
            let mut next_qid: u64 = 0;
            while let Ok(query) = queryable.recv_async().await {
                let id = next_qid;
                next_qid += 1;
                let payload = query
                    .payload()
                    .map(|p| p.to_bytes().into_owned())
                    .unwrap_or_default();
                pending_task.lock().insert(id, query);
                if tx.send_async((id, payload)).await.is_err() {
                    break;
                }
            }
        });
        let rep = self.alloc_rep();
        self.queryables.insert(
            rep,
            QueryableData {
                rx,
                pending,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn open_querier(
        &mut self,
        res: Resource<hu::plugin::session::SessionHandle>,
        ke: String,
    ) -> Result<Resource<hu::plugin::raw_transport::Querier>, String> {
        self.require_perm(Permission::AccessRawCdr)?;
        let session = self.session_for_handle(&res)?;
        let rep = self.alloc_rep();
        self.queriers.insert(rep, QuerierData { session, ke });
        Ok(Resource::new_own(rep))
    }

    fn drop(&mut self, res: Resource<hu::plugin::session::SessionHandle>) -> wasmtime::Result<()> {
        self.session_handle_names.remove(&res.rep());
        Ok(())
    }
}

// ─── render host impl ────────────────────────────────────────────────────────

impl hu::plugin::render::Host for PluginState {
    fn println(&mut self, text: String) {
        let mut lines = self.output_lines.lock();
        lines.push(text);
        if lines.len() > 1000 {
            lines.drain(0..500);
        }
    }

    fn set_title(&mut self, title: String) {
        *self.title.lock() = title;
    }

    fn emit_json(&mut self, key: String, value: String) {
        self.println(format!("{{\"{key}\":{value}}}"));
    }

    fn exit(&mut self, code: u32) {
        self.exit_code = Some(code);
    }
}

// ─── JSON→CDR helpers ────────────────────────────────────────────────────────

fn json_to_dynamic_message(
    value: &serde_json::Value,
    schema: &Arc<MessageSchema>,
) -> Result<DynamicMessage, String> {
    let obj = value.as_object().ok_or("expected a JSON object")?;
    let mut msg = DynamicMessage::new(schema);
    for field in &schema.fields {
        if let Some(v) = obj.get(&field.name) {
            let dval = json_to_dynamic_value(v, &field.field_type)?;
            msg.set_dynamic(&field.name, dval)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(msg)
}

fn json_to_dynamic_value(
    value: &serde_json::Value,
    ty: &FieldType,
) -> Result<DynamicValue, String> {
    match ty {
        FieldType::Bool => Ok(DynamicValue::Bool(value.as_bool().ok_or("expected bool")?)),
        FieldType::Int8 => Ok(DynamicValue::Int8(
            value.as_i64().ok_or("expected i8")? as i8
        )),
        FieldType::Int16 => Ok(DynamicValue::Int16(
            value.as_i64().ok_or("expected i16")? as i16
        )),
        FieldType::Int32 => Ok(DynamicValue::Int32(
            value.as_i64().ok_or("expected i32")? as i32
        )),
        FieldType::Int64 => Ok(DynamicValue::Int64(value.as_i64().ok_or("expected i64")?)),
        FieldType::Uint8 => Ok(DynamicValue::Uint8(
            value.as_u64().ok_or("expected u8")? as u8
        )),
        FieldType::Uint16 => Ok(DynamicValue::Uint16(
            value.as_u64().ok_or("expected u16")? as u16
        )),
        FieldType::Uint32 => Ok(DynamicValue::Uint32(
            value.as_u64().ok_or("expected u32")? as u32
        )),
        FieldType::Uint64 => Ok(DynamicValue::Uint64(value.as_u64().ok_or("expected u64")?)),
        FieldType::Float32 => Ok(DynamicValue::Float32(
            value.as_f64().ok_or("expected f32")? as f32
        )),
        FieldType::Float64 => Ok(DynamicValue::Float64(value.as_f64().ok_or("expected f64")?)),
        FieldType::String | FieldType::BoundedString(_) => Ok(DynamicValue::String(
            value.as_str().ok_or("expected string")?.to_string(),
        )),
        FieldType::Message(inner_schema) => Ok(DynamicValue::Message(Box::new(
            json_to_dynamic_message(value, inner_schema)?,
        ))),
        FieldType::Array(inner, _)
        | FieldType::Sequence(inner)
        | FieldType::BoundedSequence(inner, _) => {
            let arr = value.as_array().ok_or("expected array")?;
            let items = arr
                .iter()
                .map(|v| json_to_dynamic_value(v, inner))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DynamicValue::Array(items))
        }
    }
}

// ─── Loaded plugin handle ─────────────────────────────────────────────────────

pub struct WasmPlugin {
    pub manifest: hu::plugin::types::PluginManifest,
    pub output_lines: Arc<Mutex<Vec<String>>>,
    pub title: Arc<Mutex<String>>,
    store: Store<PluginState>,
    bindings: HuPlugin,
}

impl WasmPlugin {
    /// Dispatch an event to the plugin. Returns the exit code if the plugin
    /// called `render::exit()`, or `None` to continue.
    pub fn dispatch_event(&mut self, event: hu::plugin::types::PluginEvent) -> Option<u32> {
        // Filter events if the plugin declared a subscription list.
        if !self.manifest.subscribed_events.is_empty() {
            let kind = match &event {
                hu::plugin::types::PluginEvent::Startup(_) => EventKind::Startup,
                hu::plugin::types::PluginEvent::Tick => EventKind::Tick,
                hu::plugin::types::PluginEvent::KeyAction(_) => EventKind::KeyAction,
                hu::plugin::types::PluginEvent::TopicSelected(_) => EventKind::TopicSelected,
            };
            let subscribed = self.manifest.subscribed_events.iter().any(|k| {
                matches!(
                    (k, &kind),
                    (EventKind::Startup, EventKind::Startup)
                        | (EventKind::Tick, EventKind::Tick)
                        | (EventKind::KeyAction, EventKind::KeyAction)
                        | (EventKind::TopicSelected, EventKind::TopicSelected)
                )
            });
            if !subscribed {
                return self.store.data().exit_code;
            }
        }
        // Reset epoch deadline before each WASM call.
        self.store.set_epoch_deadline(30);
        if let Err(e) = self.bindings.call_on_event(&mut self.store, &event) {
            tracing::warn!("WASM plugin '{}' error on event: {e}", self.manifest.name);
        }
        self.store.data().exit_code
    }
}

// ─── Loader ──────────────────────────────────────────────────────────────────

/// Load all `.wasm` plugins found in `$HU_PLUGIN_PATH` and
/// `~/.local/share/hu/plugins/`.
///
/// Returns `(loaded, failed)` where `failed` is a list of `(path_display, error_message)`
/// for plugins that could not be loaded, so the TUI can show an indicator.
pub fn load_plugins(
    engine_ref: Arc<CoreEngine>,
) -> Result<(Vec<WasmPlugin>, Vec<(String, String)>)> {
    let search_dirs = plugin_search_dirs();
    let mut engine_config = wasmtime::Config::default();
    engine_config.epoch_interruption(true);
    let wasm_engine = Engine::new(&engine_config).context("creating WASM engine")?;

    // Spawn an epoch ticker so epoch_interruption can fire.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let ticker_engine = wasm_engine.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                ticker_engine.increment_epoch();
            }
        });
    }
    let mut plugins = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    for dir in &search_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }
            match load_one(&wasm_engine, &path, engine_ref.clone()) {
                Ok(plugin) => {
                    tracing::info!(
                        "Loaded WASM plugin '{}' from {}",
                        plugin.manifest.name,
                        path.display()
                    );
                    plugins.push(plugin);
                }
                Err(e) => {
                    let path_str = path.display().to_string();
                    tracing::warn!("Failed to load WASM plugin {path_str}: {e}");
                    failed.push((path_str, e.to_string()));
                }
            }
        }
    }

    Ok((plugins, failed))
}

/// Discover `.wasm` plugin files without loading them. Used by `hu plugin list`.
pub fn discover_wasm_plugins() -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    for dir in plugin_search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            // Strip the conventional "hu-" prefix so `hu-meter.wasm` registers
            // as "meter" and `hu meter <args>` dispatches correctly.
            let name = stem.strip_prefix("hu-").unwrap_or(stem).to_string();
            result.push((name, path));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn plugin_work_dir(plugin_stem: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hu")
        .join("plugin-work")
        .join(plugin_stem)
}

fn load_one(
    wasm_engine: &Engine,
    path: &PathBuf,
    engine_ref: Arc<CoreEngine>,
) -> Result<WasmPlugin> {
    // Extract plugin stem for sandbox dir (strip "hu-" prefix).
    let plugin_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let plugin_stem = plugin_stem.strip_prefix("hu-").unwrap_or(plugin_stem);
    let work_dir = plugin_work_dir(plugin_stem);
    std::fs::create_dir_all(&work_dir).ok();

    // wasmtime's built-in cache (configured via engine_config) handles caching.
    let component = Component::from_file(wasm_engine, path)
        .with_context(|| format!("compiling {}", path.display()))?;

    let mut linker: Linker<PluginState> = Linker::new(wasm_engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    HuPlugin::add_to_linker::<_, HasSelf<PluginState>>(&mut linker, |s| s)?;

    let output_lines = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new(String::new()));

    // Sandbox: pre-open only the plugin's own work directory as /work.
    // inherit_env exposes HU_ROUTER, HU_DOMAIN, etc. to the plugin guest.
    // Only load trusted plugins.
    let mut wasi_builder = WasiCtxBuilder::new();
    wasi_builder.inherit_env();
    if let Err(e) = wasi_builder.preopened_dir(
        &work_dir,
        "/work",
        wasmtime_wasi::DirPerms::all(),
        wasmtime_wasi::FilePerms::all(),
    ) {
        tracing::warn!(
            "failed to pre-open plugin work dir {}: {e}",
            work_dir.display()
        );
    }
    let wasi = wasi_builder.build();

    // Register the main hiroz session as "default" so plugins can call
    // session::get-session("default") without declaring it in their manifest.
    let default_session = engine_ref.session.clone();
    let mut initial_sessions: HashMap<String, Arc<zenoh::Session>> = HashMap::new();
    initial_sessions.insert("default".to_string(), default_session);

    let state = PluginState {
        wasi,
        table: wasmtime_wasi::ResourceTable::new(),
        engine: engine_ref,
        subscriptions: HashMap::new(),
        next_sub_rep: 0,
        sessions: initial_sessions,
        session_handle_names: HashMap::new(),
        raw_subs: HashMap::new(),
        raw_pubs: HashMap::new(),
        lv_tokens: HashMap::new(),
        lv_subs: HashMap::new(),
        queryables: HashMap::new(),
        queriers: HashMap::new(),
        next_raw_rep: 0,
        rate_trackers: HashMap::new(),
        service_clients: HashMap::new(),
        output_lines: output_lines.clone(),
        title: title.clone(),
        exit_code: None,
        permissions: vec![],
    };

    let mut store = Store::new(wasm_engine, state);
    // Allow up to 30 epochs (3 seconds) per WASM call before interrupting.
    store.set_epoch_deadline(30);
    let bindings = HuPlugin::instantiate(&mut store, &component, &linker)?;

    let manifest = bindings
        .call_manifest(&mut store)
        .context("calling manifest()")?;

    // Initialize permissions from manifest.
    store.data_mut().permissions = manifest.required_permissions.clone();

    open_declared_sessions(&mut store, &manifest)?;

    *title.lock() = manifest.name.clone();

    Ok(WasmPlugin {
        manifest,
        output_lines,
        title,
        store,
        bindings,
    })
}

/// Open all sessions declared in the manifest and store them in PluginState.
fn open_declared_sessions(
    store: &mut Store<PluginState>,
    manifest: &hu::plugin::types::PluginManifest,
) -> Result<()> {
    for req in &manifest.sessions {
        let name = req.name.clone();
        let endpoint = req.endpoint.clone();
        let mode_str = match req.mode {
            hu::plugin::types::SessionMode::Client => "\"client\"",
            hu::plugin::types::SessionMode::Peer => "\"peer\"",
        };

        let mut config = zenoh::Config::default();
        config
            .insert_json5("mode", mode_str)
            .map_err(|e| anyhow::anyhow!("session '{name}': set mode: {e}"))?;
        config
            .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
            .map_err(|e| anyhow::anyhow!("session '{name}': set endpoint: {e}"))?;
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .map_err(|e| anyhow::anyhow!("session '{name}': disable multicast: {e}"))?;

        let session = zenoh::open(config)
            .wait()
            .map_err(|e| anyhow::anyhow!("opening session '{name}' → {endpoint}: {e}"))?;

        store
            .data_mut()
            .sessions
            .insert(name.clone(), Arc::new(session));

        tracing::info!("WASM plugin session '{}' → {} opened", name, endpoint);
    }
    Ok(())
}

/// Statically validate a `.wasm` file by compiling it as a WASM component.
/// Does not require a live Zenoh session. Returns an OK message or an error.
pub fn validate_plugin_static(path: &std::path::Path) -> Result<String> {
    let engine_config = wasmtime::Config::default();
    let engine = Engine::new(&engine_config).context("creating validation engine")?;
    Component::from_file(&engine, path).with_context(|| format!("compiling {}", path.display()))?;
    Ok(format!("OK: {} is a valid WASM component", path.display()))
}

fn plugin_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(paths) = std::env::var("HU_PLUGIN_PATH") {
        for p in std::env::split_paths(&paths) {
            dirs.push(p);
        }
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/hu/plugins"));
    }
    dirs
}
