//! WASM plugin loader: WasmPlugin, load_plugins, discover_wasm_plugins.
//!
//! Plugin world detection order at load time:
//!   1. hu-cli-plugin  → PluginBindings::Cli
//!   2. hu-web-plugin  → PluginBindings::Web  (feature = "web-plugins")
//!   3. hu-tui-plugin  → PluginBindings::Tui  (fallback)

pub mod host;
pub mod state;

pub(crate) use host::hu;
pub use host::hu::plugin::types::{CliEvent, PluginManifest, TuiEvent};
#[cfg(feature = "web-plugins")]
pub use host::web_bindgen::hu::plugin::web_types::{HttpRequest, HttpResponse};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
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
use self::host::{HuTuiPlugin, cli_bindgen};
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

/// Per-world generated bindings. Only the bindings type varies per variant;
/// shared bookkeeping lives in `PluginCommon`. Preserves the type safety that a
/// TUI event cannot be dispatched to a CLI plugin.
pub enum PluginBindings {
    /// Plugin compiled against `hu-cli-plugin` world.
    Cli(cli_bindgen::HuCliPlugin),
    /// Plugin compiled against `hu-tui-plugin` world (v0.1).
    Tui(HuTuiPlugin),
    /// Plugin compiled against `hu-web-plugin` world.
    #[cfg(feature = "web-plugins")]
    Web(web_bindgen::HuWebPlugin),
}

/// Typed plugin handle.
pub struct WasmPlugin {
    common: PluginCommon,
    bindings: PluginBindings,
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

    /// A plugin that accepts `tui-event`s (the `hu-tui-plugin` world). Only
    /// these receive forwarded key-action / topic-selected events.
    pub fn is_tui(&self) -> bool {
        matches!(self.bindings, PluginBindings::Tui(_))
    }

    #[cfg(feature = "web-plugins")]
    pub fn is_web(&self) -> bool {
        matches!(self.bindings, PluginBindings::Web(_))
    }

    /// Dispatch a CLI event to a `Cli` plugin. `CliEvent` lacks TUI-only
    /// variants, so the compiler bars TUI events on this path. Returns the exit
    /// code if set (`None` also for a dispatch against a non-CLI handle).
    pub fn dispatch_cli_event(&mut self, event: CliEvent) -> Option<u32> {
        let PluginBindings::Cli(bindings) = &mut self.bindings else {
            return None;
        };
        let store = &mut self.common.store;
        let manifest = &self.common.manifest;
        // Interrupt bypasses subscription filtering.
        if matches!(event, CliEvent::Interrupt) {
            store.set_epoch_deadline(30);
            if let Err(e) = bindings.call_on_event(&mut *store, &event) {
                tracing::warn!("CLI plugin '{}' interrupt error: {e}", manifest.name);
            }
            return store.data().exit_code;
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
                return store.data().exit_code;
            }
        }
        store.set_epoch_deadline(30);
        if let Err(e) = bindings.call_on_event(&mut *store, &event) {
            tracing::warn!("CLI plugin '{}' error: {e}", manifest.name);
        }
        store.data().exit_code
    }

    /// Dispatch a TUI event to a `Tui` plugin. Returns the plugin's exit code if
    /// set (`None` also covers a dispatch made against a non-TUI handle).
    pub fn dispatch_tui_event(&mut self, event: TuiEvent) -> Option<u32> {
        let Self { common, bindings } = self;
        match bindings {
            PluginBindings::Tui(bindings) => {
                dispatch_inner("TUI", &mut common.store, &common.manifest, |s| {
                    bindings.call_on_event(s, &event)
                })
            }
            _ => None,
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

// ─── Loader ──────────────────────────────────────────────────────────────────

type LoadResult = (Vec<WasmPlugin>, Vec<(String, String)>);

/// Build the wasmtime engine used to load plugins. Enables epoch interruption
/// (to preempt a runaway guest) and the on-disk compile cache — hu is a
/// short-lived CLI, so caching lets repeat invocations deserialize the artifact
/// instead of re-running cranelift (~2.5s per hu-meter compile; the WASM test
/// suite spawns ~40 such subprocesses).
fn configured_wasm_engine() -> Result<Engine> {
    let mut engine_config = wasmtime::Config::default();
    engine_config.epoch_interruption(true);
    // A missing/unwritable cache dir must never be fatal — fall back to
    // uncached compilation.
    if let Ok(cache) = wasmtime::Cache::from_file(None) {
        engine_config.cache(Some(cache));
    }
    Engine::new(&engine_config).context("creating WASM engine")
}

// ─── Epoch budget vs. blocking host calls ────────────────────────────────────

/// Number of host calls currently blocked on I/O on behalf of a guest.
///
/// Every guest dispatch runs under `set_epoch_deadline(30)` and the ticker below
/// increments the epoch every 100 ms, so a guest gets ~3 s of *wall clock* — and
/// wall clock is what the ticker measures, so time a host call spends waiting on
/// the network counts against the guest's budget even though the guest is not
/// running. A host call that blocks longer than the remaining budget therefore
/// traps the guest the moment it returns, and the guest never runs the error
/// branch it was just handed. That is not a theoretical bound: schema discovery
/// waits for a live publisher and then queries it, which legitimately takes
/// seconds on a cold graph.
static HOST_BLOCKING_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Suspends the epoch ticker for as long as it is alive. Hold one around any
/// host call that blocks on I/O, so the wait is not charged to the guest's
/// compute budget.
///
/// The suspension is process-wide (there is one engine and one ticker), so a
/// *different* runaway guest is not preempted while a blocking call is in
/// flight. That window is bounded by the host call's own timeout, and the
/// alternative — trapping a well-behaved guest for waiting on the network — is
/// strictly worse.
pub(crate) struct HostBlockGuard(());

impl HostBlockGuard {
    pub(crate) fn enter() -> Self {
        HOST_BLOCKING_CALLS.fetch_add(1, Ordering::SeqCst);
        Self(())
    }
}

impl Drop for HostBlockGuard {
    fn drop(&mut self) {
        HOST_BLOCKING_CALLS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether the epoch ticker should advance right now. Split out so the rule is
/// testable without an engine.
fn epoch_should_tick() -> bool {
    HOST_BLOCKING_CALLS.load(Ordering::SeqCst) == 0
}

/// Process-wide shared WASM engine (and its single epoch-ticker task). Building
/// a fresh engine per `load_plugins` call would spawn a new ticker each time —
/// the TUI's `reload_plugins` loops, so tickers (each holding an engine clone)
/// would leak unboundedly. Sharing one engine keeps exactly one ticker per
/// process.
static SHARED_WASM_ENGINE: OnceLock<Engine> = OnceLock::new();

fn shared_wasm_engine() -> Result<Engine> {
    if let Some(engine) = SHARED_WASM_ENGINE.get() {
        return Ok(engine.clone());
    }
    // Build a candidate up front so a construction error can propagate; only the
    // engine that wins the `get_or_init` race gets its ticker spawned.
    let candidate = configured_wasm_engine()?;
    let engine = SHARED_WASM_ENGINE.get_or_init(|| {
        let ticker_engine = candidate.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if epoch_should_tick() {
                        ticker_engine.increment_epoch();
                    }
                }
            });
        } else {
            // No Tokio runtime here: without a ticker the epoch never advances
            // and runaway plugins can't be preempted. Fall back to a dedicated
            // OS thread incrementing the epoch on the same cadence.
            if let Err(e) = std::thread::Builder::new()
                .name("hu-wasm-epoch".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_millis(100));
                        if epoch_should_tick() {
                            ticker_engine.increment_epoch();
                        }
                    }
                })
            {
                // No ticker means no preemption; surface the loss rather than
                // silently disabling it.
                tracing::warn!(
                    error = %e,
                    "failed to spawn hu-wasm-epoch ticker thread; WASM plugin preemption is disabled"
                );
            }
        }
        candidate
    });
    Ok(engine.clone())
}

/// Compile and instantiate the plugins found at `paths`, collecting per-path
/// failures rather than aborting the whole load.
fn load_from(
    engine_ref: Arc<CoreEngine>,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Result<LoadResult> {
    let wasm_engine = shared_wasm_engine()?;
    let mut plugins = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    for path in paths {
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

pub fn load_plugins(engine_ref: Arc<CoreEngine>) -> Result<LoadResult> {
    load_from(engine_ref, iter_wasm_files())
}

/// Like [`load_plugins`], but JIT-compiles only the plugin whose discovered
/// name equals `name`. A one-shot CLI command drives exactly one plugin;
/// compiling every installed `.wasm` on a CPU-constrained CI runner starves the
/// liveliness-graph subscriber during startup, so the command's single graph
/// read lands before external tokens are processed. Compiling just the needed
/// plugin removes that startup cost.
pub fn load_plugin_named(engine_ref: Arc<CoreEngine>, name: &str) -> Result<LoadResult> {
    let paths: Vec<PathBuf> = discover_wasm_plugins()
        .into_iter()
        .filter(|(plugin_name, _)| plugin_name == name)
        .map(|(_, path)| path)
        .collect();

    let (plugins, failed) = load_from(engine_ref.clone(), paths)?;

    // `name` matched the filename-derived name; a plugin whose manifest name
    // differs from its filename stem would be missed here, yet the caller selects
    // by `manifest().name`. Fall back to loading everything rather than reporting
    // a real installed plugin as "not found" — the full cost is only paid for a
    // misnamed install.
    if plugins.is_empty() {
        return load_plugins(engine_ref);
    }

    Ok((plugins, failed))
}

fn plugin_kind_label(p: &WasmPlugin) -> &'static str {
    match p.bindings {
        PluginBindings::Cli(_) => "cli",
        PluginBindings::Tui(_) => "tui",
        #[cfg(feature = "web-plugins")]
        PluginBindings::Web(_) => "web",
    }
}

pub fn discover_wasm_plugins() -> Vec<(String, PathBuf)> {
    let mut result: Vec<(String, PathBuf)> = iter_wasm_files()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            // Compiled artifacts normalize hyphens to underscores
            // ("hu-meter" -> "hu_meter.wasm"), but installed plugins may use
            // either "hu_" or "hu-" (both documented) — strip whichever is present.
            let name = stem
                .strip_prefix("hu_")
                .or_else(|| stem.strip_prefix("hu-"))
                .unwrap_or(stem)
                .to_string();
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

/// Sanitize a filename-derived plugin stem into one safe path segment. The stem
/// comes from an on-disk filename, so e.g. `hu-..\.wasm` could yield `..` and
/// let the work dir escape its base (path traversal). Map every character
/// outside `[A-Za-z0-9_-]` — including `.` and path separators — to `_`, so the
/// result is always a single safe segment (`..` becomes `__`); fall back to
/// `"unknown"` only for an empty stem.
fn sanitize_plugin_stem(plugin_stem: &str) -> String {
    let cleaned: String = plugin_stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn plugin_work_dir(plugin_stem: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hu")
        .join("plugin-work")
        .join(sanitize_plugin_stem(plugin_stem))
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
    for var in &["HU_CONNECT", "HU_DOMAIN", "HOME", "PATH", "RUST_LOG"] {
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
    // Discovery accepts both `hu-` and `hu_` filename prefixes, so strip either
    // to keep `hu-meter.wasm` and `hu_meter.wasm` mapping to the same work dir.
    let plugin_stem = plugin_stem
        .strip_prefix("hu-")
        .or_else(|| plugin_stem.strip_prefix("hu_"))
        .unwrap_or(plugin_stem);
    let work_dir = plugin_work_dir(plugin_stem);
    std::fs::create_dir_all(&work_dir).ok();

    let component = Component::from_file(wasm_engine, path)
        .with_context(|| format!("compiling {}", path.display()))?;

    // Probe CLI world first (most restrictive — fewest dead arms in plugins).
    if let Ok(plugin) = try_load_cli(wasm_engine, &component, &work_dir, engine_ref.clone()) {
        return Ok(plugin);
    }

    // Probe Web world.
    #[cfg(feature = "web-plugins")]
    if let Ok(plugin) = try_load_web(wasm_engine, &component, &work_dir, engine_ref.clone()) {
        return Ok(plugin);
    }

    // Fall back to the TUI world (hu-tui-plugin).
    try_load_tui(wasm_engine, &component, &work_dir, engine_ref)
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
try_load!(try_load_tui, HuTuiPlugin, Tui);
#[cfg(feature = "web-plugins")]
try_load!(try_load_web, web_bindgen::HuWebPlugin, Web);

fn open_declared_sessions(
    store: &mut Store<PluginState>,
    manifest: &hu::plugin::types::PluginManifest,
) -> Result<()> {
    // The manifest is untrusted and opening a session makes an outbound Zenoh
    // connection, so gate it like the runtime `open_session` host call: refuse
    // all declared sessions unless the plugin was granted `OpenSession`.
    if !manifest.sessions.is_empty()
        && !manifest
            .required_permissions
            .contains(&hu::plugin::types::Permission::OpenSession)
    {
        anyhow::bail!(
            "plugin declares {} session(s) but did not request the OpenSession permission",
            manifest.sessions.len()
        );
    }

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
        // Serialize the (untrusted) manifest endpoint through serde_json so any
        // quotes/brackets are escaped rather than injected into the JSON5 config.
        let endpoints_json = serde_json::to_string(&[endpoint.as_str()])
            .map_err(|e| anyhow::anyhow!("session '{name}': encode endpoint: {e}"))?;
        config
            .insert_json5("connect/endpoints", &endpoints_json)
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

#[cfg(test)]
mod tests {
    use super::{HostBlockGuard, epoch_should_tick};

    // The ticker's only guard against charging network waits to the guest's
    // compute budget is this counter, and it is one `fetch_add` away from being
    // silently dropped by a later edit. (Serial by construction: nothing else in
    // this crate's unit tests takes the guard, since host calls need a store.)
    #[test]
    fn host_block_guard_suspends_the_epoch_ticker() {
        assert!(epoch_should_tick(), "ticker suspended before any guard");
        {
            let _outer = HostBlockGuard::enter();
            assert!(!epoch_should_tick(), "guard did not suspend the ticker");
            {
                let _inner = HostBlockGuard::enter();
                assert!(!epoch_should_tick(), "nested guard un-suspended it");
            }
            assert!(
                !epoch_should_tick(),
                "dropping the inner guard resumed ticking while the outer is held"
            );
        }
        assert!(epoch_should_tick(), "ticker never resumed");
    }
}
