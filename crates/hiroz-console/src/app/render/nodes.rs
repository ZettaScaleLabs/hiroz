//! Node panel rendering

use hiroz::entity::EndpointKind;
use ratatui::{
    text::{Line, Span},
    widgets::ListItem,
};

use crate::app::App;

use super::common::*;

impl App {
    /// Render node list items
    pub fn render_node_list_items(
        &self,
        filter_text: &str,
        list_width: usize,
    ) -> Vec<ListItem<'static>> {
        let all_nodes = &self.cached_nodes;

        let filtered: Vec<_> = self.filter_items(all_nodes, filter_text, |(name, namespace)| {
            vec![name.clone(), namespace.clone()]
        });

        filtered
            .iter()
            .enumerate()
            .map(|(i, (name, namespace))| {
                let style = list_item_style(i == self.selected_index);

                // Format: "* <namespace>/<name>"
                let full_name = format!("{}/{}", namespace, name);
                let icon_width = 2; // "* "
                let node_max_width = list_width.saturating_sub(icon_width);
                let display_node = truncate_with_ellipsis(&full_name, node_max_width);

                // Build the line with highlighted node name
                let mut spans = vec![Span::styled("* ".to_string(), style)];
                spans.extend(create_highlighted_spans(&display_node, filter_text, style));

                ListItem::new(Line::from(spans))
            })
            .collect()
    }

    /// Render node detail panel content
    pub fn render_node_detail(&self) -> String {
        let graph = self.core.graph.lock();
        let filter_text = self.filter_input.to_lowercase();

        // Get filtered nodes
        let filtered_nodes: Vec<_> = graph
            .get_node_names()
            .iter()
            .filter(|(name, namespace)| {
                if filter_text.is_empty() {
                    true
                } else {
                    name.to_lowercase().contains(&filter_text)
                        || namespace.to_lowercase().contains(&filter_text)
                }
            })
            .map(|(n, ns)| (n.clone(), ns.clone()))
            .collect();

        let Some((name, namespace)) = filtered_nodes.get(self.selected_index) else {
            return "No node selected".to_string();
        };

        let node_key = (namespace.clone(), name.clone());
        let publishers =
            graph.get_names_and_types_by_node(node_key.clone(), EndpointKind::Publisher);
        let subscribers =
            graph.get_names_and_types_by_node(node_key.clone(), EndpointKind::Subscription);
        let services = graph.get_names_and_types_by_node(node_key.clone(), EndpointKind::Service);
        let clients = graph.get_names_and_types_by_node(node_key, EndpointKind::Client);

        let mut detail = format!("Node: {}/{}\n", namespace, name);

        let show_types = self.detail_state.publishers_expanded;
        let expand_hint = if show_types { "[-]" } else { "[+]" };

        if !publishers.is_empty() {
            detail.push_str(&format!(
                "\n{} Publishers ({}):\n",
                expand_hint,
                publishers.len()
            ));
            for (topic, type_name) in &publishers {
                if show_types {
                    detail.push_str(&format!("  +-- {} ({})\n", topic, type_name));
                } else {
                    detail.push_str(&format!("  +-- {}\n", topic));
                }
            }
        }

        if !subscribers.is_empty() {
            detail.push_str(&format!(
                "\n{} Subscribers ({}):\n",
                expand_hint,
                subscribers.len()
            ));
            for (topic, type_name) in &subscribers {
                if show_types {
                    detail.push_str(&format!("  +-- {} ({})\n", topic, type_name));
                } else {
                    detail.push_str(&format!("  +-- {}\n", topic));
                }
            }
        }

        if !services.is_empty() {
            detail.push_str(&format!(
                "\n{} Services ({}):\n",
                expand_hint,
                services.len()
            ));
            for (service, type_name) in &services {
                if show_types {
                    detail.push_str(&format!("  +-- {} ({})\n", service, type_name));
                } else {
                    detail.push_str(&format!("  +-- {}\n", service));
                }
            }
        }

        if !clients.is_empty() {
            detail.push_str(&format!("\n{} Clients ({}):\n", expand_hint, clients.len()));
            for (service, type_name) in &clients {
                if show_types {
                    detail.push_str(&format!("  +-- {} ({})\n", service, type_name));
                } else {
                    detail.push_str(&format!("  +-- {}\n", service));
                }
            }
        }

        if publishers.is_empty()
            && subscribers.is_empty()
            && services.is_empty()
            && clients.is_empty()
        {
            detail.push_str("\n(No topics or services)");
        }

        detail
    }
}
