//! Common rendering helpers

use hiroz::qos::{QosDurability, QosDuration, QosHistory, QosProfile, QosReliability};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
    widgets::BorderType,
};

use crate::app::state::*;

/// Convert protocol QosProfile to hiroz QosProfile for display
fn protocol_qos_to_hiroz(qos: &hiroz_protocol::qos::QosProfile) -> QosProfile {
    QosProfile {
        reliability: match qos.reliability {
            hiroz_protocol::qos::QosReliability::Reliable => QosReliability::Reliable,
            hiroz_protocol::qos::QosReliability::BestEffort => QosReliability::BestEffort,
        },
        durability: match qos.durability {
            hiroz_protocol::qos::QosDurability::TransientLocal => QosDurability::TransientLocal,
            hiroz_protocol::qos::QosDurability::Volatile => QosDurability::Volatile,
        },
        history: match qos.history {
            hiroz_protocol::qos::QosHistory::KeepLast(depth) => QosHistory::from_depth(depth),
            hiroz_protocol::qos::QosHistory::KeepAll => QosHistory::KeepAll,
        },
        deadline: QosDuration::INFINITE,
        lifespan: QosDuration::INFINITE,
        liveliness: hiroz::qos::QosLiveliness::Automatic,
        liveliness_lease_duration: QosDuration::INFINITE,
    }
}

/// Format QoS profile for TUI display
pub fn format_qos_detail(qos: &hiroz_protocol::qos::QosProfile) -> String {
    let qos = protocol_qos_to_hiroz(qos);
    let mut lines = Vec::new();
    lines.push(format!("    Reliability: {}", qos.reliability));
    lines.push(format!("    Durability: {}", qos.durability));
    lines.push(format!("    History: {}", qos.history));
    lines.push(format!("    Liveliness: {}", qos.liveliness));
    if qos.deadline != QosDuration::INFINITE {
        lines.push(format!("    Deadline: {}", qos.deadline));
    }
    if qos.lifespan != QosDuration::INFINITE {
        lines.push(format!("    Lifespan: {}", qos.lifespan));
    }
    if qos.liveliness_lease_duration != QosDuration::INFINITE {
        lines.push(format!(
            "    Lease Duration: {}",
            qos.liveliness_lease_duration
        ));
    }
    lines.join("\n")
}

/// Truncate text with ellipsis if it exceeds max_len
pub fn truncate_with_ellipsis(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }

    if max_len <= 3 {
        return "...".to_string();
    }

    format!("{}...", &text[..max_len.saturating_sub(3)])
}

/// Get expansion indicator string
pub fn expand_indicator(expanded: bool) -> &'static str {
    if expanded { "[-]" } else { "[+]" }
}

/// Get selection marker for section navigation
pub fn section_marker(focused: bool, selected: bool) -> &'static str {
    if focused && selected { " > " } else { "   " }
}

/// Create highlighted spans for filter matches
pub fn create_highlighted_spans(
    text: &str,
    filter_text: &str,
    base_style: Style,
) -> Vec<Span<'static>> {
    if filter_text.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let text_lower = text.to_lowercase();
    let filter_lower = filter_text.to_lowercase();

    let mut spans = Vec::new();
    let mut start = 0;

    while let Some(pos) = text_lower[start..].find(&filter_lower) {
        let match_start = start + pos;
        let match_end = match_start + filter_lower.len();

        // Add text before the match
        if match_start > start {
            spans.push(Span::styled(
                text[start..match_start].to_string(),
                base_style,
            ));
        }

        // Add highlighted match
        spans.push(Span::styled(
            text[match_start..match_end].to_string(),
            base_style.fg(Color::Red).add_modifier(Modifier::BOLD),
        ));

        start = match_end;
    }

    // Add remaining text
    if start < text.len() {
        spans.push(Span::styled(text[start..].to_string(), base_style));
    }

    spans
}

/// Format rate display string and color
pub fn format_rate(rate: f64, is_fresh: bool) -> (String, Color) {
    if !is_fresh {
        return (format!(" {:.0}*", rate), Color::DarkGray);
    }

    if rate >= RATE_THRESHOLD_KHZ {
        (format!(" {:.1}k", rate / RATE_THRESHOLD_KHZ), Color::Green)
    } else if rate >= RATE_THRESHOLD_HZ {
        (format!(" {:.0}", rate), Color::Green)
    } else if rate >= RATE_THRESHOLD_DHZ {
        (format!(" {:.1}", rate), Color::Green)
    } else if rate > 0.0 {
        (format!(" {:.2}", rate), Color::Green)
    } else {
        (" 0".to_string(), Color::DarkGray)
    }
}

/// Get style for selected/unselected list items
pub fn list_item_style(is_selected: bool) -> Style {
    if is_selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// Get border style based on focus state
pub fn border_style(is_focused: bool) -> Style {
    if is_focused {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    }
}

/// Get border type based on focus state
pub fn border_type(is_focused: bool) -> BorderType {
    if is_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    }
}
