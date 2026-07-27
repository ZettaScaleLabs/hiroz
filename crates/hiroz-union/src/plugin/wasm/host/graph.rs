//! graph::Host implementation — list_topics, list_nodes, list_services.

use hiroz_protocol::EndpointKind;

use super::super::state::PluginState;
use super::{hu, web_bindgen};

impl hu::plugin::graph::Host for PluginState {
    fn list_topics(&mut self) -> Vec<hu::plugin::graph::TopicInfo> {
        let graph = &self.engine.graph;
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
        let graph = &self.engine.graph;
        graph
            .get_node_names()
            .into_iter()
            .map(|(name, namespace)| hu::plugin::graph::NodeInfo { namespace, name })
            .collect()
    }

    fn list_services(&mut self) -> Vec<hu::plugin::graph::ServiceInfo> {
        let graph = &self.engine.graph;
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

    fn describe_node(
        &mut self,
        namespace: String,
        name: String,
    ) -> Option<hu::plugin::graph::NodeDetail> {
        let graph = &self.engine.graph;
        // Match against the same denormalized (namespace, name) pairs get_node_names()
        // produces — see get_node_names(): empty namespace becomes "/", otherwise a
        // leading '/' is prepended. Same lookup key shape as app/render/nodes.rs.
        let found = graph
            .get_node_names()
            .into_iter()
            .any(|(n, ns)| n == name && ns == namespace);
        if !found {
            return None;
        }
        // get_names_and_types_by_node uses the raw by_node index, which stores
        // the root namespace "/" as "" (see hiroz::entity::normalize_node_namespace),
        // whereas `namespace` here is the denormalized "/" form. Normalize back
        // or root-namespace lookups return empty pubs/subs.
        let node_key = (
            if namespace == "/" {
                String::new()
            } else {
                namespace
            },
            name,
        );
        let to_endpoints = |v: Vec<(String, String)>| {
            v.into_iter()
                .map(|(topic, type_name)| hu::plugin::graph::NodeEndpoint {
                    name: topic,
                    type_name,
                })
                .collect::<Vec<_>>()
        };
        let publishers = to_endpoints(
            graph.get_names_and_types_by_node(node_key.clone(), EndpointKind::Publisher),
        );
        let subscribers =
            to_endpoints(graph.get_names_and_types_by_node(node_key, EndpointKind::Subscription));
        Some(hu::plugin::graph::NodeDetail {
            publishers,
            subscribers,
        })
    }
}

// web-types has only records — generated Host trait is empty.
impl web_bindgen::hu::plugin::web_types::Host for PluginState {}
