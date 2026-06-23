use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use hiroz::dynamic::{DynamicMessage, DynamicValue};
use hiroz_protocol::EndpointKind;
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
    // render interface
    pub output_lines: Arc<Mutex<Vec<String>>>,
    pub title: Arc<Mutex<String>>,
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
                        let json = dyn_msg_to_json(&msg).to_string();
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
        _name: String,
        _type_name: String,
    ) -> Result<Resource<hu::plugin::ros::ServiceClient>, String> {
        Err("service calls not yet implemented".to_string())
    }

    fn measure_hz(&mut self, _topic: String, _window_ms: u32) -> Result<f64, String> {
        Ok(0.0)
    }

    fn measure_bw(&mut self, _topic: String, _window_ms: u32) -> Result<f64, String> {
        Ok(0.0)
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
        _res: Resource<hu::plugin::ros::ServiceClient>,
        _request_json: String,
        _timeout_ms: u32,
    ) -> Result<String, String> {
        Err("service calls not yet implemented".to_string())
    }

    fn drop(&mut self, _res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
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
            .unwrap()
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
            loop {
                match sub.recv_async().await {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes().into_owned();
                        if tx.send_async(bytes).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
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
            loop {
                match sub.recv_async().await {
                    Ok(sample) => {
                        let key = sample.key_expr().to_string();
                        let appeared = sample.kind() == zenoh::sample::SampleKind::Put;
                        if tx.send_async((key, appeared)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
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
            loop {
                match queryable.recv_async().await {
                    Ok(query) => {
                        let id = next_qid;
                        next_qid += 1;
                        let payload = query
                            .payload()
                            .map(|p| p.to_bytes().into_owned())
                            .unwrap_or_default();
                        pending_task.lock().unwrap().insert(id, query);
                        if tx.send_async((id, payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
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
        let mut lines = self.output_lines.lock().unwrap();
        lines.push(text);
        if lines.len() > 1000 {
            lines.drain(0..500);
        }
    }

    fn set_title(&mut self, title: String) {
        *self.title.lock().unwrap() = title;
    }

    fn emit_json(&mut self, key: String, value: String) {
        self.println(format!("{{\"{key}\":{value}}}"));
    }
}

// ─── CDR→JSON helpers ────────────────────────────────────────────────────────

fn dyn_msg_to_json(msg: &DynamicMessage) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in msg.iter() {
        map.insert(name.to_string(), dyn_value_to_json(value));
    }
    serde_json::Value::Object(map)
}

fn dyn_value_to_json(value: &DynamicValue) -> serde_json::Value {
    match value {
        DynamicValue::Bool(b) => serde_json::Value::Bool(*b),
        DynamicValue::Int8(i) => (*i as i64).into(),
        DynamicValue::Int16(i) => (*i as i64).into(),
        DynamicValue::Int32(i) => (*i as i64).into(),
        DynamicValue::Int64(i) => (*i).into(),
        DynamicValue::Uint8(u) => (*u as u64).into(),
        DynamicValue::Uint16(u) => (*u as u64).into(),
        DynamicValue::Uint32(u) => (*u as u64).into(),
        DynamicValue::Uint64(u) => (*u).into(),
        DynamicValue::Float32(f) => serde_json::Number::from_f64(*f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::String(s) => serde_json::Value::String(s.clone()),
        DynamicValue::Bytes(b) => serde_json::Value::String(
            b.iter()
                .map(|x| format!("{:02x}", x))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        DynamicValue::Message(inner) => dyn_msg_to_json(inner),
        DynamicValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(dyn_value_to_json).collect())
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
    pub fn dispatch_event(&mut self, event: hu::plugin::types::PluginEvent) {
        if let Err(e) = self.bindings.call_on_event(&mut self.store, &event) {
            tracing::warn!("WASM plugin '{}' error on event: {e}", self.manifest.name);
        }
    }
}

// ─── Loader ──────────────────────────────────────────────────────────────────

/// Load all `.wasm` plugins found in `$HU_PLUGIN_PATH` and
/// `~/.local/share/hu/plugins/`.
pub fn load_plugins(engine_ref: Arc<CoreEngine>) -> Vec<WasmPlugin> {
    let search_dirs = plugin_search_dirs();
    let wasm_engine = Engine::default();
    let mut plugins = Vec::new();

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
                    tracing::warn!("Failed to load WASM plugin {}: {e}", path.display());
                }
            }
        }
    }

    plugins
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
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            result.push((name, path));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn load_one(
    wasm_engine: &Engine,
    path: &PathBuf,
    engine_ref: Arc<CoreEngine>,
) -> Result<WasmPlugin> {
    let component = Component::from_file(wasm_engine, path)
        .with_context(|| format!("compiling {}", path.display()))?;

    let mut linker: Linker<PluginState> = Linker::new(wasm_engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    HuPlugin::add_to_linker::<_, HasSelf<PluginState>>(&mut linker, |s| s)?;

    let output_lines = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new(String::new()));

    let wasi = WasiCtxBuilder::new().inherit_env().build();
    let state = PluginState {
        wasi,
        table: wasmtime_wasi::ResourceTable::new(),
        engine: engine_ref,
        subscriptions: HashMap::new(),
        next_sub_rep: 0,
        sessions: HashMap::new(),
        session_handle_names: HashMap::new(),
        raw_subs: HashMap::new(),
        raw_pubs: HashMap::new(),
        lv_tokens: HashMap::new(),
        lv_subs: HashMap::new(),
        queryables: HashMap::new(),
        queriers: HashMap::new(),
        next_raw_rep: 0,
        output_lines: output_lines.clone(),
        title: title.clone(),
    };

    let mut store = Store::new(wasm_engine, state);
    let bindings = HuPlugin::instantiate(&mut store, &component, &linker)?;

    let manifest = bindings
        .call_manifest(&mut store)
        .context("calling manifest()")?;

    open_declared_sessions(&mut store, &manifest)?;

    *title.lock().unwrap() = manifest.name.clone();

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
