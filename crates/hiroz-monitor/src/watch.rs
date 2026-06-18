use anyhow::Result;
use clap::Args;
use hiroz_protocol::Entity;
use zenoh::sample::SampleKind;

use crate::context::Ctx;

#[derive(Args)]
pub struct WatchArgs {
    /// Filter by prefix
    #[arg(long)]
    pub filter: Option<String>,
}

pub async fn run(ctx: &Ctx, _args: WatchArgs, json: bool) -> Result<()> {
    let ke = format!("@ros2_lv/{}/**", ctx.domain);
    let fmt = hiroz_protocol::KeyExprFormat::RmwZenoh;

    let sub = ctx
        .session
        .liveliness()
        .declare_subscriber(&ke)
        .history(true)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !json {
        eprintln!("Watching graph events (Ctrl+C to stop)...");
    }

    while let Ok(sample) = sub.recv_async().await {
        let ke_expr = sample.key_expr();
        let event = match sample.kind() {
            SampleKind::Put => "appeared",
            SampleKind::Delete => "disappeared",
        };

        let entity = fmt.parse_liveliness(ke_expr).ok();

        if json {
            let desc = entity
                .as_ref()
                .map(entity_to_json)
                .unwrap_or_else(|| serde_json::json!({"raw": ke_expr.to_string()}));
            println!(
                "{}",
                serde_json::json!({
                    "event": event,
                    "entity": desc,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
            );
        } else {
            let ts = chrono::Local::now().format("%H:%M:%S%.3f");
            let summary = entity
                .as_ref()
                .map(entity_summary)
                .unwrap_or_else(|| ke_expr.to_string());
            println!("[{}] {} {}", ts, event, summary);
        }
    }

    Ok(())
}

fn entity_to_json(entity: &Entity) -> serde_json::Value {
    match entity {
        Entity::Node(n) => serde_json::json!({
            "kind": "node",
            "namespace": n.namespace,
            "name": n.name,
        }),
        Entity::Endpoint(ep) => {
            let type_name = ep.type_info.as_ref().map(|t| t.name.as_str()).unwrap_or("");
            serde_json::json!({
                "kind": format!("{}", ep.kind),
                "topic": ep.topic,
                "type": type_name,
            })
        }
    }
}

fn entity_summary(entity: &Entity) -> String {
    match entity {
        Entity::Node(n) => format!("node {}/{}", n.namespace, n.name),
        Entity::Endpoint(ep) => {
            let type_name = ep
                .type_info
                .as_ref()
                .map(|t| t.name.as_str())
                .unwrap_or("?");
            format!("{} {} [{}]", ep.kind, ep.topic, type_name)
        }
    }
}
