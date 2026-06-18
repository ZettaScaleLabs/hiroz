use anyhow::Result;
use clap::Args;
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct GraphArgs {
    /// Refresh continuously (every N seconds)
    #[arg(long)]
    pub watch: Option<f64>,
}

pub async fn run(ctx: &Ctx, args: GraphArgs, json: bool) -> Result<()> {
    loop {
        sleep(Duration::from_millis(500)).await;

        let topics = ctx.graph.get_topic_names_and_types();
        let nodes = ctx.graph.get_node_names();
        let services = ctx.graph.get_service_names_and_types();

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "topics": topics.iter().map(|(n, t)| serde_json::json!({"name": n, "type": t})).collect::<Vec<_>>(),
                    "nodes": nodes.iter().map(|(ns, n)| serde_json::json!({"namespace": ns, "name": n})).collect::<Vec<_>>(),
                    "services": services.iter().map(|(n, t)| serde_json::json!({"name": n, "type": t})).collect::<Vec<_>>(),
                })
            );
        } else {
            println!(
                "--- Graph snapshot ({} topics, {} nodes, {} services) ---",
                topics.len(),
                nodes.len(),
                services.len()
            );
            println!("Nodes:");
            for (ns, name) in &nodes {
                let full = if ns == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", ns, name)
                };
                println!("  {}", full);
            }
            println!("Topics:");
            for (name, typ) in &topics {
                println!("  {}  [{}]", name, typ);
            }
            println!("Services:");
            for (name, typ) in &services {
                println!("  {}  [{}]", name, typ);
            }
        }

        match args.watch {
            Some(interval) => sleep(Duration::from_secs_f64(interval)).await,
            None => break,
        }
    }

    Ok(())
}
