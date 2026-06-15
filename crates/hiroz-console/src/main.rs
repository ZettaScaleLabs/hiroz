use std::{sync::Arc, time::Duration};

mod app;
mod core;
mod export;
mod modes;

use core::engine::CoreEngine;

use app::{
    App, FocusPane, PAGE_SCROLL_AMOUNT, POLL_TIMEOUT_MS, Panel, QUICK_MEASURE_DURATION_SECS,
};
use clap::{Parser, ValueEnum};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use export::export_and_exit;
use ratatui::{Terminal, backend::CrosstermBackend};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Default)]
enum Backend {
    /// rmw_zenoh backend (default) - compatible with rmw_zenoh nodes
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
#[command(name = "hiroz-console")]
#[command(about = "ROS2 Graph Inspector & Dataflow Monitor")]
struct Cli {
    /// Zenoh router address
    #[arg(default_value = "tcp/127.0.0.1:7447")]
    router: String,

    /// ROS domain ID
    #[arg(default_value = "0")]
    domain: usize,

    #[arg(long, value_enum, default_value = "rmw-zenoh")]
    backend: Backend,

    /// Enable TUI interface (default if no other mode specified)
    #[arg(long)]
    tui: bool,

    /// Headless mode: JSON streaming to stdout
    #[arg(long)]
    headless: bool,

    /// Output structured JSON logs
    #[arg(long)]
    json: bool,

    /// Enable debug logging
    #[arg(long)]
    debug: bool,

    /// Export current state and exit
    #[arg(long)]
    export: Option<String>, // graph.json, graph.dot, metrics.csv

    /// Topics to echo (subscribe and display messages)
    #[arg(long = "echo", value_name = "TOPIC")]
    echo_topics: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    // Initialize logger
    core::logger::init_logger(cli.json, cli.debug);

    // Create core engine
    let core = Arc::new(CoreEngine::new(&cli.router, cli.domain, cli.backend).await?);
    core.start_monitoring().await;

    tracing::info!(
        router = cli.router,
        domain = cli.domain,
        backend = ?cli.backend,
        "Connected to Zenoh router"
    );

    // Handle export mode
    if let Some(export_path) = cli.export {
        return export_and_exit(&core, &export_path).await;
    }

    // Determine mode
    if cli.headless {
        modes::headless::run_headless_mode(&core, cli.json, cli.echo_topics).await?;
    } else {
        run_tui_mode(core).await?;
    }

    Ok(())
}

async fn run_tui_mode(
    core: Arc<CoreEngine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(core).await?;

    // Main loop
    let result = run_tui_loop(&mut terminal, &mut app).await;

    // Restore terminal
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
        // Update caches periodically
        app.update_graph_cache();
        app.update_metrics();
        app.update_multi_metrics();

        // Render
        terminal.draw(|f| app.render(f))?;

        // Check for quit
        if app.quit {
            return Ok(());
        }

        // Poll for events with timeout
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
    // Ctrl+C quits from any mode
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return Ok(());
    }

    // Handle filter mode input separately
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

    // Handle help overlay
    if app.show_help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                app.show_help = false;
            }
            _ => {}
        }
        return Ok(());
    }

    // Normal mode key handling
    match key.code {
        // Quit
        KeyCode::Char('q') => app.quit = true,

        // Help
        KeyCode::Char('?') => app.show_help = true,

        // Navigation
        KeyCode::Up | KeyCode::Char('k') => {
            if app.focus_pane == FocusPane::List {
                app.select_previous();
            } else {
                // Navigate sections in detail pane
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
                // Navigate sections in detail pane
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

        // Page navigation
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
            // Select last item
            let max = match app.current_panel {
                Panel::Topics => app.cached_topics.len().saturating_sub(1),
                Panel::Nodes => app.cached_nodes.len().saturating_sub(1),
                Panel::Services => app.cached_services.len().saturating_sub(1),
                Panel::Measure => 0,
            };
            app.selected_index = max;
        }

        // Panel switching
        KeyCode::Tab => {
            app.current_panel = match app.current_panel {
                Panel::Topics => Panel::Services,
                Panel::Services => Panel::Nodes,
                Panel::Nodes => Panel::Measure,
                Panel::Measure => Panel::Topics,
            };
            app.selected_index = 0;
            app.detail_scroll = 0;
        }
        KeyCode::BackTab => {
            app.current_panel = match app.current_panel {
                Panel::Topics => Panel::Measure,
                Panel::Services => Panel::Topics,
                Panel::Nodes => Panel::Services,
                Panel::Measure => Panel::Nodes,
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

        // Detail section expansion (Enter or Space)
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.focus_pane == FocusPane::Detail {
                // Toggle section expansion based on current panel
                match app.current_panel {
                    Panel::Nodes => {
                        // Nodes panel: toggle all type name expansion
                        app.detail_state.publishers_expanded =
                            !app.detail_state.publishers_expanded;
                    }
                    _ => {
                        // Topics/Services: toggle individual sections
                        match app.detail_state.selected_section {
                            app::DetailSection::Publishers => {
                                app.detail_state.publishers_expanded =
                                    !app.detail_state.publishers_expanded;
                            }
                            app::DetailSection::Subscribers => {
                                app.detail_state.subscribers_expanded =
                                    !app.detail_state.subscribers_expanded;
                            }
                            app::DetailSection::Clients => {
                                app.detail_state.clients_expanded =
                                    !app.detail_state.clients_expanded;
                            }
                        }
                    }
                }
            } else {
                // Switch to detail pane
                app.focus_pane = FocusPane::Detail;
            }
        }

        // Back / Escape
        KeyCode::Esc if app.focus_pane == FocusPane::Detail => {
            app.focus_pane = FocusPane::List;
        }

        // Filter mode
        KeyCode::Char('/') => app.enter_filter_mode(),

        // Rate measurement (quick) or clear measurements
        KeyCode::Char('r') => {
            if app.current_panel == Panel::Measure {
                // Clear all measurements in Measure tab
                app.clear_measuring_topics();
            } else if app.current_panel == Panel::Topics
                && !app.cached_topics.is_empty()
                && let Some((topic, _)) = app.cached_topics.get(app.selected_index)
            {
                // Quick rate check in Topics tab
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

        // Toggle topic in measurement list (m) - don't jump to Measure tab
        KeyCode::Char('m') => {
            match app.current_panel {
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
                    // For nodes, we could add all their topics to measurement
                    // For now, just show a message
                    app.set_temp_status("Use Topics tab to add topics to measurement".to_string());
                }
                Panel::Measure => {
                    // Already in measurement panel - do nothing
                }
            }
        }

        // Screenshot
        KeyCode::Char('S') => {
            app.take_screenshot = true;
            // The actual screenshot capture and notification happens in render_detail
        }

        // Export rate cache to CSV
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

        // Toggle recording of measuring topics
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
