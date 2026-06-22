use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use hiroz_protocol::EndpointKind;
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker, Resource, bindgen},
};
use wasmtime_wasi::{WasiCtxBuilder, WasiCtxView, WasiView};

use crate::core::engine::CoreEngine;

// Generate host/plugin bindings from the WIT schema.
bindgen!({
    world: "hu-plugin",
    path: "wit/hu-plugin.wit",
});

// ─── Subscription tracking ────────────────────────────────────────────────────

struct SubscriptionData {
    topic: String,
    /// Messages decoded to JSON.  Populated by a background tokio task.
    rx: flume::Receiver<String>,
    /// Keep the tokio task alive as long as the subscription exists.
    _abort: tokio::task::AbortHandle,
}

// ─── Per-plugin state stored in the wasmtime Store ───────────────────────────

pub struct PluginState {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    engine: Arc<CoreEngine>,
    /// Active subscriptions keyed by a per-plugin u32 rep.
    subscriptions: HashMap<u32, SubscriptionData>,
    next_sub_rep: u32,
    /// Output lines emitted via render::println.
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

// ─── Host implementation of the WIT interfaces ───────────────────────────────

impl hu::plugin::types::Host for PluginState {}

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
        // get_node_names() returns (name, namespace) — swap for NodeInfo field order.
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

impl hu::plugin::ros::Host for PluginState {
    fn subscribe(
        &mut self,
        topic: String,
    ) -> Result<Resource<hu::plugin::ros::Subscription>, String> {
        let rep = self.next_sub_rep;
        self.next_sub_rep += 1;

        // Spawn a background task that polls the Zenoh session for messages on
        // this topic and forwards JSON-decoded payloads via a bounded channel.
        // v0.1: returns an empty channel; real CDR→JSON decoding comes later.
        let (tx, rx) = flume::bounded::<String>(256);
        let session = self.engine.session.clone();
        let topic_clone = topic.clone();
        let handle = tokio::spawn(async move {
            // We need the full key expression including type hash to subscribe.
            // For v0.1 emit a synthetic message so the plugin at least loads.
            let _ = tx
                .send_async(format!("{{\"__topic\":\"{topic_clone}\"}}"))
                .await;
            // Real implementation: use node.create_dyn_sub_auto() and decode CDR to JSON.
            let _session = session; // keep alive
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
        Err("service calls not yet implemented in v0.1".to_string())
    }

    fn measure_hz(&mut self, _topic: String, _window_ms: u32) -> Result<f64, String> {
        // v0.1 stub: real hz measurement via MetricsCollector comes in a later task.
        Ok(0.0)
    }

    fn measure_bw(&mut self, _topic: String, _window_ms: u32) -> Result<f64, String> {
        // v0.1 stub: real bw measurement via MetricsCollector comes in a later task.
        Ok(0.0)
    }
}

impl hu::plugin::ros::HostSubscription for PluginState {
    fn try_recv(&mut self, res: Resource<hu::plugin::ros::Subscription>) -> Option<String> {
        let rep = res.rep();
        self.subscriptions
            .get(&rep)
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
        Err("service calls not yet implemented in v0.1".to_string())
    }

    fn drop(&mut self, _res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
        Ok(())
    }
}

impl hu::plugin::render::Host for PluginState {
    fn println(&mut self, text: String) {
        let mut lines = self.output_lines.lock().unwrap();
        lines.push(text);
        // Keep ring buffer bounded.
        if lines.len() > 1000 {
            lines.drain(0..500);
        }
    }

    fn set_title(&mut self, title: String) {
        *self.title.lock().unwrap() = title;
    }

    fn emit_json(&mut self, key: String, value: String) {
        let line = format!("{{\"{}\":{}}}", key, value);
        self.println(line);
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

fn load_one(
    wasm_engine: &Engine,
    path: &PathBuf,
    engine_ref: Arc<CoreEngine>,
) -> Result<WasmPlugin> {
    let component = Component::from_file(wasm_engine, path)
        .with_context(|| format!("compiling {}", path.display()))?;

    let mut linker: Linker<PluginState> = Linker::new(wasm_engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    HuPlugin::add_to_linker(&mut linker, |s: &mut PluginState| HasSelf(s))?;

    let output_lines = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new(String::new()));

    let wasi = WasiCtxBuilder::new().inherit_env().build();

    let state = PluginState {
        wasi,
        table: wasmtime_wasi::ResourceTable::new(),
        engine: engine_ref,
        subscriptions: HashMap::new(),
        next_sub_rep: 0,
        output_lines: output_lines.clone(),
        title: title.clone(),
    };

    let mut store = Store::new(wasm_engine, state);
    let bindings = HuPlugin::instantiate(&mut store, &component, &linker)?;

    let manifest = bindings
        .call_manifest(&mut store)
        .context("calling manifest()")?;

    *title.lock().unwrap() = manifest.name.clone();

    Ok(WasmPlugin {
        manifest,
        output_lines,
        title,
        store,
        bindings,
    })
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
