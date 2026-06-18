use std::path::Path;

use hiroz::entity::{EndpointKind, entity_get_endpoint};

use crate::core::engine::CoreEngine;

pub async fn export_and_exit(
    core: &CoreEngine,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let extension = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    match extension {
        "json" => export_json(core, path).await?,
        "dot" => export_dot(core, path).await?,
        "csv" => export_csv(core, path).await?,
        _ => {
            eprintln!("Unsupported export format: {}", extension);
            eprintln!("Supported formats: .json, .dot, .csv");
            std::process::exit(1);
        }
    }

    tracing::info!("Exported to: {}", path);
    Ok(())
}

async fn export_json(
    core: &CoreEngine,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = {
        let graph = core.graph.lock();
        let snapshot = graph.snapshot(core.domain_id);
        serde_json::to_string_pretty(&snapshot)?
    };
    tokio::fs::write(path, json).await?;
    Ok(())
}

#[allow(clippy::await_holding_lock)]
async fn export_dot(
    core: &CoreEngine,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let graph = core.graph.lock();
    let mut dot = String::from("digraph ROS_Graph {\n");
    dot.push_str("  rankdir=LR;\n");
    dot.push_str("  node [shape=box];\n\n");

    // Nodes
    for (name, namespace) in graph.get_node_names() {
        let node_id = format!("{}/{}", namespace, name);
        dot.push_str(&format!(
            "  \"{}\" [label=\"{}\", fillcolor=lightblue, style=filled];\n",
            node_id, name
        ));
    }

    // Topics as ellipse nodes
    for (topic, type_name) in graph.get_topic_names_and_types() {
        dot.push_str(&format!(
            "  \"topic:{}\" [label=\"{}\\n{}\", shape=ellipse, fillcolor=lightgreen, style=filled];\n",
            topic, topic, type_name
        ));

        // Publishers -> Topic
        for entity in graph.get_entities_by_topic(EndpointKind::Publisher, &topic) {
            if let Some(endpoint) = entity_get_endpoint(&entity)
                && let Some(node) = endpoint.node.as_ref()
            {
                let node_id = format!("{}/{}", node.namespace, node.name);
                dot.push_str(&format!(
                    "  \"{}\" -> \"topic:{}\" [color=blue];\n",
                    node_id, topic
                ));
            }
        }

        // Topic -> Subscribers
        for entity in graph.get_entities_by_topic(EndpointKind::Subscription, &topic) {
            if let Some(endpoint) = entity_get_endpoint(&entity)
                && let Some(node) = endpoint.node.as_ref()
            {
                let node_id = format!("{}/{}", node.namespace, node.name);
                dot.push_str(&format!(
                    "  \"topic:{}\" -> \"{}\" [color=green];\n",
                    topic, node_id
                ));
            }
        }
    }

    dot.push_str("}\n");
    tokio::fs::write(path, dot).await?;
    Ok(())
}

#[allow(clippy::await_holding_lock)]
async fn export_csv(
    core: &CoreEngine,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let metrics = core.metrics.lock();
    let mut csv = String::from("timestamp,topic,rate_hz,bandwidth_kbps,avg_payload_bytes\n");

    for record in &metrics.history {
        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            record.timestamp,
            record.topic,
            record.rate_hz,
            record.bandwidth_kbps,
            record.avg_payload_bytes
        ));
    }

    tokio::fs::write(path, csv).await?;
    Ok(())
}
