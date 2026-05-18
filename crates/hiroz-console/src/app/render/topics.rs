//! Topic panel rendering

use std::time::Instant;

use hiroz::entity::{EndpointKind, entity_get_endpoint};
use ratatui::{
    text::{Line, Span},
    widgets::ListItem,
};

use crate::app::App;
use crate::app::state::*;

use super::common::*;

impl App {
    /// Render topic list items
    pub fn render_topic_list_items(
        &self,
        filter_text: &str,
        list_width: usize,
    ) -> Vec<ListItem<'static>> {
        let all_topics = &self.cached_topics;

        let filtered: Vec<_> = self.filter_items(all_topics, filter_text, |(topic, type_name)| {
            vec![topic.clone(), type_name.clone()]
        });

        filtered
            .iter()
            .enumerate()
            .map(|(i, (topic, _type_name))| {
                let is_selected = i == self.selected_index;
                let base_style = list_item_style(is_selected);

                // Check if topic is being measured
                let is_measuring = self.is_measuring(topic);
                let measure_indicator = if is_measuring { "[M] " } else { "" };
                let measure_color = ratatui::style::Color::Magenta;

                // Get rate info if available
                let (rate_str, rate_color) = if let Some(cached) = self.rate_cache.get(topic) {
                    let age = Instant::now().duration_since(cached.last_updated);
                    let is_fresh = age < self.rate_cache_ttl;
                    format_rate(cached.rate, is_fresh)
                } else {
                    ("".to_string(), ratatui::style::Color::Blue)
                };

                // Calculate available space for topic name
                let icon_width = 2; // "# "
                let indicator_width = measure_indicator.len();
                let rate_width = rate_str.len();
                let topic_max_width =
                    list_width.saturating_sub(icon_width + indicator_width + rate_width);
                let display_topic = truncate_with_ellipsis(topic, topic_max_width);

                // Build the line with highlighted topic
                let mut spans = vec![Span::styled("# ".to_string(), base_style)];

                // Add measuring indicator
                if is_measuring {
                    spans.push(Span::styled(
                        measure_indicator.to_string(),
                        ratatui::style::Style::default().fg(measure_color),
                    ));
                }

                spans.extend(create_highlighted_spans(
                    &display_topic,
                    filter_text,
                    base_style,
                ));
                spans.push(Span::styled(
                    rate_str,
                    ratatui::style::Style::default().fg(rate_color),
                ));

                ListItem::new(Line::from(spans))
            })
            .collect()
    }

    /// Render topic detail panel content
    pub fn render_topic_detail(&mut self) -> String {
        let graph = self.core.graph.lock();
        let filter_text = self.filter_input.to_lowercase();

        // Get filtered topics
        let filtered_topics: Vec<_> = graph
            .get_topic_names_and_types()
            .iter()
            .filter(|(topic, type_name)| {
                if filter_text.is_empty() {
                    true
                } else {
                    topic.to_lowercase().contains(&filter_text)
                        || type_name.to_lowercase().contains(&filter_text)
                }
            })
            .map(|(t, tn)| (t.clone(), tn.clone()))
            .collect();

        let Some((topic, type_name)) = filtered_topics.get(self.selected_index) else {
            return "No topic selected".to_string();
        };

        let mut detail = format!("Topic: {}\nType: {}\n", topic, type_name);

        // Show cached rate if available
        if let Some(cached) = self.rate_cache.get(topic) {
            let age = Instant::now().duration_since(cached.last_updated);
            if age < self.rate_cache_ttl {
                detail.push_str(&format!(
                    "Rate: {:.1} Hz (measured {}s ago)\n",
                    cached.rate,
                    age.as_secs()
                ));
            } else {
                detail.push_str(&format!(
                    "Rate: {:.1} Hz (stale, {}s old)\n",
                    cached.rate,
                    age.as_secs()
                ));
            }
        } else {
            detail.push_str("Rate: Not measured (press 'r')\n");
        }

        // Render publishers section
        let pub_entities = graph.get_entities_by_topic(EndpointKind::Publisher, topic);
        if !pub_entities.is_empty() {
            let is_focused = self.focus_pane == FocusPane::Detail;
            let is_selected = self.detail_state.selected_section == DetailSection::Publishers;

            detail.push_str(&format!(
                "\n{}{} Publishers ({}):",
                section_marker(is_focused, is_selected),
                expand_indicator(self.detail_state.publishers_expanded),
                pub_entities.len()
            ));

            if self.detail_state.publishers_expanded {
                detail.push_str("\n\n");
                for (idx, entity) in pub_entities.iter().enumerate() {
                    if let Some(endpoint) = entity_get_endpoint(entity) {
                        detail.push_str(&format!("   Publisher {}:\n", idx + 1));
                        match endpoint.node.as_ref() {
                            Some(node) => {
                                detail.push_str(&format!(
                                    "    Node: {}/{}\n",
                                    node.namespace, node.name
                                ));
                            }
                            None => detail.push_str("    Node: unknown\n"),
                        }

                        if let Some(ti) = &endpoint.type_info {
                            let hash_str = ti.hash.to_rihs_string();
                            let hash_short = if hash_str.len() > HASH_TRUNCATE_LEN {
                                format!("{}...", &hash_str[..HASH_TRUNCATE_LEN])
                            } else {
                                hash_str
                            };
                            detail.push_str(&format!("    Type: {} ({})\n", ti.name, hash_short));
                        }

                        detail.push_str(&format_qos_detail(&endpoint.qos));
                        detail.push_str("\n\n");
                    }
                }
            } else {
                detail.push('\n');
            }
        }

        // Render subscribers section
        let sub_entities = graph.get_entities_by_topic(EndpointKind::Subscription, topic);
        if !sub_entities.is_empty() {
            let is_focused = self.focus_pane == FocusPane::Detail;
            let is_selected = self.detail_state.selected_section == DetailSection::Subscribers;

            detail.push_str(&format!(
                "\n{}{} Subscribers ({}):",
                section_marker(is_focused, is_selected),
                expand_indicator(self.detail_state.subscribers_expanded),
                sub_entities.len()
            ));

            if self.detail_state.subscribers_expanded {
                detail.push_str("\n\n");
                for (idx, entity) in sub_entities.iter().enumerate() {
                    if let Some(endpoint) = entity_get_endpoint(entity) {
                        detail.push_str(&format!("   Subscriber {}:\n", idx + 1));
                        match endpoint.node.as_ref() {
                            Some(node) => {
                                detail.push_str(&format!(
                                    "    Node: {}/{}\n",
                                    node.namespace, node.name
                                ));
                            }
                            None => detail.push_str("    Node: unknown\n"),
                        }

                        if let Some(ti) = &endpoint.type_info {
                            let hash_str = ti.hash.to_rihs_string();
                            let hash_short = if hash_str.len() > HASH_TRUNCATE_LEN {
                                format!("{}...", &hash_str[..HASH_TRUNCATE_LEN])
                            } else {
                                hash_str
                            };
                            detail.push_str(&format!("    Type: {} ({})\n", ti.name, hash_short));
                        }

                        detail.push_str(&format_qos_detail(&endpoint.qos));
                        detail.push_str("\n\n");
                    }
                }
            } else {
                detail.push('\n');
            }
        }

        detail
    }
}
