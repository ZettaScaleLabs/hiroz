use std::{sync::Arc, time::Duration};

mod app;
mod core;
mod export;
mod frontend;
mod modes;

#[cfg(feature = "wasm-plugins")]
mod plugin;

use core::engine::CoreEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

use app::{App, POLL_TIMEOUT_MS};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Default)]
enum Backend {
    #[default]
    RmwZenoh,
}

impl From<Backend> for core::engine::Backend {
    fn from(backend: Backend) -> Self {
        match backend {
            Backend::RmwZenoh => core::engine::Backend::RmwZenoh,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "hu",
    // Releases ship versioned artifacts and the install docs tell users to run
    // `hu --version` to check what they got, so the binary has to answer.
    version,
    about = "Plugin platform and TUI for the hiroz ROS 2 ecosystem",
    disable_help_subcommand = true
)]
struct Cli {
    /// Zenoh router address to connect to
    #[arg(long, default_value = "tcp/127.0.0.1:7447", global = true)]
    connect: String,

    /// ROS domain ID
    #[arg(long, default_value = "0", global = true)]
    domain: usize,

    #[arg(long, value_enum, default_value = "rmw-zenoh", global = true)]
    backend: Backend,

    /// Output structured JSON logs
    #[arg(long, global = true)]
    json: bool,

    /// Enable debug logging
    #[arg(long, global = true)]
    debug: bool,

    /// Export current state and exit
    #[arg(long, global = true)]
    export: Option<String>,

    /// Topics to echo (subscribe and display messages)
    #[arg(long = "echo", value_name = "TOPIC", global = true)]
    echo_topics: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive TUI (this is also what bare `hu` does)
    Tui,
    /// Serve the web plugin UI over HTTP
    Web {
        /// Port to listen on (default 8080)
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },
    /// Stream graph and events as newline-delimited JSON, no TUI
    Stream,
    /// Manage installed WASM plugins
    #[command(name = "plugin")]
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Start an embedded Zenoh router (rmw_zenoh_cpp compatible)
    Router {
        /// Endpoints to listen on. Repeat to open several listeners.
        /// Defaults to `tcp/[::]:7447`.
        #[arg(short, long, value_name = "ENDPOINT")]
        listen: Vec<String>,

        /// JSON5 or YAML router config file (overrides `--listen`).
        #[arg(short, long, value_name = "PATH")]
        config: Option<String>,
    },
    /// Dispatch to a named WASM plugin (e.g. `hu meter hz /topic`)
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum PluginAction {
    /// List all installed hu-* plugins
    List,
    /// Validate that a .wasm file compiles as a WASM component
    Validate {
        /// Path to the .wasm plugin file
        path: String,
    },
    /// Install a plugin from a local file, a URL, or a name in the registry
    Install {
        /// Path to a .wasm file, a URL, or a plugin name (e.g. `meter`)
        source: String,

        /// Plugin index URL to resolve a name against (also HU_PLUGIN_REGISTRY)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    /// Remove an installed plugin
    Uninstall {
        /// Plugin name as shown by `hu plugin list` (e.g. `meter`)
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    core::logger::init_logger(cli.json, cli.debug);

    // Env vars override clap defaults so `HU_CONNECT=tcp/... hu meter hz /topic` works.
    let router = std::env::var("HU_CONNECT").unwrap_or(cli.connect);
    let domain: usize = std::env::var("HU_DOMAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cli.domain);

    // Platform-management commands need no engine and return immediately.
    match &cli.command {
        Some(Commands::Plugin {
            action: PluginAction::List,
        }) => return run_plugin_list(cli.json),
        Some(Commands::Plugin {
            action: PluginAction::Validate { path },
        }) => return run_plugin_validate(path, cli.json),
        Some(Commands::Plugin {
            action: PluginAction::Install { source, registry },
        }) => return run_plugin_install(source, registry.as_deref(), cli.json),
        Some(Commands::Plugin {
            action: PluginAction::Uninstall { name },
        }) => return run_plugin_uninstall(name, cli.json),
        Some(Commands::Router { listen, config }) => {
            return run_router(listen.clone(), config.clone()).await;
        }
        _ => {}
    }

    // Everything else is a frontend over a shared CoreEngine.
    let core = Arc::new(CoreEngine::new(&router, domain, cli.backend).await?);
    core.start_monitoring().await;
    tracing::info!(router = router, domain = domain, "Using Zenoh router");

    let code = match cli.command {
        Some(Commands::Tui) => frontend::launch(frontend::Tui, core, &router).await?,
        Some(Commands::Web { port }) => {
            frontend::launch(
                frontend::Web {
                    port: port.unwrap_or(8080),
                },
                core,
                &router,
            )
            .await?
        }
        Some(Commands::Stream) => {
            frontend::launch(
                frontend::Stream {
                    json: cli.json,
                    echo: cli.echo_topics,
                },
                core,
                &router,
            )
            .await?
        }
        Some(Commands::External(args)) => run_cli_frontend(args, core, &router).await?,
        // Plugin/Router are handled and returned above.
        Some(Commands::Plugin { .. }) | Some(Commands::Router { .. }) => unreachable!(),
        // Default: bare `hu`, plus the deprecated `--export` flag.
        None => {
            if let Some(path) = cli.export {
                frontend::launch(frontend::Export { path }, core, &router).await?
            } else {
                frontend::launch(frontend::Tui, core, &router).await?
            }
        }
    };

    std::process::exit(code as i32)
}

/// Dispatch an unknown `hu <name> …` subcommand to the CLI frontend (a WASM
/// cli-plugin). Handles the `meter param load` file-read glue that plugins
/// can't do inside the sandbox.
#[cfg(feature = "wasm-plugins")]
async fn run_cli_frontend(
    args: Vec<String>,
    core: Arc<CoreEngine>,
    router: &str,
) -> Result<u8, BoxError> {
    let plugin_name = args[0].clone();
    let mut plugin_args = args[1..].to_vec();
    // `meter param load <node> <yaml-file>` needs real filesystem access, which
    // WASM plugins deliberately don't have. Read and parse the file here on the
    // host instead, and hand the plugin pre-parsed parameter data (JSON, which
    // it can already decode) rather than a path.
    if plugin_name == "meter"
        && plugin_args.first().map(String::as_str) == Some("param")
        && plugin_args.get(1).map(String::as_str) == Some("load")
    {
        let node = plugin_args.get(2).cloned().unwrap_or_default();
        let path = plugin_args.get(3).cloned().unwrap_or_default();
        let params = load_ros_param_yaml(&path, &node)
            .map_err(|e| format!("failed to read/parse param file '{path}': {e}"))?;
        plugin_args = vec![
            "param".to_string(),
            "load".to_string(),
            node,
            serde_json::to_string(&params)?,
        ];
    }
    frontend::launch(
        frontend::Cli {
            plugin: plugin_name,
            args: plugin_args,
        },
        core,
        router,
    )
    .await
}

#[cfg(not(feature = "wasm-plugins"))]
async fn run_cli_frontend(
    args: Vec<String>,
    _core: Arc<CoreEngine>,
    _router: &str,
) -> Result<u8, BoxError> {
    eprintln!(
        "error: unknown subcommand '{}' (WASM plugin support not compiled in)",
        args[0]
    );
    Ok(1)
}

/// Non-interactive modes (CLI plugins, headless, web, export) need a live
/// router to do anything useful, so fail fast with an actionable hint if none
/// appears shortly. The TUI deliberately does not call this — it starts anyway
/// and shows a disconnected banner until a router comes up.
async fn require_router(
    engine: &CoreEngine,
    router_addr: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if engine.wait_for_router(Duration::from_secs(3)).await {
        Ok(())
    } else {
        Err(crate::core::engine::router_connect_hint(router_addr).into())
    }
}

/// Start an embedded Zenoh router with rmw_zenoh_cpp-compatible settings and
/// block until Ctrl-C. This is the built-in replacement for
/// `cargo run --example zenoh_router`.
async fn run_router(
    listen: Vec<String>,
    config: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use hiroz::config::RouterConfigBuilder;

    let zenoh_config = if let Some(path) = &config {
        zenoh::Config::from_file(path)
            .map_err(|e| format!("failed to load router config '{path}': {e}"))?
    } else {
        let mut builder = RouterConfigBuilder::new();
        if listen.is_empty() {
            builder = builder.with_listen_port(7447);
        } else {
            for endpoint in &listen {
                builder = builder.with_listen_endpoint(endpoint);
            }
        }
        builder.build_config()?
    };

    let listen_info = if let Some(path) = &config {
        format!("config file {path}")
    } else if listen.is_empty() {
        "tcp/[::]:7447".to_string()
    } else {
        listen.join(", ")
    };

    let session = zenoh::open(zenoh_config).await?;

    tracing::info!(zid = %session.zid(), listen = %listen_info, "hu router started");
    println!("hu router (rmw_zenoh_cpp compatible) listening on {listen_info}");
    println!("ZID: {}", session.zid());
    println!("Press Ctrl-C to stop");

    tokio::signal::ctrl_c().await?;

    println!("\nhu router shutting down...");
    session.close().await?;
    Ok(())
}

fn run_plugin_list(json: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(not(feature = "wasm-plugins"))]
    {
        if json {
            println!("[]");
        } else {
            println!("WASM plugin support not compiled in (feature wasm-plugins is disabled).");
        }
        return Ok(());
    }
    #[cfg(feature = "wasm-plugins")]
    {
        let plugins = plugin::wasm::discover_wasm_plugins();
        // Installed plugins carry a version and provenance; ones dropped on
        // the path by hand do not, and the listing should say so rather than
        // present them as equivalent.
        let db = plugin::install::load_db();
        // Match the managed FILE, not just the name. Discovery can return the
        // same plugin name from `$HU_PLUGIN_PATH`, the executable-relative
        // prefix and the managed directory; matching on the name alone labels a
        // development build with the installed plugin's version and source.
        let managed_dir = plugin::install::install_dir().ok();
        let meta = |name: &str, path: &std::path::Path| {
            db.plugins.iter().find(|p| {
                p.name == name
                    && managed_dir
                        .as_ref()
                        .is_some_and(|d| d.join(&p.file) == *path)
            })
        };

        if json {
            let entries: Vec<_> = plugins
                .iter()
                .map(|(name, path)| {
                    let m = meta(name, path);
                    serde_json::json!({
                        "name": name,
                        "path": path.to_string_lossy(),
                        "kind": "wasm",
                        "version": m.and_then(|m| m.version.clone()),
                        "origin": m.and_then(|m| m.origin.clone()),
                        "source": m.map(|m| m.source.clone()).unwrap_or_else(|| "unmanaged".into()),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        } else {
            if plugins.is_empty() {
                println!("No WASM plugins found in $HU_PLUGIN_PATH or ~/.local/share/hu/plugins/.");
                println!("Install one with:  hu plugin install <file|url|name>");
                return Ok(());
            }
            println!("{:<16} {:<10} {:<10} PATH", "PLUGIN", "VERSION", "SOURCE");
            println!("{}", "-".repeat(78));
            for (name, path) in &plugins {
                let m = meta(name, path);
                println!(
                    "{:<16} {:<10} {:<10} {}",
                    name,
                    m.and_then(|m| m.version.as_deref()).unwrap_or("-"),
                    m.map(|m| source_label(&m.source)).unwrap_or("unmanaged"),
                    path.to_string_lossy()
                );
            }
        }
        Ok(())
    }
}

#[cfg(feature = "wasm-plugins")]
/// Render the recorded source kind. `InstalledEntry::source` holds one of
/// `local`, `url` or `registry`; anything else comes from a record written by
/// an older hu, and is shown verbatim rather than guessed at.
fn source_label(source: &str) -> &str {
    match source {
        "url" => "download",
        other => other,
    }
}

fn run_plugin_install(
    source: &str,
    registry: Option<&str>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(not(feature = "wasm-plugins"))]
    {
        let _ = (source, registry, json);
        eprintln!("WASM plugin support not compiled in.");
        std::process::exit(1);
    }
    #[cfg(feature = "wasm-plugins")]
    {
        match plugin::install::install(source, registry) {
            Ok(path) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "installed", "path": path.to_string_lossy()})
                    );
                } else {
                    println!("installed {}", path.display());
                    println!("verify with: hu plugin list");
                }
                Ok(())
            }
            Err(e) => {
                if json {
                    println!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                std::process::exit(1);
            }
        }
    }
}

fn run_plugin_uninstall(
    name: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(not(feature = "wasm-plugins"))]
    {
        let _ = (name, json);
        eprintln!("WASM plugin support not compiled in.");
        std::process::exit(1);
    }
    #[cfg(feature = "wasm-plugins")]
    {
        match plugin::install::uninstall(name) {
            Ok(path) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "removed", "path": path.to_string_lossy()})
                    );
                } else {
                    println!("removed {}", path.display());
                }
                Ok(())
            }
            Err(e) => {
                if json {
                    println!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                std::process::exit(1);
            }
        }
    }
}

fn run_plugin_validate(
    path: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(not(feature = "wasm-plugins"))]
    {
        let _ = (path, json);
        eprintln!("WASM plugin support not compiled in.");
        std::process::exit(1);
    }
    #[cfg(feature = "wasm-plugins")]
    {
        let p = std::path::Path::new(path);
        if !p.exists() {
            eprintln!("error: file not found: {}", p.display());
            std::process::exit(1);
        }
        if p.extension().and_then(|e| e.to_str()) != Some("wasm") {
            eprintln!("warning: file does not have .wasm extension");
        }
        match plugin::wasm::validate_plugin_static(p) {
            Ok(msg) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "ok", "path": path, "message": msg})
                    );
                } else {
                    println!("{msg}");
                    println!("(Use --connect to reach a router and read the full plugin manifest)");
                }
                Ok(())
            }
            Err(e) => {
                if json {
                    println!("{}", serde_json::json!({"error": e.to_string()}));
                } else {
                    eprintln!("error: {e}");
                }
                std::process::exit(1);
            }
        }
    }
}

async fn run_tui_mode(
    core: Arc<CoreEngine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(core.clone()).await?;
    #[cfg(feature = "wasm-plugins")]
    {
        let (plugins, failed) = plugin::wasm::load_plugins(core)?;
        app.plugin_mgr.set_plugins(plugins);
        app.plugin_mgr.failed = failed;
        // Let TUI plugins initialize before the first frame's tick.
        app.plugin_mgr.dispatch_startup();
    }
    let result = run_tui_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        app.update_graph_cache();
        app.update_metrics();
        app.update_multi_metrics();
        app.plugin_mgr.dispatch_tick_all();

        terminal.draw(|f| app.render(f))?;

        if event::poll(Duration::from_millis(POLL_TIMEOUT_MS))?
            && let Event::Key(key) = event::read()?
            && app::input::handle_key(app, key).await?
        {
            return Ok(());
        }
    }
}

/// Read a ROS 2 `ros2 param dump`-style YAML file and flatten the
/// `ros__parameters` map for the given node into a JSON array of
/// `[name, value]` pairs. Nested maps are flattened with `.`-joined names,
/// matching ROS 2's own parameter-name convention.
#[cfg(feature = "wasm-plugins")]
fn load_ros_param_yaml(
    path: &str,
    node: &str,
) -> Result<Vec<(String, serde_json::Value)>, Box<dyn std::error::Error + Send + Sync>> {
    let content = std::fs::read_to_string(path)?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&content)?;
    let doc = doc
        .as_mapping()
        .ok_or("expected a top-level YAML mapping")?;

    let node_key = node.trim_start_matches('/');
    let node_value = doc
        .iter()
        .find(|(k, _)| {
            k.as_str()
                .map(|s| s.trim_start_matches('/') == node_key)
                .unwrap_or(false)
        })
        .map(|(_, v)| v)
        .ok_or_else(|| format!("node '{node}' not found in param file"))?;

    let params = node_value
        .get("ros__parameters")
        .and_then(|v| v.as_mapping())
        .ok_or("expected 'ros__parameters' mapping under the node")?;

    fn flatten(
        prefix: &str,
        map: &serde_yaml::Mapping,
        out: &mut Vec<(String, serde_json::Value)>,
    ) {
        for (k, v) in map {
            let Some(key) = k.as_str() else { continue };
            let full_key = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            if let Some(nested) = v.as_mapping() {
                flatten(&full_key, nested, out);
            } else {
                let json_value: serde_json::Value =
                    serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
                out.push((full_key, json_value));
            }
        }
    }

    let mut out = Vec::new();
    flatten("", params, &mut out);
    Ok(out)
}
