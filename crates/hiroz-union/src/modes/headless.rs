use chrono::Utc;
use hiroz::dynamic::{DynSub, MessageSchemaTypeDescription};
use serde::Serialize;
use std::{collections::HashMap, time::Duration};

use crate::core::{
    engine::CoreEngine,
    message_formatter::{dynamic_message_to_json, format_message_pretty},
};
use hiroz::graph::GraphSnapshot;

/// Wrapper for initial state that adds the event field
#[derive(Serialize)]
struct InitialStateEvent {
    event: &'static str,
    #[serde(flatten)]
    snapshot: GraphSnapshot,
}

pub async fn run_headless_mode(
    core: &CoreEngine,
    json: bool,
    echo_topics: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("Starting headless mode - streaming events to stdout");

    let mut event_rx = core.subscribe_events();

    // Initial state dump
    if json {
        let graph = core.graph.lock();
        let snapshot = graph.snapshot(core.domain_id);
        let event = InitialStateEvent {
            event: "initial_state",
            snapshot,
        };
        println!("{}", serde_json::to_string(&event)?);
    } else {
        print_initial_state(core);
    }

    // Create dynamic subscribers for echo topics
    let mut subscribers: HashMap<String, DynSub> = HashMap::new();

    if !echo_topics.is_empty() {
        tracing::info!(
            "Creating dynamic subscribers for {} topics",
            echo_topics.len()
        );

        for topic in echo_topics {
            match core
                .create_dynamic_subscriber(&topic, Duration::from_secs(5))
                .await
            {
                Ok(sub) => {
                    let Some(schema) = sub.schema() else {
                        eprintln!(
                            "Failed to inspect schema for {}: missing dynamic schema",
                            topic
                        );
                        continue;
                    };
                    let type_hash = schema
                        .compute_type_hash()
                        .map(|hash| hash.to_rihs_string())
                        .unwrap_or_else(|_| "unknown".to_string());
                    if json {
                        // Output schema info
                        let schema_info = serde_json::json!({
                            "event": "topic_subscribed",
                            "topic": topic,
                            "type_name": schema.type_name,
                            "type_hash": type_hash,
                            "fields": schema.field_names().collect::<Vec<_>>(),
                        });
                        println!("{}", serde_json::to_string(&schema_info)?);
                    } else {
                        println!("\n=== Subscribed to {} ===", topic);
                        println!("Type: {}", schema.type_name);
                        println!("Hash: {}", type_hash);
                        println!("Fields: {:?}", schema.field_names().collect::<Vec<_>>());
                        println!();
                    }
                    subscribers.insert(topic.clone(), sub);
                }
                Err(e) => {
                    eprintln!("Failed to subscribe to {}: {}", topic, e);
                }
            }
        }
    }

    // Event loop
    if subscribers.is_empty() {
        // No echo topics - original behavior
        while let Ok(event) = event_rx.recv().await {
            if json {
                println!("{}", event.to_json());
            } else {
                println!(
                    "[{}] {}",
                    Utc::now().format("%Y-%m-%d %H:%M:%S"),
                    event.to_human_readable()
                );
            }
        }
    } else {
        // With echo topics - interleave events and messages
        loop {
            tokio::select! {
                // Handle system events
                Ok(event) = event_rx.recv() => {
                    if json {
                        println!("{}", event.to_json());
                    } else {
                        println!(
                            "[{}] {}",
                            Utc::now().format("%Y-%m-%d %H:%M:%S"),
                            event.to_human_readable()
                        );
                    }
                }

                // Handle echo messages
                _ = async {
                    for (topic, subscriber) in &subscribers {
                        if let Some(result) = subscriber.try_recv() {
                            match result {
                                Ok(msg) => {
                                    if json {
                                        let msg_json = serde_json::json!({
                                            "event": "message_received",
                                            "topic": topic,
                                            "type": msg.schema().type_name,
                                            "data": dynamic_message_to_json(&msg),
                                        });
                                        if let Ok(json_str) = serde_json::to_string(&msg_json) {
                                            println!("{}", json_str);
                                        }
                                    } else {
                                        println!("\n=== {} ===", topic);
                                        print!("{}", format_message_pretty(&msg));
                                    }
                                }
                                Err(error) => {
                                    eprintln!("Failed to receive from {}: {}", topic, error);
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                } => {}
            }
        }
    }

    Ok(())
}

fn print_initial_state(core: &CoreEngine) {
    let graph = core.graph.lock();
    let topics = graph.get_topic_names_and_types();
    let services = graph.get_service_names_and_types();
    let nodes = graph.get_node_names();

    println!("Discovered Topics:");
    for (topic, type_name) in &topics {
        println!("  topic: {} ({})", topic, type_name);
    }
    if topics.is_empty() {
        println!("  (none)");
    }

    println!("Discovered Services:");
    for (service, type_name) in &services {
        println!("  service: {} ({})", service, type_name);
    }
    if services.is_empty() {
        println!("  (none)");
    }

    println!("Discovered Nodes:");
    for (name, namespace) in &nodes {
        println!("  node: {}/{}", namespace, name);
    }
    if nodes.is_empty() {
        println!("  (none)");
    }
}
