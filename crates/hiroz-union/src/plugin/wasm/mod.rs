//! WASM plugin loader: WasmPlugin, load_plugins, discover_wasm_plugins.
//!
//! Plugin world detection order at load time:
//!   1. hu-cli-plugin  → PluginBindings::Cli
//!   2. hu-tui-plugin  → PluginBindings::Tui
//!   3. hu-web-plugin  → PluginBindings::Web  (feature = "web-plugins")
//!   4. hu-plugin      → PluginBindings::Legacy  (v0.3 compat)

pub mod host;
pub mod state;

pub(crate) use host::hu;
pub use host::hu::plugin::types::{CliEvent, PluginManifest, TuiEvent};
#[cfg(feature = "web-plugins")]
pub use host::web_bindgen::hu::plugin::web_types::{HttpRequest, HttpResponse};

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

#[cfg(feature = "web-plugins")]
use self::host::web_bindgen;
use self::host::{HuPlugin, cli_bindgen, tui_bindgen};
use self::state::PluginState;

// ─── Loaded plugin handle ─────────────────────────────────────────────────────

/// Fields shared by every plugin world: identity, buffered output, and the
/// wasmtime store the guest instance runs in.
pub struct PluginCommon {
    manifest: PluginManifest,
    output_lines: Arc<Mutex<Vec<String>>>,
    title: Arc<Mutex<String>>,
    store: Store<PluginState>,
}

/// Per-world generated bindings.  Keeping only the bindings type per-variant
/// (instead of duplicating `manifest`/`output_lines`/`title`/`store` on each
/// arm) means the host dispatcher still cannot accidentally send a TUI event
/// to a CLI plugin, but the shared bookkeeping lives in one place.
pub enum PluginBindings {
    /// Plugin compiled against `hu-cli-plugin` world.
    Cli(cli_bindgen::HuCliPlugin),
    /// Plugin compiled against `hu-tui-plugin` world (v0.1).
    Tui(tui_bindgen::HuTuiPlugin),
    /// Plugin compiled against `hu-web-plugin` world.
    #[cfg(feature = "web-plugins")]
    Web(web_bindgen::HuWebPlugin),
    /// Plugin compiled against legacy `hu-plugin` world (v0.3 compat).
    Legacy(HuPlugin),
}

/// Typed plugin handle.
pub struct WasmPlugin {
    common: PluginCommon,
    bindings: PluginBindings,
}

/// Outcome of dispatching an event/request to a `WasmPlugin`.  Distinguishes
/// "dispatch ran on a plugin of the wrong kind" (a programmer error at the
/// call site) from "dispatch ran fine, no exit code yet" (normal steady
/// state) — collapsing both to `None` made it impossible to tell them apart.
pub enum DispatchOutcome {
    /// The plugin handle was not of the kind this dispatch method targets.
    WrongKind,
    /// The dispatch call was made; the plugin may or may not have exited.
    Ran { exit_code: Option<u32> },
}

impl DispatchOutcome {
    /// Convenience accessor for call sites that don't care about the
    /// wrong-kind/ran distinction and just want the exit code, if any.
    pub fn exit_code(&self) -> Option<u32> {
        match self {
            DispatchOutcome::WrongKind => None,
            DispatchOutcome::Ran { exit_code } => *exit_code,
        }
    }
}

impl WasmPlugin {
    pub fn manifest(&self) -> &PluginManifest {
        &self.common.manifest
    }

    pub fn output_lines(&self) -> &Arc<Mutex<Vec<String>>> {
        &self.common.output_lines
    }

    pub fn title(&self) -> &Arc<Mutex<String>> {
        &self.common.title
    }

    pub fn is_cli(&self) -> bool {
        matches!(self.bindings, PluginBindings::Cli(_))
    }

    #[cfg(feature = "web-plugins")]
    pub fn is_web(&self) -> bool {
        matches!(self.bindings, PluginBindings::Web(_))
    }

    /// Dispatch a CLI event to a `Cli` plugin.  Type-safe: `CliEvent` has no
    /// `key-action` or `topic-selected` variants so the compiler prevents TUI
    /// events from being sent down the CLI path.
    pub fn dispatch_cli_event(&mut self, event: CliEvent) -> DispatchOutcome {
        let PluginBindings::Cli(bindings) = &mut self.bindings else {
            return DispatchOutcome::WrongKind;
        };
        let store = &mut self.common.store;
        let manifest = &self.common.manifest;
        // Interrupt bypasses subscription filtering.
        if matches!(event, CliEvent::Interrupt) {
            store.set_epoch_deadline(30);
            if let Err(e) = bindings.call_on_event(&mut *store, &event) {
                tracing::warn!("CLI plugin '{}' interrupt error: {e}", manifest.name);
            }
            return DispatchOutcome::Ran {
                exit_code: store.data().exit_code,
            };
        }
        // Event subscription filtering.
        if !manifest.subscribed_events.is_empty() {
            use hu::plugin::types::EventKind;
            let kind = match &event {
                CliEvent::Startup(_) => EventKind::Startup,
                CliEvent::Tick => EventKind::Tick,
                CliEvent::Interrupt => unreachable!(),
            };
            let subscribed = manifest.subscribed_events.iter().any(|k| {
                matches!(
                    (k, &kind),
                    (EventKind::Startup, EventKind::Startup) | (EventKind::Tick, EventKind::Tick)
                )
            });
            if !subscribed {
                return DispatchOutcome::Ran {
                    exit_code: store.data().exit_code,
                };
            }
        }
        store.set_epoch_deadline(30);
        if let Err(e) = bindings.call_on_event(&mut *store, &event) {
            tracing::warn!("CLI plugin '{}' error: {e}", manifest.name);
        }
        DispatchOutcome::Ran {
            exit_code: store.data().exit_code,
        }
    }

    /// Dispatch a TUI event to a `Tui` or `Legacy` plugin.
    pub fn dispatch_tui_event(&mut self, event: TuiEvent) -> DispatchOutcome {
        let Self { common, bindings } = self;
        match bindings {
            PluginBindings::Tui(bindings) => DispatchOutcome::Ran {
                exit_code: dispatch_inner("TUI", &mut common.store, &common.manifest, |s| {
                    bindings.call_on_event(s, &event)
                }),
            },
            PluginBindings::Legacy(bindings) => {
                // Convert TuiEvent → PluginEvent for the legacy world.
                let pe = tui_to_plugin_event(event);
                DispatchOutcome::Ran {
                    exit_code: dispatch_inner("Legacy", &mut common.store, &common.manifest, |s| {
                        bindings.call_on_event(s, &pe)
                    }),
                }
            }
            _ => DispatchOutcome::WrongKind,
        }
    }

    /// Dispatch a web request to a `Web` plugin and return the HTTP response.
    #[cfg(feature = "web-plugins")]
    pub fn dispatch_web_request(&mut self, req: HttpRequest) -> Option<HttpResponse> {
        let PluginBindings::Web(bindings) = &mut self.bindings else {
            return None;
        };
        let store = &mut self.common.store;
        let manifest = &self.common.manifest;
        store.set_epoch_deadline(30);
        match bindings.call_handle(&mut *store, &req) {
            Ok(resp) => Some(resp),
            Err(e) => {
                tracing::warn!("Web plugin '{}' handle error: {e}", manifest.name);
                None
            }
        }
    }
}

fn dispatch_inner(
    label: &str,
    store: &mut Store<PluginState>,
    manifest: &hu::plugin::types::PluginManifest,
    call: impl FnOnce(&mut Store<PluginState>) -> anyhow::Result<()>,
) -> Option<u32> {
    store.set_epoch_deadline(30);
    if let Err(e) = call(store) {
        tracing::warn!("{label} plugin '{}' error: {e}", manifest.name);
    }
    store.data().exit_code
}

fn tui_to_plugin_event(e: TuiEvent) -> hu::plugin::types::PluginEvent {
    match e {
        TuiEvent::Startup(args) => hu::plugin::types::PluginEvent::Startup(args),
        TuiEvent::KeyAction(s) => hu::plugin::types::PluginEvent::KeyAction(s),
        TuiEvent::TopicSelected(s) => hu::plugin::types::PluginEvent::TopicSelected(s),
        TuiEvent::Tick => hu::plugin::types::PluginEvent::Tick,
        TuiEvent::Interrupt => hu::plugin::types::PluginEvent::Interrupt,
    }
}

// ─── Loader ──────────────────────────────────────────────────────────────────

type LoadResult = (Vec<WasmPlugin>, Vec<(String, String)>);

pub fn load_plugins(engine_ref: Arc<CoreEngine>) -> Result<LoadResult> {
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

    for path in iter_wasm_files() {
        match load_one(&wasm_engine, &path, engine_ref.clone()) {
            Ok(plugin) => {
                tracing::info!(
                    "Loaded WASM plugin '{}' ({}) from {}",
                    plugin.manifest().name,
                    plugin_kind_label(&plugin),
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

    Ok((plugins, failed))
}

fn plugin_kind_label(p: &WasmPlugin) -> &'static str {
    match p.bindings {
        PluginBindings::Cli(_) => "cli",
        PluginBindings::Tui(_) => "tui",
        #[cfg(feature = "web-plugins")]
        PluginBindings::Web(_) => "web",
        PluginBindings::Legacy(_) => "legacy",
    }
}

pub fn discover_wasm_plugins() -> Vec<(String, PathBuf)> {
    let mut result: Vec<(String, PathBuf)> = iter_wasm_files()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            // Compiled WASM artifact filenames always normalize hyphens to
            // underscores (crate name "hu-meter" -> "hu_meter.wasm"), so strip
            // the underscored prefix here, not the crate name's hyphenated form.
            let name = stem.strip_prefix("hu_").unwrap_or(stem).to_string();
            (name, path)
        })
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn iter_wasm_files() -> impl Iterator<Item = PathBuf> {
    plugin_search_dirs()
        .into_iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
}

fn plugin_work_dir(plugin_stem: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hu")
        .join("plugin-work")
        .join(plugin_stem)
}

type StateAndStore = (
    Arc<Mutex<Vec<String>>>,
    Arc<Mutex<String>>,
    Store<PluginState>,
);

/// Build a fresh `PluginState` + `Store` for a component at `path`.
fn make_state_and_store(
    wasm_engine: &Engine,
    work_dir: &PathBuf,
    engine_ref: Arc<CoreEngine>,
) -> Result<StateAndStore> {
    let output_lines = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new(String::new()));

    let mut wasi_builder = WasiCtxBuilder::new();
    for var in &["HU_ROUTER", "HU_DOMAIN", "HOME", "PATH", "RUST_LOG"] {
        if let Ok(val) = std::env::var(var) {
            wasi_builder.env(var, &val);
        }
    }
    if let Err(e) = wasi_builder.preopened_dir(
        work_dir,
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
    Ok((output_lines, title, store))
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

    // Probe CLI world first (most restrictive — fewest dead arms in plugins).
    if let Ok(plugin) = try_load_cli(wasm_engine, &component, &work_dir, engine_ref.clone()) {
        return Ok(plugin);
    }

    // Probe TUI world (v0.1 hu-tui-plugin).
    if let Ok(plugin) = try_load_tui(wasm_engine, &component, &work_dir, engine_ref.clone()) {
        return Ok(plugin);
    }

    // Probe Web world.
    #[cfg(feature = "web-plugins")]
    if let Ok(plugin) = try_load_web(wasm_engine, &component, &work_dir, engine_ref.clone()) {
        return Ok(plugin);
    }

    // Fall back to legacy hu-plugin world (v0.3 compat).
    try_load_legacy(wasm_engine, &component, &work_dir, engine_ref)
        .with_context(|| format!("loading {}", path.display()))
}

macro_rules! try_load {
    ($fn_name:ident, $bindings_ty:ty, $variant:ident) => {
        fn $fn_name(
            wasm_engine: &Engine,
            component: &Component,
            work_dir: &PathBuf,
            engine_ref: Arc<CoreEngine>,
        ) -> Result<WasmPlugin> {
            let mut linker: Linker<PluginState> = Linker::new(wasm_engine);
            wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
            <$bindings_ty>::add_to_linker::<_, HasSelf<PluginState>>(&mut linker, |s| s)?;
            let (output_lines, title, mut store) =
                make_state_and_store(wasm_engine, work_dir, engine_ref)?;
            let bindings = <$bindings_ty>::instantiate(&mut store, component, &linker)?;
            let manifest = bindings.call_manifest(&mut store).context("manifest()")?;
            store
                .data_mut()
                .set_permissions(manifest.required_permissions.clone());
            open_declared_sessions(&mut store, &manifest)?;
            *title.lock() = manifest.name.clone();
            Ok(WasmPlugin {
                common: PluginCommon {
                    manifest,
                    output_lines,
                    title,
                    store,
                },
                bindings: PluginBindings::$variant(bindings),
            })
        }
    };
}

try_load!(try_load_cli, cli_bindgen::HuCliPlugin, Cli);
try_load!(try_load_tui, tui_bindgen::HuTuiPlugin, Tui);
#[cfg(feature = "web-plugins")]
try_load!(try_load_web, web_bindgen::HuWebPlugin, Web);
try_load!(try_load_legacy, HuPlugin, Legacy);

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
    let engine = Engine::new(&wasmtime::Config::default()).context("creating validation engine")?;
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
