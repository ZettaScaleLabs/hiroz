//! graph::Host implementation — list_topics, list_nodes, list_services.

use hiroz_protocol::EndpointKind;

use super::super::state::PluginState;
use super::{hu, web_bindgen};

impl hu::plugin::graph::Host for PluginState {
    fn list_topics(&mut self) -> Vec<hu::plugin::graph::TopicInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_topic_names_and_types()
            .into_iter()
            .map(|(name, type_name)| {
                let publishers = graph
                    .get_entities_by_topic(EndpointKind::Publisher, &name)
                    .len() as u32;
                let subscribers = graph
                    .get_entities_by_topic(EndpointKind::Subscription, &name)
                    .len() as u32;
                hu::plugin::graph::TopicInfo {
                    name,
                    type_name,
                    publishers,
                    subscribers,
                }
            })
            .collect()
    }

    fn list_nodes(&mut self) -> Vec<hu::plugin::graph::NodeInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_node_names()
            .into_iter()
            .map(|(name, namespace)| hu::plugin::graph::NodeInfo { namespace, name })
            .collect()
    }

    fn list_services(&mut self) -> Vec<hu::plugin::graph::ServiceInfo> {
        let graph = self.engine.graph.lock();
        graph
            .get_service_names_and_types()
            .into_iter()
            .map(|(name, type_name)| {
                let servers = graph.count_by_service(EndpointKind::Service, &name) as u32;
                hu::plugin::graph::ServiceInfo {
                    name,
                    type_name,
                    servers,
                }
            })
            .collect()
    }
}

// web-types has only records — generated Host trait is empty.
impl web_bindgen::hu::plugin::web_types::Host for PluginState {}
