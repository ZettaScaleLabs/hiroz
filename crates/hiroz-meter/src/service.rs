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
    /// Find services by name (substring match)
    Find {
        /// Name substring to search for
        name_filter: String,
    },
    /// Show the type of a service
    Type {
        /// Service name (e.g. /add_two_ints)
        name: String,
    },
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

        ServiceAction::Find { name_filter } => {
            sleep(Duration::from_millis(500)).await;
            let services = ctx.graph.get_service_names_and_types();
            let matched: Vec<_> = services
                .into_iter()
                .filter(|(name, _)| name.contains(&name_filter))
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

        ServiceAction::Type { name } => {
            sleep(Duration::from_millis(500)).await;
            let services = ctx.graph.get_service_names_and_types();
            let found = services
                .into_iter()
                .find(|(n, _)| n == &name || n == &format!("/{}", name.trim_start_matches('/')));
            match found {
                Some((_, typ)) => println!("{}", typ),
                None => anyhow::bail!("Service '{}' not found", name),
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

            // Derive response type from request type for pretty-printing.
            let resp_type = msg_type.as_deref().and_then(response_type_for_request);

            let mut got_reply = false;
            while let Ok(reply) = replies.recv_async().await {
                got_reply = true;
                match reply.result() {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes().into_owned();
                        if let Some(rt) = resp_type {
                            if let Ok(v) = cdr_to_json(&bytes, rt) {
                                if json {
                                    println!(
                                        "{}",
                                        serde_json::json!({
                                            "service": name,
                                            "response": v,
                                        })
                                    );
                                } else {
                                    println!("{}", serde_json::to_string_pretty(&v)?);
                                }
                                continue;
                            }
                        }
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

/// Map a request type name to its corresponding response type.
fn response_type_for_request(req_type: &str) -> Option<&'static str> {
    let short = req_type
        .rsplit_once('/')
        .map(|(_, s)| s)
        .unwrap_or(req_type);
    match short {
        "AddTwoInts_Request" => Some("AddTwoInts_Response"),
        "SetBool_Request" => Some("SetBool_Response"),
        "Trigger_Request" => Some("Trigger_Response"),
        "Empty" => Some("Empty"),
        _ => None,
    }
}

/// Decode a CDR response payload into a JSON value for known service response types.
fn cdr_to_json(payload: &[u8], resp_type: &str) -> Result<serde_json::Value> {
    // CDR header is 4 bytes: [0x00, 0x01, 0x00, 0x00] (little-endian)
    let data = payload
        .get(4..)
        .ok_or_else(|| anyhow::anyhow!("payload too short"))?;
    match resp_type {
        "AddTwoInts_Response" => {
            // int64 sum
            let sum = read_i64_le(data)?;
            Ok(serde_json::json!({ "sum": sum }))
        }
        "SetBool_Response" | "Trigger_Response" => {
            // bool success, string message
            let success = *data.first().ok_or_else(|| anyhow::anyhow!("short"))? != 0;
            // string: 4-byte length + utf8 bytes (no null terminator in CDR)
            let msg = if data.len() >= 5 {
                let len = u32::from_le_bytes(data[1..5].try_into().unwrap_or_default()) as usize;
                let end = 5 + len;
                if end <= data.len() {
                    // strip null terminator if present
                    let s = &data[5..end];
                    let s = s.strip_suffix(b"\0").unwrap_or(s);
                    String::from_utf8_lossy(s).into_owned()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            Ok(serde_json::json!({ "success": success, "message": msg }))
        }
        "Empty" => Ok(serde_json::json!({})),
        _ => anyhow::bail!("unknown response type"),
    }
}

fn read_i64_le(data: &[u8]) -> Result<i64> {
    data.get(..8)
        .and_then(|b| b.try_into().ok())
        .map(i64::from_le_bytes)
        .ok_or_else(|| anyhow::anyhow!("not enough bytes for i64"))
}
