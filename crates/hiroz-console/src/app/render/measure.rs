//! Measurement panel rendering

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph, Sparkline},
};

use crate::app::App;
use crate::app::state::*;

use super::common::*;

impl App {
    /// Get sorted list of measuring topics (ensures consistent ordering)
    fn get_sorted_measuring_topics(&self) -> Vec<String> {
        let mut topics: Vec<_> = self.measuring_topics.iter().cloned().collect();
        topics.sort();
        topics
    }

    /// Render list items for measuring topics
    pub fn render_measure_list_items(&self, list_width: usize) -> Vec<ListItem<'static>> {
        if self.measuring_topics.is_empty() {
            return vec![];
        }

        let metrics = self.topic_metrics.lock();
        let topics = self.get_sorted_measuring_topics();

        topics
            .iter()
            .enumerate()
            .map(|(i, topic)| {
                let is_selected = i == self.measure_selected_index;
                let style = list_item_style(is_selected);

                // Get rate info
                let rate_str = if let Some(tm) = metrics.get(topic) {
                    format!(" {:.1} Hz", tm.current_rate)
                } else {
                    String::new()
                };

                // Calculate available space for topic name
                let icon_width = 2; // "# "
                let rate_width = rate_str.len();
                let topic_max_width = list_width.saturating_sub(icon_width + rate_width);
                let display_topic = truncate_with_ellipsis(topic, topic_max_width);

                let mut spans = vec![Span::styled("# ".to_string(), style)];
                spans.push(Span::styled(display_topic, style));
                spans.push(Span::styled(rate_str, Style::default().fg(Color::Cyan)));

                ListItem::new(Line::from(spans))
            })
            .collect()
    }

    /// Render the measurement detail panel (sparklines for selected topic)
    pub fn render_measurement_panel(&self, f: &mut Frame, area: Rect) {
        let measuring_count = self.measuring_topics.len();

        if measuring_count == 0 {
            // No topics being monitored
            let info = Paragraph::new(
                "No topics being monitored.\n\n\
                 Go to Topics tab (press 1) and press 'm' on topics to add them.\n\n\
                 Keybindings:\n\
                   m - Toggle topic in measurement list (in Topics tab)\n\
                   r - Clear all measurements (in Measure tab)\n\
                   4 - Jump to this Measure tab",
            )
            .block(Block::default().title(" Sparklines ").borders(Borders::ALL));
            f.render_widget(info, area);
            return;
        }

        let topics = self.get_sorted_measuring_topics();
        let selected_topic = topics.get(self.measure_selected_index);

        let Some(topic) = selected_topic else {
            return;
        };

        let metrics = self.topic_metrics.lock();
        let Some(tm) = metrics.get(topic) else {
            let info = Paragraph::new("Waiting for data...").block(
                Block::default()
                    .title(format!(" {} ", topic))
                    .borders(Borders::ALL),
            );
            f.render_widget(info, area);
            return;
        };

        // Layout for sparklines
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Summary stats
                Constraint::Min(5),    // Rate sparkline
                Constraint::Min(5),    // Bandwidth sparkline
            ])
            .split(area);

        // Summary stats
        let summary = Paragraph::new(format!(
            "Rate: {:.1} Hz  |  Bandwidth: {:.2} KB/s",
            tm.current_rate, tm.current_bandwidth
        ))
        .block(
            Block::default()
                .title(format!(" {} ", topic))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        )
        .style(Style::default().fg(Color::White));
        f.render_widget(summary, chunks[0]);

        // Rate sparkline
        self.render_sparkline_with_axes(
            f,
            chunks[1],
            &tm.rate_history.iter().copied().collect::<Vec<_>>(),
            tm.current_rate,
            "Rate (msg/s)",
            Color::Cyan,
        );

        // Bandwidth sparkline
        self.render_sparkline_with_axes(
            f,
            chunks[2],
            &tm.bandwidth_history.iter().copied().collect::<Vec<_>>(),
            tm.current_bandwidth * 10.0,
            "Bandwidth (KB/s)",
            Color::Yellow,
        );
    }

    /// Render a sparkline with Y-axis labels and X-axis time indicator
    fn render_sparkline_with_axes(
        &self,
        f: &mut Frame,
        area: Rect,
        data: &[u64],
        current_value: f64,
        title: &str,
        color: Color,
    ) {
        if data.is_empty() {
            let empty = Paragraph::new("Waiting for data...").block(
                Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL),
            );
            f.render_widget(empty, area);
            return;
        }

        // Layout: [Y-axis labels (6 chars)] [Sparkline]
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(7), // Y-axis labels
                Constraint::Min(10),   // Sparkline area
            ])
            .split(area);

        // Calculate max for scaling
        let max_val = data.iter().max().copied().unwrap_or(1).max(1);
        let scale_factor = if title.contains("KB/s") { 10.0 } else { 1.0 };

        // Y-axis labels
        let y_axis_text = format!("{:>5.1}\n\n\n{:>5.1}", max_val as f64 / scale_factor, 0.0);
        let y_axis = Paragraph::new(y_axis_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right);
        f.render_widget(y_axis, h_chunks[0]);

        // Sparkline area with block
        let sparkline_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // Sparkline
                Constraint::Length(2), // X-axis (tick marks + labels)
            ])
            .split(h_chunks[1]);

        // Title with current value
        let display_value = current_value / scale_factor;
        let title_text = format!(
            " {} : {:.1} (max: {:.1}) ",
            title,
            display_value,
            max_val as f64 / scale_factor
        );

        let sparkline = Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::RIGHT | Borders::LEFT)
                    .title(title_text)
                    .title_style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
            )
            .data(data)
            .max(max_val)
            .style(Style::default().fg(color));
        f.render_widget(sparkline, sparkline_chunks[0]);

        // X-axis with per-second tick marks
        let width = sparkline_chunks[1].width as usize;

        // Build tick marks - one per ~10 seconds for readability
        let mut axis_chars = String::new();
        axis_chars.push('└');

        let tick_interval = if width > 60 { 10 } else { 15 }; // seconds per tick
        let chars_per_sec = (width.saturating_sub(2)) as f64 / HISTORY_LENGTH as f64;

        for i in 0..width.saturating_sub(2) {
            let sec = (i as f64 / chars_per_sec) as usize;
            if sec.is_multiple_of(tick_interval) && i > 0 {
                axis_chars.push('┴');
            } else {
                axis_chars.push('─');
            }
        }
        axis_chars.push('┘');

        // Time labels below axis
        let label_line = format!(
            " {}s{:width$}now",
            HISTORY_LENGTH,
            "",
            width = width.saturating_sub(8)
        );

        let x_axis = Paragraph::new(vec![
            Line::from(Span::styled(
                axis_chars,
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                label_line,
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        f.render_widget(x_axis, sparkline_chunks[1]);
    }
}
