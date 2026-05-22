//! Graph discovery: node/topic/service introspection from Python.

use hiroz::entity::EndpointKind;
use hiroz::graph::Graph;
use std::sync::Arc;

/// Python-accessible graph discovery methods.
///
/// These are exposed as methods on PyZNode rather than a separate class,
/// since they require access to the shared Graph instance.
pub(crate) struct GraphQueries;

impl GraphQueries {
    /// Get all topic names and their types.
    /// Returns list of (topic_name, type_name) tuples.
    pub fn get_topic_names_and_types(graph: &Arc<Graph>) -> Vec<(String, String)> {
        graph.get_topic_names_and_types()
    }

    /// Get all node names.
    /// Returns list of (name, namespace) tuples.
    pub fn get_node_names(graph: &Arc<Graph>) -> Vec<(String, String)> {
        graph.get_node_names()
    }

    /// Get all service names and their types.
    /// Returns list of (service_name, type_name) tuples.
    pub fn get_service_names_and_types(graph: &Arc<Graph>) -> Vec<(String, String)> {
        graph.get_service_names_and_types()
    }

    /// Count publishers for a topic.
    pub fn count_publishers(graph: &Arc<Graph>, topic: &str) -> usize {
        graph.count(EndpointKind::Publisher, topic)
    }

    /// Count subscribers for a topic.
    pub fn count_subscribers(graph: &Arc<Graph>, topic: &str) -> usize {
        graph.count(EndpointKind::Subscription, topic)
    }
}
