use anyhow::Result;
use clap::{Args, Subcommand};
use hiroz::attachment::Attachment;
use tokio::time::{Duration, sleep};

use crate::{context::Ctx, r#pub::yaml_to_cdr};

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand)]
pub enum ServiceAction {
    /// List all services
    List,
    /// Call a service
    Call {
        /// Service name (e.g. /add_two_ints)
        name: String,

        /// Request payload as space-separated hex bytes
        #[arg(long)]
        payload: Option<String>,

        /// Request payload from file
        #[arg(long)]
        file: Option<String>,

        /// Request as YAML (requires --msg-type)
        #[arg(long)]
        yaml: Option<String>,

        /// Message type for --yaml encoding (e.g. example_interfaces/srv/AddTwoInts_Request)
        #[arg(long)]
        msg_type: Option<String>,

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
            yaml,
            msg_type,
            timeout,
        } => {
            let ke = format!("{}/{}/**", ctx.domain, name.trim_start_matches('/'));

            let payload_bytes: Vec<u8> = if let Some(hex) = payload {
                parse_hex(&hex)?
            } else if let Some(path) = file {
                std::fs::read(&path)?
            } else if let Some(y) = yaml {
                let t = msg_type
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("--yaml requires --msg-type"))?;
                yaml_to_cdr(&y, t)?
            } else {
                vec![0u8; 4] // empty CDR
            };

            let timeout_dur = Duration::from_secs_f64(timeout);

            // hiroz service protocol requires an Attachment (sequence number + GID) on the query.
            let attachment = Attachment::new(1, [0u8; 16]);
            let attachment_bytes: zenoh::bytes::ZBytes = attachment.into();

            let replies = ctx
                .session
                .get(&ke)
                .payload(zenoh::bytes::ZBytes::from(payload_bytes))
                .attachment(attachment_bytes)
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
