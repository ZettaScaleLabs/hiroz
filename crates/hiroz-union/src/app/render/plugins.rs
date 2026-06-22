use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::app::{App, state::FocusPane};

use super::common::{border_style, border_type};

impl App {
    pub fn render_plugin_list_items(&self) -> Vec<ListItem<'static>> {
        if self.wasm_plugins.is_empty() {
            return vec![ListItem::new(Span::styled(
                "  No WASM plugins loaded",
                Style::default().fg(Color::DarkGray),
            ))];
        }

        self.wasm_plugins
            .iter()
            .enumerate()
            .map(|(i, plugin)| {
                let title = plugin.title.lock().unwrap().clone();
                let display = if title.is_empty() {
                    plugin.manifest.name.clone()
                } else {
                    format!("{} — {}", plugin.manifest.name, title)
                };
                let style = if i == self.plugin_selected_index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(display, style),
                ]))
            })
            .collect()
    }

    pub fn render_plugin_output(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Detail;

        if self.wasm_plugins.is_empty() || self.plugin_selected_index >= self.wasm_plugins.len() {
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

        let plugin = &self.wasm_plugins[self.plugin_selected_index];
        let lines: Vec<String> = plugin.output_lines.lock().unwrap().clone();
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
