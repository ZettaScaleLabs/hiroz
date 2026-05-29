//! Graph discovery: node/topic/service introspection from Python.

use hiroz::entity::EndpointKind;
use hiroz::graph::Graph;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Poll interval for discovery waits. Matches the ~50ms cadence rclpy uses
/// internally for its wait-for-service spin.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Block until at least one service server matching `service_name` is visible
/// in the graph, or `timeout` (seconds) elapses. `None` waits forever.
///
/// Must be called with the GIL released (`py.allow_threads`) so it does not
/// stall other Python threads while sleeping. Returns true if a server appeared.
pub(crate) fn wait_for_service_server(
    graph: &Arc<Graph>,
    service_name: &str,
    timeout: Option<f64>,
) -> bool {
    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));
    loop {
        if graph.count(EndpointKind::Service, service_name) > 0 {
            return true;
        }
        if let Some(d) = deadline
            && Instant::now() >= d
        {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

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
