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
    /// Show info for a node
    Node { name: String },
    /// Show info for a service
    Service { name: String },
}

pub async fn run(ctx: &Ctx, args: InfoArgs, json: bool) -> Result<()> {
    sleep(Duration::from_millis(500)).await;

    match args.what {
        InfoWhat::Topic { name } => {
            let n = name.trim_start_matches('/');
            let pub_count = ctx.graph.count(EndpointKind::Publisher, n);
            let sub_count = ctx.graph.count(EndpointKind::Subscription, n);
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
            let nodes = ctx.graph.get_node_names();
            let found = nodes.iter().any(|(ns, nm)| {
                let full = if ns == "/" {
                    format!("/{}", nm)
                } else {
                    format!("{}/{}", ns, nm)
                };
                full.trim_start_matches('/') == n
            });

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "node": name,
                        "found": found,
                    })
                );
            } else if found {
                println!("Node: {}", name);
            } else {
                println!("Node {} not found in graph", name);
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
