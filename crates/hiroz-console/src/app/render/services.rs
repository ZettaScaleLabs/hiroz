//! Service panel rendering

use hiroz::entity::{EndpointKind, entity_get_endpoint};
use ratatui::{
    text::{Line, Span},
    widgets::ListItem,
};

use crate::app::App;
use crate::app::state::*;

use super::common::*;

impl App {
    /// Render service list items
    pub fn render_service_list_items(
        &self,
        filter_text: &str,
        list_width: usize,
    ) -> Vec<ListItem<'static>> {
        let all_services = &self.cached_services;

        let filtered: Vec<_> =
            self.filter_items(all_services, filter_text, |(service, type_name)| {
                vec![service.clone(), type_name.clone()]
            });

        filtered
            .iter()
            .enumerate()
            .map(|(i, (service, _type_name))| {
                let style = list_item_style(i == self.selected_index);

                // Calculate available space for service name
                let icon_width = 2; // "@ "
                let service_max_width = list_width.saturating_sub(icon_width);
                let display_service = truncate_with_ellipsis(service, service_max_width);

                // Build the line with highlighted service name
                let mut spans = vec![Span::styled("@ ".to_string(), style)];
                spans.extend(create_highlighted_spans(
                    &display_service,
                    filter_text,
                    style,
                ));

                ListItem::new(Line::from(spans))
            })
            .collect()
    }

    /// Render service detail panel content
    pub fn render_service_detail(&mut self) -> String {
        let graph = self.core.graph.lock();
        let filter_text = self.filter_input.to_lowercase();

        // Get filtered services
        let filtered_services: Vec<_> = graph
            .get_service_names_and_types()
            .iter()
            .filter(|(service, type_name)| {
                if filter_text.is_empty() {
                    true
                } else {
                    service.to_lowercase().contains(&filter_text)
                        || type_name.to_lowercase().contains(&filter_text)
                }
            })
            .map(|(s, tn)| (s.clone(), tn.clone()))
            .collect();

        let Some((service, type_name)) = filtered_services.get(self.selected_index) else {
            return "No service selected".to_string();
        };

        let server_count = graph
            .get_entities_by_service(EndpointKind::Service, service)
            .len();
        let client_count = graph
            .get_entities_by_service(EndpointKind::Client, service)
            .len();

        let mut detail = format!("Service: {}\nType: {}\n", service, type_name);

        let is_focused = self.focus_pane == FocusPane::Detail;

        // Servers section
        let server_selected = self.detail_state.selected_section == DetailSection::Publishers;
        detail.push_str(&format!(
            "\n{}{} Servers ({}):",
            section_marker(is_focused, server_selected),
            expand_indicator(self.detail_state.publishers_expanded),
            server_count
        ));

        if self.detail_state.publishers_expanded {
            let server_entities = graph.get_entities_by_service(EndpointKind::Service, service);
            if !server_entities.is_empty() {
                detail.push_str("\n\n");
                for (idx, entity) in server_entities.iter().enumerate() {
                    if let Some(endpoint) = entity_get_endpoint(entity) {
                        detail.push_str(&format!("   Server {}:\n", idx + 1));
                        match endpoint.node.as_ref() {
                            Some(node) => {
                                detail.push_str(&format!(
                                    "    Node: {}/{}\n",
                                    node.namespace, node.name
                                ));
                            }
                            None => detail.push_str("    Node: unknown\n"),
                        }
                        detail.push_str(&format_qos_detail(&endpoint.qos));
                        detail.push_str("\n\n");
                    }
                }
            } else {
                detail.push_str("\n    (none)\n");
            }
        } else {
            detail.push('\n');
        }

        // Clients section
        let client_selected = self.detail_state.selected_section == DetailSection::Clients;
        detail.push_str(&format!(
            "\n{}{} Clients ({}):",
            section_marker(is_focused, client_selected),
            expand_indicator(self.detail_state.clients_expanded),
            client_count
        ));

        if self.detail_state.clients_expanded {
            let client_entities = graph.get_entities_by_service(EndpointKind::Client, service);
            if !client_entities.is_empty() {
                detail.push_str("\n\n");
                for (idx, entity) in client_entities.iter().enumerate() {
                    if let Some(endpoint) = entity_get_endpoint(entity) {
                        detail.push_str(&format!("   Client {}:\n", idx + 1));
                        match endpoint.node.as_ref() {
                            Some(node) => {
                                detail.push_str(&format!(
                                    "    Node: {}/{}\n",
                                    node.namespace, node.name
                                ));
                            }
                            None => detail.push_str("    Node: unknown\n"),
                        }
                        detail.push_str(&format_qos_detail(&endpoint.qos));
                        detail.push_str("\n\n");
                    }
                }
            } else {
                detail.push_str("\n    (none)\n");
            }
        } else {
            detail.push('\n');
        }

        detail
    }
}
