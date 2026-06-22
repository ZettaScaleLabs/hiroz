use anyhow::Result;
use clap::{Args, Subcommand};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct ActionArgs {
    #[command(subcommand)]
    pub action: ActionCmd,
}

#[derive(Subcommand)]
pub enum ActionCmd {
    /// List all action servers
    List,
    /// Show info about an action server
    Info {
        /// Action name (e.g. /fibonacci)
        name: String,
    },
    /// Print the type of an action server
    Type {
        /// Action name (e.g. /fibonacci)
        name: String,
    },
    /// Find action servers by type (substring match)
    Find {
        /// Type filter (e.g. example_interfaces/action/Fibonacci)
        type_filter: String,
    },
    /// Send a goal to an action server (raw hex CDR)
    SendGoal {
        /// Action name (e.g. /fibonacci)
        name: String,

        /// Goal payload as space-separated hex bytes (CDR-encoded)
        #[arg(long)]
        payload: Option<String>,

        /// Goal payload from file
        #[arg(long)]
        file: Option<String>,

        /// Timeout waiting for result (seconds)
        #[arg(long, default_value = "30.0")]
        timeout: f64,
    },
}

pub async fn run(ctx: &Ctx, args: ActionArgs, json: bool) -> Result<()> {
    match args.action {
        ActionCmd::List => {
            sleep(Duration::from_millis(500)).await;
            let actions = ctx.graph.get_action_names_and_types();
            if json {
                let entries: Vec<_> = actions
                    .iter()
                    .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for (name, typ) in &actions {
                    println!("{}\t[{}]", name, typ);
                }
            }
        }

        ActionCmd::Info { name } => {
            sleep(Duration::from_millis(500)).await;
            let all = ctx.graph.get_action_names_and_types();
            let typ = all
                .iter()
                .find(|(n, _)| {
                    n == &name || n.trim_start_matches('/') == name.trim_start_matches('/')
                })
                .map(|(_, t)| t.clone())
                .unwrap_or_default();

            // Count action servers (nodes that publish _action/feedback for this action)
            let feedback_topic = format!("{}/_action/feedback", name.trim_start_matches('/'));
            use hiroz_protocol::EndpointKind;
            let server_count = ctx.graph.count(EndpointKind::Publisher, &feedback_topic);
            let client_count = ctx.graph.count(EndpointKind::Subscription, &feedback_topic);

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": name,
                        "type": typ,
                        "servers": server_count,
                        "clients": client_count,
                    })
                );
            } else {
                println!("Action: {}", name);
                println!("Type:    {}", typ);
                println!("Servers: {}", server_count);
                println!("Clients: {}", client_count);
            }
        }

        ActionCmd::Type { name } => {
            sleep(Duration::from_millis(500)).await;
            let all = ctx.graph.get_action_names_and_types();
            let typ = all
                .iter()
                .find(|(n, _)| {
                    n == &name || n.trim_start_matches('/') == name.trim_start_matches('/')
                })
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            if typ.is_empty() {
                anyhow::bail!("Unknown action: {}", name);
            }
            if json {
                println!("{}", serde_json::json!({"action": name, "type": typ}));
            } else {
                println!("{}", typ);
            }
        }

        ActionCmd::Find { type_filter } => {
            sleep(Duration::from_millis(500)).await;
            let all = ctx.graph.get_action_names_and_types();
            let matched: Vec<_> = all
                .into_iter()
                .filter(|(_, t)| t.contains(&type_filter))
                .collect();
            if json {
                let entries: Vec<_> = matched
                    .iter()
                    .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for (name, _) in &matched {
                    println!("{}", name);
                }
            }
        }

        ActionCmd::SendGoal {
            name,
            payload,
            file,
            timeout,
        } => {
            // Action goals are sent via the _action/send_goal service.
            // We use the same raw Zenoh query approach as service call.
            let goal_service = format!(
                "{}/{}/{name}/_action/send_goal/**",
                ctx.domain,
                name.trim_start_matches('/')
            );

            let payload_bytes: Vec<u8> = if let Some(hex) = payload {
                parse_hex(&hex)?
            } else if let Some(path) = file {
                std::fs::read(&path)?
            } else {
                vec![0u8; 4]
            };

            let timeout_dur = Duration::from_secs_f64(timeout);
            let replies = tokio::time::timeout(timeout_dur, async {
                ctx.session
                    .get(&goal_service)
                    .payload(zenoh::bytes::ZBytes::from(payload_bytes))
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))
            })
            .await
            .map_err(|_| anyhow::anyhow!("send_goal timed out after {timeout}s"))??;

            let mut got_reply = false;
            while let Ok(reply) = replies.recv_async().await {
                got_reply = true;
                match reply.result() {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes().into_owned();
                        let hex = bytes
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "action": name,
                                    "bytes": bytes.len(),
                                    "response": hex,
                                })
                            );
                        } else {
                            println!("send_goal response: {} bytes  {}", bytes.len(), hex);
                        }
                    }
                    Err(e) => anyhow::bail!("Action returned error: {e}"),
                }
            }

            if !got_reply {
                anyhow::bail!("No response from action server {}", name);
            }
        }
    }

    Ok(())
}

fn parse_hex(s: &str) -> Result<Vec<u8>> {
    s.split_whitespace()
        .map(|tok| {
            u8::from_str_radix(tok, 16).map_err(|e| anyhow::anyhow!("Invalid hex '{tok}': {e}"))
        })
        .collect()
}
