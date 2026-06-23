//! TUI rendering module
//!
//! Split into focused submodules:
//! - `common`: Shared helper functions
//! - `topics`: Topic panel rendering
//! - `services`: Service panel rendering
//! - `nodes`: Node panel rendering
//! - `measure`: Measurement panel rendering

pub mod common;
mod measure;
mod nodes;
mod plugins;
mod services;
mod topics;

use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, Paragraph, Scrollbar, ScrollbarOrientation},
};

use crate::app::App;
use crate::app::state::*;

use common::{border_style, border_type};

impl App {
    /// Main render function
    pub fn render(&mut self, f: &mut Frame) {
        let chunks = if self.filter_mode {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Title bar
                    Constraint::Length(1), // Navigation bar
                    Constraint::Length(3), // Filter input bar
                    Constraint::Min(0),    // Main content
                    Constraint::Length(3), // Status bar
                ])
                .split(f.area())
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Title bar
                    Constraint::Length(1), // Navigation bar
                    Constraint::Min(0),    // Main content
                    Constraint::Length(3), // Status bar
                ])
                .split(f.area())
        };

        // Title with connection status
        let connection_info = match self.connection_status {
            ConnectionStatus::Connected => {
                format!("Connected to {}", self.core.router_addr)
            }
            ConnectionStatus::Disconnected => "Zenoh router: disconnected".to_string(),
        };
        let title = Paragraph::new(format!(
            " hiroz-console | Domain: {} | {} ",
            self.core.domain_id, connection_info
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(title, chunks[0]);

        // Navigation bar
        self.render_navigation(f, chunks[1]);

        let (main_area, status_area_index) = if self.filter_mode {
            // Filter input
            let filter_input = Paragraph::new(self.filter_input.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Filter (type to search) ")
                        .style(Style::default().fg(Color::Green)),
                );
            f.render_widget(filter_input, chunks[2]);

            // Set cursor position in filter input
            f.set_cursor_position(Position::new(
                chunks[2].x + self.filter_cursor as u16 + 1,
                chunks[2].y + 1,
            ));

            (chunks[3], 4)
        } else {
            (chunks[2], 3)
        };

        // Main content (split into list + detail)
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(LIST_PANE_PERCENTAGE),
                Constraint::Percentage(DETAIL_PANE_PERCENTAGE),
            ])
            .split(main_area);

        self.render_list(f, main_chunks[0]);
        self.render_detail(f, main_chunks[1]);

        // Build status message: use temp message if set, otherwise use focus-aware hints
        let status_text = if self.status_message_time.is_some() {
            // Temporary status message (e.g., "Screenshot saved", "Added topic to measurement")
            self.status_message.clone()
        } else {
            // Focus-aware hints with optional rate cache info
            let hint = self.get_status_hint();
            if self.current_panel == Panel::Topics && !self.monitor.rate_cache.is_empty() {
                let fresh_count = self
                    .monitor
                    .rate_cache
                    .values()
                    .filter(|c| {
                        Instant::now().duration_since(c.last_updated) < self.monitor.rate_cache_ttl
                    })
                    .count();
                if fresh_count > 0 {
                    format!("{} | {} cached", hint, fresh_count)
                } else {
                    hint
                }
            } else {
                hint
            }
        };

        // Status bar
        let status = Paragraph::new(status_text).block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        );
        f.render_widget(status, chunks[status_area_index]);

        // Render help panel as overlay if requested
        if self.show_help {
            self.render_help(f);
        }
    }

    fn render_navigation(&self, f: &mut Frame, area: Rect) {
        let panels = vec![
            ("1:Topics", Panel::Topics),
            ("2:Services", Panel::Services),
            ("3:Nodes", Panel::Nodes),
            ("4:Measure", Panel::Measure),
            ("5:Plugins", Panel::Plugins),
        ];

        let mut spans = Vec::new();
        for (label, panel) in panels {
            let style = if panel == self.current_panel {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::White)
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }

        let nav = Paragraph::new(Line::from(spans)).block(Block::default());
        f.render_widget(nav, area);
    }

    fn render_help(&self, f: &mut Frame) {
        let help_text = vec![
            "HIROZ Console - Key Bindings",
            "",
            "Navigation:",
            "  j/k            Navigate list items",
            "  h/l            Switch between list/detail panes",
            "  Tab             Cycle through panels",
            "  PgUp/PgDn       Page up/down in lists",
            "  Enter           Drill into item details",
            "  Esc             Go back / exit filter mode",
            "",
            "Actions:",
            "  1-4             Jump to panel",
            "  m               Start measurement",
            "  r               Quick rate check (2s)",
            "  Space           Toggle sections",
            "  /               Filter mode",
            "  e               Export metrics to CSV",
            "  w               Toggle recording",
            "  S               Screenshot",
            "",
            "Other:",
            "  ?               Show/hide help",
            "  q               Quit",
            "",
            "Press any key to close",
        ];

        // Create a centered floating panel
        let area = f.area();
        let help_width = 50;
        let help_height = help_text.len() as u16 + 4;

        let x = (area.width.saturating_sub(help_width)) / 2;
        let y = (area.height.saturating_sub(help_height)) / 2;

        let help_area = Rect {
            x,
            y,
            width: help_width.min(area.width),
            height: help_height.min(area.height),
        };

        // Clear the background first
        f.render_widget(ratatui::widgets::Clear, help_area);

        // Then render the help panel
        let help_paragraph = Paragraph::new(help_text.join("\n"))
            .block(
                Block::default()
                    .title(" Help (? to toggle) ")
                    .title_style(Style::default().fg(Color::Cyan))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
            )
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .alignment(Alignment::Left);

        f.render_widget(help_paragraph, help_area);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        // Update cache if it's stale
        if self.cache_timestamp.elapsed()
            > std::time::Duration::from_millis(self.config.graph_cache_update_ms)
        {
            self.update_graph_cache();
        }

        let filter_text = self.filter_input.to_lowercase();
        let list_width = area.width.saturating_sub(4) as usize;

        let (items, total_count): (Vec<ratatui::widgets::ListItem>, usize) =
            match self.current_panel {
                Panel::Topics => {
                    let total = self.cached_topics.len();
                    let items = self.render_topic_list_items(&filter_text, list_width);
                    (items, total)
                }
                Panel::Services => {
                    let total = self.cached_services.len();
                    let items = self.render_service_list_items(&filter_text, list_width);
                    (items, total)
                }
                Panel::Nodes => {
                    let total = self.cached_nodes.len();
                    let items = self.render_node_list_items(&filter_text, list_width);
                    (items, total)
                }
                Panel::Measure => {
                    let items = self.render_measure_list_items(list_width);
                    let total = self.monitor.measuring_topics.len();
                    (items, total)
                }
                Panel::Plugins => {
                    let total = self.plugin_count() + self.plugin_mgr.failed.len();
                    let items = self.render_plugin_list_items();
                    (items, total)
                }
            };

        // Clamp selected_index to filtered results
        if !items.is_empty() && self.selected_index >= items.len() {
            self.selected_index = items.len() - 1;
        }

        // Build title with filter info
        let panel_name = match self.current_panel {
            Panel::Topics => "Topics",
            Panel::Services => "Services",
            Panel::Nodes => "Nodes",
            Panel::Measure => "Monitoring",
            Panel::Plugins => "Plugins",
        };
        let title = if self.current_panel == Panel::Measure {
            format!(" {} ({}) ", panel_name, total_count)
        } else if !filter_text.is_empty() {
            format!(
                " {} ({}/{} filtered) ",
                panel_name,
                items.len(),
                total_count
            )
        } else {
            format!(" {} ", panel_name)
        };

        let is_focused = self.focus_pane == FocusPane::List;
        let list = List::new(items).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style(is_focused))
                .border_type(border_type(is_focused)),
        );

        f.render_widget(list, area);
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        match self.current_panel {
            Panel::Measure => self.render_measurement_panel(f, area),
            Panel::Plugins => self.render_plugin_output(f, area),
            _ => {
                let detail_text = match self.current_panel {
                    Panel::Topics => self.render_topic_detail(),
                    Panel::Services => self.render_service_detail(),
                    Panel::Nodes => self.render_node_detail(),
                    _ => String::new(),
                };

                self.render_scrollable_detail(f, area, &detail_text);
            }
        }

        // Capture screenshot if requested
        if self.take_screenshot {
            let buffer = f.buffer_mut();
            let mut screenshot = String::new();
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    let cell = &buffer[(x, y)];
                    screenshot.push_str(cell.symbol());
                }
                screenshot.push('\n');
            }
            self.take_screenshot = false;

            // Save to timestamped file
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let filename = format!("hiroz-screenshot_{}.txt", timestamp);
            match std::fs::write(&filename, &screenshot) {
                Ok(()) => {
                    self.set_temp_status(format!("Screenshot saved to {}", filename));
                }
                Err(e) => {
                    self.set_temp_status(format!("Screenshot failed: {}", e));
                }
            }
        }
    }

    fn render_scrollable_detail(&mut self, f: &mut Frame, area: Rect, detail_text: &str) {
        let lines: Vec<&str> = detail_text.lines().collect();
        let total_lines = lines.len();
        let visible_lines = area.height.saturating_sub(2) as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);

        // Clamp scroll position
        self.detail_scroll = self.detail_scroll.min(max_scroll);

        let is_focused = self.focus_pane == FocusPane::Detail;
        let detail = Paragraph::new(detail_text)
            .block(
                Block::default()
                    .title(" Detail ")
                    .borders(Borders::ALL)
                    .border_style(border_style(is_focused))
                    .border_type(border_type(is_focused)),
            )
            .scroll((self.detail_scroll as u16, 0));

        f.render_widget(detail, area);

        // Render scrollbar if content exceeds visible area
        if total_lines > visible_lines {
            self.detail_scroll_state = self
                .detail_scroll_state
                .content_length(total_lines)
                .position(self.detail_scroll);

            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("^"))
                    .end_symbol(Some("v")),
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut self.detail_scroll_state,
            );
        }
    }
}
