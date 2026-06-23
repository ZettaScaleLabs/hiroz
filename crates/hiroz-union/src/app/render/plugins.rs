use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
};

use crate::app::{App, state::FocusPane};

use super::common::{border_style, border_type};

impl App {
    pub fn render_plugin_list_items(&self) -> Vec<ListItem<'static>> {
        #[cfg(feature = "wasm-plugins")]
        let loaded_count = self.plugin_mgr.plugins.len();
        #[cfg(not(feature = "wasm-plugins"))]
        let loaded_count = 0usize;

        if loaded_count == 0 && self.plugin_mgr.failed.is_empty() {
            return vec![ListItem::new(Span::styled(
                "  No WASM plugins loaded",
                Style::default().fg(Color::DarkGray),
            ))];
        }

        let mut items: Vec<ListItem<'static>> = Vec::new();

        #[cfg(feature = "wasm-plugins")]
        for (i, plugin) in self.plugin_mgr.plugins.iter().enumerate() {
            let title = plugin.title.lock().clone();
            let display = if title.is_empty() {
                plugin.manifest.name.clone()
            } else {
                format!("{} — {}", plugin.manifest.name, title)
            };
            let style = if i == self.plugin_mgr.selected_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            items.push(ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(display, style),
            ])));
        }

        // Failed plugins shown below loaded ones with a red [FAILED] indicator.
        for (path, _err) in &self.plugin_mgr.failed {
            let stem = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path.as_str());
            let label = format!("  [FAILED] {stem}");
            items.push(ListItem::new(Span::styled(
                label,
                Style::default().fg(Color::Red),
            )));
        }

        items
    }

    pub fn render_plugin_output(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Detail;

        // Check if the selected index falls on a failed plugin.
        let failed_offset = self.plugin_count();
        if self.plugin_mgr.selected_index >= failed_offset {
            let fi = self.plugin_mgr.selected_index - failed_offset;
            if let Some((path, err)) = self.plugin_mgr.failed.get(fi) {
                let text = format!("Failed to load plugin:\n{path}\n\nError: {err}");
                let widget = Paragraph::new(text)
                    .block(
                        Block::default()
                            .title(" Plugin Load Error ")
                            .borders(Borders::ALL)
                            .border_style(ratatui::style::Style::default().fg(Color::Red))
                            .border_type(border_type(is_focused)),
                    )
                    .wrap(ratatui::widgets::Wrap { trim: false });
                f.render_widget(widget, area);
                return;
            }
        }

        #[cfg(not(feature = "wasm-plugins"))]
        {
            let placeholder = Paragraph::new("WASM plugin support not compiled in.").block(
                Block::default()
                    .title(" Plugin Output ")
                    .borders(Borders::ALL)
                    .border_style(border_style(is_focused))
                    .border_type(border_type(is_focused)),
            );
            f.render_widget(placeholder, area);
            return;
        }

        #[cfg(feature = "wasm-plugins")]
        {
            if self.plugin_mgr.plugins.is_empty()
                || self.plugin_mgr.selected_index >= self.plugin_mgr.plugins.len()
            {
                let placeholder = Paragraph::new("Select a plugin from the list").block(
                    Block::default()
                        .title(" Plugin Output ")
                        .borders(Borders::ALL)
                        .border_style(border_style(is_focused))
                        .border_type(border_type(is_focused)),
                );
                f.render_widget(placeholder, area);
                return;
            }

            let plugin = &self.plugin_mgr.plugins[self.plugin_mgr.selected_index];
            let lines: Vec<String> = plugin.output_lines.lock().clone();
            let text = lines.join("\n");
            let title = format!(" {} v{} ", plugin.manifest.name, plugin.manifest.version);

            let visible_lines = area.height.saturating_sub(2) as usize;
            let total_lines = lines.len();
            let scroll = total_lines.saturating_sub(visible_lines);

            let output = Paragraph::new(text)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(border_style(is_focused))
                        .border_type(border_type(is_focused)),
                )
                .scroll((scroll as u16, 0));

            f.render_widget(output, area);
        }
    }
}
