//! WASM plugin loader: WasmPlugin handle, load_plugins, discover_wasm_plugins.

pub mod host;
pub mod state;

pub use host::hu;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker},
};
use wasmtime_wasi::WasiCtxBuilder;
use zenoh::Wait;

use crate::core::engine::CoreEngine;

use self::host::HuPlugin;
use self::state::PluginState;

// ─── Loaded plugin handle ─────────────────────────────────────────────────────

pub struct WasmPlugin {
    pub manifest: hu::plugin::types::PluginManifest,
    pub output_lines: Arc<Mutex<Vec<String>>>,
    pub title: Arc<Mutex<String>>,
    store: Store<PluginState>,
    bindings: HuPlugin,
}

impl WasmPlugin {
    pub fn dispatch_event(&mut self, event: hu::plugin::types::PluginEvent) -> Option<u32> {
        use hu::plugin::types::EventKind;
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
        self.store.set_epoch_deadline(30);
        if let Err(e) = self.bindings.call_on_event(&mut self.store, &event) {
            tracing::warn!("WASM plugin '{}' error on event: {e}", self.manifest.name);
        }
        self.store.data().exit_code
    }
}

// ─── Loader ──────────────────────────────────────────────────────────────────

type LoadResult = (Vec<WasmPlugin>, Vec<(String, String)>);

pub fn load_plugins(engine_ref: Arc<CoreEngine>) -> Result<LoadResult> {
    let search_dirs = plugin_search_dirs();
    let mut engine_config = wasmtime::Config::default();
    engine_config.epoch_interruption(true);
    let wasm_engine = Engine::new(&engine_config).context("creating WASM engine")?;

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let ticker_engine = wasm_engine.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
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
    let plugin_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let plugin_stem = plugin_stem.strip_prefix("hu-").unwrap_or(plugin_stem);
    let work_dir = plugin_work_dir(plugin_stem);
    std::fs::create_dir_all(&work_dir).ok();

    let component = Component::from_file(wasm_engine, path)
        .with_context(|| format!("compiling {}", path.display()))?;

    let mut linker: Linker<PluginState> = Linker::new(wasm_engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    HuPlugin::add_to_linker::<_, HasSelf<PluginState>>(&mut linker, |s| s)?;

    let output_lines = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new(String::new()));

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
    store.set_epoch_deadline(30);
    let bindings = HuPlugin::instantiate(&mut store, &component, &linker)?;

    let manifest = bindings
        .call_manifest(&mut store)
        .context("calling manifest()")?;

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
