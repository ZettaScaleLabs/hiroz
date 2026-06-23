use std::{sync::Arc, time::Duration};

mod app;
mod core;
mod export;
mod modes;
mod plugin;

use core::engine::CoreEngine;

use app::{
    App, FocusPane, PAGE_SCROLL_AMOUNT, POLL_TIMEOUT_MS, Panel, QUICK_MEASURE_DURATION_SECS,
};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
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

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List installed WASM plugins
    #[command(name = "plugin")]
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List all installed hu-* plugins
    List,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Pre-clap check: if argv[1] is a WASM plugin name, route to CLI mode
    // before clap sees it (clap would reject it as an unknown subcommand).
    let raw_args: Vec<String> = std::env::args().collect();
    if let Some(candidate) = raw_args.get(1).filter(|a| !a.starts_with('-')) {
        let available = plugin::wasm::discover_wasm_plugins();
        if available.iter().any(|(n, _)| n == candidate.as_str()) {
            let plugin_name = candidate.clone();
            let tail = raw_args[2..].to_vec();
            let router = extract_named_flag(&tail, "--router")
                .or_else(|| std::env::var("HU_ROUTER").ok())
                .unwrap_or_else(|| "tcp/127.0.0.1:7447".to_string());
            let domain: usize = extract_named_flag(&tail, "--domain")
                .or_else(|| std::env::var("HU_DOMAIN").ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // Filter hu-level flags from args forwarded to the plugin.
            let plugin_args = filter_hu_flags(tail, &["--router", "--domain"]);
            let core =
                Arc::new(CoreEngine::new(&router, domain, core::engine::Backend::RmwZenoh).await?);
            core.start_monitoring().await;
            let code = modes::cli::run_cli_plugin(core, &plugin_name, plugin_args).await?;
            std::process::exit(code as i32);
        }
    }

    let cli = Cli::parse();

    core::logger::init_logger(cli.json, cli.debug);

    // Resolve router/domain from env vars (set by hu when dispatching to plugins)
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

    if cli.headless {
        modes::headless::run_headless_mode(&core, cli.json, cli.echo_topics).await?;
    } else {
        run_tui_mode(core).await?;
    }

    Ok(())
}

/// Return the value of `--flag <value>` from a raw arg list, if present.
fn extract_named_flag(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(val) = a.strip_prefix(&format!("{flag}=")) {
            return Some(val.to_string());
        }
    }
    None
}

/// Remove `--flag <value>` pairs from an arg list. Flags whose value follows
/// as the next element (not `--flag=value`) are detected by their presence in
/// `named_flags`. Unknown flags and positional args are kept as-is.
fn filter_hu_flags(args: Vec<String>, named_flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if named_flags.contains(&a.as_str()) {
            it.next(); // consume the value
        } else if named_flags.iter().any(|f| a.starts_with(&format!("{f}="))) {
            // skip --flag=value form
        } else {
            out.push(a);
        }
    }
    out
}

fn run_plugin_list(json: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

async fn run_tui_mode(
    core: Arc<CoreEngine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(core.clone()).await?;
    app.wasm_plugins = plugin::wasm::load_plugins(core);
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

        if app.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(POLL_TIMEOUT_MS))?
            && let Event::Key(key) = event::read()?
        {
            handle_key_event(app, key).await?;
        }
    }
}

async fn handle_key_event(
    app: &mut App,
    key: event::KeyEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return Ok(());
    }

    if app.filter_mode {
        match key.code {
            KeyCode::Esc => app.exit_filter_mode(),
            KeyCode::Enter => app.exit_filter_mode(),
            KeyCode::Backspace => app.delete_filter_char(),
            KeyCode::Left => app.move_filter_cursor_left(),
            KeyCode::Right => app.move_filter_cursor_right(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_filter()
            }
            KeyCode::Char(c) => app.enter_filter_char(c),
            _ => {}
        }
        return Ok(());
    }

    if app.show_help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                app.show_help = false;
            }
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.show_help = true,

        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_pane == FocusPane::List {
                app.select_previous();
            } else {
                app.detail_state.selected_section = match app.detail_state.selected_section {
                    app::DetailSection::Publishers => app::DetailSection::Clients,
                    app::DetailSection::Subscribers => app::DetailSection::Publishers,
                    app::DetailSection::Clients => app::DetailSection::Subscribers,
                };
                app.scroll_detail_up();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.focus_pane == FocusPane::List {
                app.select_next();
            } else {
                app.detail_state.selected_section = match app.detail_state.selected_section {
                    app::DetailSection::Publishers => app::DetailSection::Subscribers,
                    app::DetailSection::Subscribers => app::DetailSection::Clients,
                    app::DetailSection::Clients => app::DetailSection::Publishers,
                };
                app.scroll_detail_down();
            }
        }
        KeyCode::Left | KeyCode::Char('h') if app.focus_pane == FocusPane::Detail => {
            app.focus_pane = FocusPane::List;
        }
        KeyCode::Right | KeyCode::Char('l') if app.focus_pane == FocusPane::List => {
            app.focus_pane = FocusPane::Detail;
        }

        KeyCode::PageUp => {
            for _ in 0..PAGE_SCROLL_AMOUNT {
                if app.focus_pane == FocusPane::List {
                    app.select_previous();
                } else {
                    app.scroll_detail_up();
                }
            }
        }
        KeyCode::PageDown => {
            for _ in 0..PAGE_SCROLL_AMOUNT {
                if app.focus_pane == FocusPane::List {
                    app.select_next();
                } else {
                    app.scroll_detail_down();
                }
            }
        }
        KeyCode::Home => {
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::End => {
            let max = match app.current_panel {
                Panel::Topics => app.cached_topics.len().saturating_sub(1),
                Panel::Nodes => app.cached_nodes.len().saturating_sub(1),
                Panel::Services => app.cached_services.len().saturating_sub(1),
                Panel::Measure | Panel::Plugins => 0,
            };
            app.selected_index = max;
        }

        KeyCode::Tab => {
            app.current_panel = match app.current_panel {
                Panel::Topics => Panel::Services,
                Panel::Services => Panel::Nodes,
                Panel::Nodes => Panel::Measure,
                Panel::Measure => Panel::Plugins,
                Panel::Plugins => Panel::Topics,
            };
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::BackTab => {
            app.current_panel = match app.current_panel {
                Panel::Topics => Panel::Plugins,
                Panel::Services => Panel::Topics,
                Panel::Nodes => Panel::Services,
                Panel::Measure => Panel::Nodes,
                Panel::Plugins => Panel::Measure,
            };
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::Char('1') => {
            app.current_panel = Panel::Topics;
            app.selected_index = 0;
        }
        KeyCode::Char('2') => {
            app.current_panel = Panel::Services;
            app.selected_index = 0;
        }
        KeyCode::Char('3') => {
            app.current_panel = Panel::Nodes;
            app.selected_index = 0;
        }
        KeyCode::Char('4') => {
            app.current_panel = Panel::Measure;
            app.selected_index = 0;
        }
        KeyCode::Char('5') => {
            app.current_panel = Panel::Plugins;
            app.plugin_selected_index = 0;
        }
        KeyCode::Char('t') if app.current_panel == Panel::Plugins => {
            let idx = app.plugin_selected_index;
            if idx < app.wasm_plugins.len() {
                app.wasm_plugins[idx]
                    .dispatch_event(crate::plugin::wasm::hu::plugin::types::PluginEvent::Tick);
                // exit_code is ignored in TUI mode — plugin pane stays open
            }
        }

        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.focus_pane == FocusPane::Detail {
                match app.current_panel {
                    Panel::Nodes => {
                        app.detail_state.publishers_expanded =
                            !app.detail_state.publishers_expanded;
                    }
                    _ => match app.detail_state.selected_section {
                        app::DetailSection::Publishers => {
                            app.detail_state.publishers_expanded =
                                !app.detail_state.publishers_expanded;
                        }
                        app::DetailSection::Subscribers => {
                            app.detail_state.subscribers_expanded =
                                !app.detail_state.subscribers_expanded;
                        }
                        app::DetailSection::Clients => {
                            app.detail_state.clients_expanded = !app.detail_state.clients_expanded;
                        }
                    },
                }
            } else {
                app.focus_pane = FocusPane::Detail;
            }
        }

        KeyCode::Esc if app.focus_pane == FocusPane::Detail => {
            app.focus_pane = FocusPane::List;
        }

        KeyCode::Char('/') => app.enter_filter_mode(),

        KeyCode::Char('r') => {
            if app.current_panel == Panel::Measure {
                app.clear_measuring_topics();
            } else if app.current_panel == Panel::Topics
                && !app.cached_topics.is_empty()
                && let Some((topic, _)) = app.cached_topics.get(app.selected_index)
            {
                let topic = topic.clone();
                app.status_message = format!("Measuring rate for {}...", topic);
                match app
                    .quick_measure_rate(&topic, QUICK_MEASURE_DURATION_SECS)
                    .await
                {
                    Ok(rate) => {
                        app.set_temp_status(format!("{}: {:.1} Hz", topic, rate));
                    }
                    Err(e) => {
                        app.set_temp_status(format!("Rate measurement failed: {}", e));
                    }
                }
            }
        }

        KeyCode::Char('m') => match app.current_panel {
            Panel::Topics => {
                if !app.cached_topics.is_empty()
                    && let Some((topic, _)) = app.cached_topics.get(app.selected_index)
                {
                    let topic = topic.clone();
                    app.toggle_measuring_topic(&topic).await;
                }
            }
            Panel::Services => {
                if !app.cached_services.is_empty()
                    && let Some((service, _)) = app.cached_services.get(app.selected_index)
                {
                    let service = service.clone();
                    app.toggle_measuring_topic(&service).await;
                }
            }
            Panel::Nodes => {
                app.set_temp_status("Use Topics tab to add topics to measurement".to_string());
            }
            Panel::Measure => {}
            Panel::Plugins => {}
        },

        KeyCode::Char('S') => {
            app.take_screenshot = true;
        }

        KeyCode::Char('e') => {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let filename = format!("hiroz-rates_{}.csv", timestamp);
            match app.export_metrics(&filename) {
                Ok(()) => {
                    app.set_temp_status(format!("Exported to {}", filename));
                }
                Err(e) => {
                    app.set_temp_status(format!("Export failed: {}", e));
                }
            }
        }

        KeyCode::Char('w') => match app.toggle_recording() {
            Ok(msg) => {
                app.set_temp_status(msg);
            }
            Err(e) => {
                app.set_temp_status(format!("{}", e));
            }
        },

        _ => {}
    }

    Ok(())
}
