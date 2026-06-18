use anyhow::Result;
use clap::{Args, Subcommand};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct ListArgs {
    #[command(subcommand)]
    pub what: ListWhat,
}

#[derive(Subcommand)]
pub enum ListWhat {
    /// List all topics
    Topics,
    /// List all nodes
    Nodes,
    /// List all services
    Services,
}

pub async fn run(ctx: &Ctx, args: ListArgs, json: bool) -> Result<()> {
    // Brief wait for liveliness to settle
    sleep(Duration::from_millis(500)).await;

    match args.what {
        ListWhat::Topics => {
            let topics = ctx.graph.get_topic_names_and_types();
            if json {
                let entries: Vec<_> = topics
                    .iter()
                    .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for (name, typ) in &topics {
                    println!("{}\t[{}]", name, typ);
                }
            }
        }
        ListWhat::Nodes => {
            let nodes = ctx.graph.get_node_names();
            if json {
                let entries: Vec<_> = nodes
                    .iter()
                    .map(|(ns, n)| serde_json::json!({"namespace": ns, "name": n}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for (ns, name) in &nodes {
                    let full = if ns == "/" {
                        format!("/{}", name)
                    } else {
                        format!("{}/{}", ns, name)
                    };
                    println!("{}", full);
                }
            }
        }
        ListWhat::Services => {
            let services = ctx.graph.get_service_names_and_types();
            if json {
                let entries: Vec<_> = services
                    .iter()
                    .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for (name, typ) in &services {
                    println!("{}\t[{}]", name, typ);
                }
            }
        }
    }

    Ok(())
}
