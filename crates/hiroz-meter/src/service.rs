use anyhow::Result;
use clap::{Args, Subcommand};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand)]
pub enum ServiceAction {
    /// List all services
    List,
    /// Call a service with raw CDR payload
    Call {
        /// Service name (e.g. /add_two_ints)
        name: String,

        /// Request payload as space-separated hex bytes
        #[arg(long)]
        payload: Option<String>,

        /// Request payload from file
        #[arg(long)]
        file: Option<String>,

        /// Timeout in seconds
        #[arg(long, default_value = "5.0")]
        timeout: f64,
    },
}

pub async fn run(ctx: &Ctx, args: ServiceArgs, json: bool) -> Result<()> {
    match args.action {
        ServiceAction::List => {
            sleep(Duration::from_millis(500)).await;
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

        ServiceAction::Call {
            name,
            payload,
            file,
            timeout,
        } => {
            let ke = format!("{}/{}/**", ctx.domain, name.trim_start_matches('/'));

            let payload_bytes: Vec<u8> = if let Some(hex) = payload {
                parse_hex(&hex)?
            } else if let Some(path) = file {
                std::fs::read(&path)?
            } else {
                vec![0u8; 4] // empty CDR
            };

            let timeout_dur = Duration::from_secs_f64(timeout);

            let replies = ctx
                .session
                .get(&ke)
                .payload(zenoh::bytes::ZBytes::from(payload_bytes))
                .timeout(timeout_dur)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

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
                                    "service": name,
                                    "bytes": bytes.len(),
                                    "response": hex,
                                })
                            );
                        } else {
                            println!("response: {} bytes  {}", bytes.len(), hex);
                        }
                    }
                    Err(e) => {
                        anyhow::bail!("Service returned error: {e}");
                    }
                }
            }

            if !got_reply {
                anyhow::bail!("No response received from {}", name);
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
