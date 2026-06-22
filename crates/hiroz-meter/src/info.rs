use anyhow::Result;
use clap::{Args, Subcommand};
use hiroz_protocol::EndpointKind;
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct InfoArgs {
    #[command(subcommand)]
    pub what: InfoWhat,
}

#[derive(Subcommand)]
pub enum InfoWhat {
    /// Show info for a topic
    Topic { name: String },
    /// Print the type of a topic
    TopicType { name: String },
    /// Show info for a node
    Node { name: String },
    /// Show info for a service
    Service { name: String },
}

pub async fn run(ctx: &Ctx, args: InfoArgs, json: bool) -> Result<()> {
    sleep(Duration::from_millis(500)).await;

    match args.what {
        InfoWhat::TopicType { name } => {
            let n = name.trim_start_matches('/');
            let typ = ctx
                .graph
                .get_topic_names_and_types()
                .into_iter()
                .find(|(t, _)| t.trim_start_matches('/') == n)
                .map(|(_, t)| t)
                .unwrap_or_default();
            if typ.is_empty() {
                anyhow::bail!("Unknown topic: {}", name);
            }
            if json {
                println!("{}", serde_json::json!({"topic": name, "type": typ}));
            } else {
                println!("{}", typ);
            }
        }

        InfoWhat::Topic { name } => {
            let n = name.trim_start_matches('/');
            // by_topic keys include the leading '/' so pass the original name
            let topic_key = format!("/{}", n);
            let pub_count = ctx.graph.count(EndpointKind::Publisher, &topic_key);
            let sub_count = ctx.graph.count(EndpointKind::Subscription, &topic_key);
            let typ = ctx
                .graph
                .get_topic_names_and_types()
                .into_iter()
                .find(|(t, _)| t.trim_start_matches('/') == n)
                .map(|(_, t)| t)
                .unwrap_or_default();

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "topic": name,
                        "type": typ,
                        "publisher_count": pub_count,
                        "subscriber_count": sub_count,
                    })
                );
            } else {
                println!("Topic: {}", name);
                println!("Type:  {}", typ);
                println!("Publishers:  {}", pub_count);
                println!("Subscribers: {}", sub_count);
            }
        }

        InfoWhat::Node { name } => {
            let n = name.trim_start_matches('/');
            // Parse ns/name — by_node uses empty string for root namespace
            let node_key: (String, String) = match n.rsplit_once('/') {
                Some((ns_part, nm_part)) => {
                    let ns = if ns_part.is_empty() { "" } else { ns_part };
                    (ns.to_string(), nm_part.to_string())
                }
                None => ("".to_string(), n.to_string()),
            };

            let found = ctx.graph.node_exists(node_key.clone());

            if !found {
                if json {
                    println!("{}", serde_json::json!({"node": name, "found": false}));
                } else {
                    println!("Node {} not found in graph", name);
                }
                return Ok(());
            }

            let publishers = ctx
                .graph
                .get_names_and_types_by_node(node_key.clone(), EndpointKind::Publisher);
            let subscribers = ctx
                .graph
                .get_names_and_types_by_node(node_key.clone(), EndpointKind::Subscription);
            let service_servers = ctx
                .graph
                .get_names_and_types_by_node(node_key.clone(), EndpointKind::Service);
            let service_clients = ctx
                .graph
                .get_names_and_types_by_node(node_key.clone(), EndpointKind::Client);
            let action_servers = ctx
                .graph
                .get_action_server_names_and_types_by_node(node_key.clone());
            let action_clients = ctx
                .graph
                .get_action_client_names_and_types_by_node(node_key.clone());

            if json {
                let to_entries = |v: &[(String, String)]| -> Vec<serde_json::Value> {
                    v.iter()
                        .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
                        .collect()
                };
                println!(
                    "{}",
                    serde_json::json!({
                        "node": name,
                        "found": true,
                        "publishers": to_entries(&publishers),
                        "subscribers": to_entries(&subscribers),
                        "service_servers": to_entries(&service_servers),
                        "service_clients": to_entries(&service_clients),
                        "action_servers": to_entries(&action_servers),
                        "action_clients": to_entries(&action_clients),
                    })
                );
            } else {
                println!("Node: {}", name);
                let print_section = |label: &str, items: &[(String, String)]| {
                    println!("  {}:", label);
                    if items.is_empty() {
                        println!("    (none)");
                    } else {
                        for (n, t) in items {
                            println!("    {} [{}]", n, t);
                        }
                    }
                };
                print_section("Publishers", &publishers);
                print_section("Subscribers", &subscribers);
                print_section("Service Servers", &service_servers);
                print_section("Service Clients", &service_clients);
                print_section("Action Servers", &action_servers);
                print_section("Action Clients", &action_clients);
            }
        }

        InfoWhat::Service { name } => {
            let n = name.trim_start_matches('/');
            let server_count = ctx.graph.count_by_service(EndpointKind::Service, n);
            let client_count = ctx.graph.count_by_service(EndpointKind::Client, n);
            let typ = ctx
                .graph
                .get_service_names_and_types()
                .into_iter()
                .find(|(s, _)| s.trim_start_matches('/') == n)
                .map(|(_, t)| t)
                .unwrap_or_default();

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "service": name,
                        "type": typ,
                        "servers": server_count,
                        "clients": client_count,
                    })
                );
            } else {
                println!("Service: {}", name);
                println!("Type:    {}", typ);
                println!("Servers: {}", server_count);
                println!("Clients: {}", client_count);
            }
        }
    }

    Ok(())
}
