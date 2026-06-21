use hiroz::graph::Graph;
use hiroz_protocol::{EndpointKind, qos::QosReliability};
use std::sync::Arc;
use tokio::time::{Duration, sleep};

/// Waits briefly for graph discovery, then warns if any publisher on `topic`
/// uses BEST_EFFORT reliability while hu-meter subscribes with RELIABLE.
pub async fn warn_if_qos_mismatch(graph: Arc<Graph>, topic: String) {
    sleep(Duration::from_millis(500)).await;

    let topic_key = format!("/{}", topic.trim_start_matches('/'));
    let publishers = graph.get_entities_by_topic(EndpointKind::Publisher, &topic_key);

    for pub_arc in publishers {
        if let Some(ep) = hiroz::entity::entity_get_endpoint(&pub_arc) {
            if ep.qos.reliability == QosReliability::BestEffort {
                eprintln!(
                    "[warn] Publisher on {} uses BEST_EFFORT reliability — \
                     hu-meter subscribes with RELIABLE and may miss messages \
                     if your rmw enforces QoS compatibility (ros2cli#593)",
                    topic
                );
                return;
            }
        }
    }
}
