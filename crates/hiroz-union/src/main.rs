use std::{sync::Arc, time::Duration};

mod app;
mod core;
mod export;
mod modes;

#[cfg(feature = "wasm-plugins")]
mod plugin;

use core::engine::CoreEngine;

use app::{App, POLL_TIMEOUT_MS};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use export::export_and_exit;
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
    about = "Plugin platform and TUI for the hiroz ROS 2 ecosystem",
    disable_help_subcommand = true
)]
struct Cli {
    /// Zenoh router address
    #[arg(long, default_value = "tcp/127.0.0.1:7447", global = true)]
    router: String,

    /// ROS domain ID
    #[arg(long, default_value = "0", global = true)]
    domain: usize,

    #[arg(long, value_enum, default_value = "rmw-zenoh", global = true)]
    backend: Backend,

    /// Headless mode: JSON streaming to stdout
    #[arg(long, global = true)]
    headless: bool,

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

    /// Start the web plugin server on the given port (default: 8080)
    #[arg(long, value_name = "PORT", global = true)]
    web: Option<Option<u16>>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage installed WASM plugins
    #[command(name = "plugin")]
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Dispatch to a named WASM plugin (e.g. `hu meter hz /topic`)
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum PluginAction {
    /// List all installed hu-* plugins
    List,
    /// Validate a .wasm plugin file and report its manifest
    Validate {
        /// Path to the .wasm plugin file
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    core::logger::init_logger(cli.json, cli.debug);

    // Env vars override clap defaults so `HU_ROUTER=tcp/... hu meter hz /topic` works.
    let router = std::env::var("HU_ROUTER").unwrap_or(cli.router);
    let domain: usize = std::env::var("HU_DOMAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cli.domain);

    match cli.command {
        Some(Commands::Plugin {
            action: PluginAction::List,
        }) => {
            return run_plugin_list(cli.json);
        }
        Some(Commands::Plugin {
            action: PluginAction::Validate { path },
        }) => {
            return run_plugin_validate(&path, cli.json);
        }
        Some(Commands::External(args)) => {
            #[cfg(feature = "wasm-plugins")]
            {
                let plugin_name = args[0].clone();
                let plugin_args = args[1..].to_vec();
                let core = Arc::new(CoreEngine::new(&router, domain, cli.backend).await?);
                core.start_monitoring().await;
                let code = modes::cli::run_cli_plugin(core, &plugin_name, plugin_args).await?;
                std::process::exit(code as i32);
            }
            #[cfg(not(feature = "wasm-plugins"))]
            {
                eprintln!(
                    "error: unknown subcommand '{}' (WASM plugin support not compiled in)",
                    args[0]
                );
                std::process::exit(1);
            }
        }
        None => {}
    }

    let core = Arc::new(CoreEngine::new(&router, domain, cli.backend).await?);
    core.start_monitoring().await;

    tracing::info!(
        router = router,
        domain = domain,
        "Connected to Zenoh router"
    );

    if let Some(export_path) = cli.export {
        return export_and_exit(&core, &export_path).await;
    }

    if let Some(port_opt) = cli.web {
        let port = port_opt.unwrap_or(8080);
        modes::web::run_web_mode(core, port).await?;
    } else if cli.headless {
        modes::headless::run_headless_mode(&core, cli.json, cli.echo_topics).await?;
    } else {
        run_tui_mode(core).await?;
    }

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
        if json {
            let entries: Vec<_> = plugins
                .iter()
                .map(|(name, path)| {
                    serde_json::json!({
                        "name": name,
                        "path": path.to_string_lossy(),
                        "kind": "wasm",
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        } else {
            if plugins.is_empty() {
                println!("No WASM plugins found in $HU_PLUGIN_PATH or ~/.local/share/hu/plugins/.");
                return Ok(());
            }
            println!("{:<20} PATH", "PLUGIN");
            println!("{}", "-".repeat(60));
            for (name, path) in &plugins {
                println!("{:<20} {}", name, path.to_string_lossy());
            }
        }
        Ok(())
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
                    println!("(Use --router to connect and read the full plugin manifest)");
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
        app.plugin_mgr.plugins = plugins;
        app.plugin_mgr.failed = failed;
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

        terminal.draw(|f| app.render(f))?;

        if event::poll(Duration::from_millis(POLL_TIMEOUT_MS))?
            && let Event::Key(key) = event::read()?
            && app::input::handle_key(app, key).await?
        {
            return Ok(());
        }
    }
}
