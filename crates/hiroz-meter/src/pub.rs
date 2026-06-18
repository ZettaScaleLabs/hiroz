use anyhow::Result;
use clap::Args;
use tokio::time::{Duration, interval};
use zenoh::bytes::ZBytes;

use crate::context::Ctx;

#[derive(Args)]
pub struct PubArgs {
    /// Topic name (e.g. /chatter)
    pub topic: String,

    /// Message type (e.g. std_msgs/msg/String) — informational, not used for encoding
    #[arg(long)]
    pub msg_type: Option<String>,

    /// Payload as space-separated hex bytes (CDR-encoded)
    #[arg(long, conflicts_with = "file")]
    pub payload: Option<String>,

    /// Payload from file (raw CDR bytes)
    #[arg(long, conflicts_with = "payload")]
    pub file: Option<String>,

    /// Convenience: publish a plain string wrapped in CDR std_msgs/String encoding
    #[arg(long, conflicts_with_all = &["payload", "file"])]
    pub string: Option<String>,

    /// Publish rate in Hz (0 = publish once and exit)
    #[arg(long, default_value = "1.0")]
    pub rate: f64,

    /// Number of messages to publish (0 = unlimited, requires --rate > 0)
    #[arg(long, default_value = "1")]
    pub count: usize,
}

pub async fn run(ctx: &Ctx, args: PubArgs, json: bool) -> Result<()> {
    let topic = args.topic.trim_start_matches('/').to_string();
    // Key expression for publishing: domain/topic/<type>/<hash>
    // Use a wildcard-free key for direct publish — other hiroz nodes subscribe
    // with a wildcard suffix so they will receive it.
    let ke = format!("{}/{}", ctx.domain, topic);

    let payload_bytes = build_payload(&args)?;

    let publisher = ctx
        .session
        .declare_publisher(&ke)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let count = args.count;
    let rate = args.rate;

    if rate <= 0.0 || count == 1 {
        // Publish once
        publisher
            .put(ZBytes::from(payload_bytes.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "topic": args.topic,
                    "bytes": payload_bytes.len(),
                    "published": 1,
                })
            );
        } else {
            println!(
                "Published 1 message ({} bytes) to {}",
                payload_bytes.len(),
                args.topic
            );
        }
        return Ok(());
    }

    let period = Duration::from_secs_f64(1.0 / rate);
    let mut ticker = interval(period);
    let mut published = 0usize;

    loop {
        ticker.tick().await;
        publisher
            .put(ZBytes::from(payload_bytes.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        published += 1;

        if !json {
            println!(
                "[{}] Published {} bytes to {}",
                published,
                payload_bytes.len(),
                args.topic
            );
        }

        if count > 0 && published >= count {
            break;
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "topic": args.topic,
                "bytes": payload_bytes.len(),
                "published": published,
            })
        );
    }

    Ok(())
}

fn build_payload(args: &PubArgs) -> Result<Vec<u8>> {
    if let Some(hex) = &args.payload {
        return parse_hex(hex);
    }
    if let Some(path) = &args.file {
        return Ok(std::fs::read(path)?);
    }
    if let Some(s) = &args.string {
        return Ok(encode_cdr_string(s));
    }
    // Default: empty CDR (4-byte header only)
    Ok(vec![0u8; 4])
}

/// Encode a plain string as CDR std_msgs/String:
/// 4-byte CDR header (little-endian, no padding) + 4-byte string length + UTF-8 bytes + null terminator
fn encode_cdr_string(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let len = bytes.len() + 1; // +1 for null terminator
    let mut buf = Vec::with_capacity(4 + 4 + len);
    // CDR header: 0x00 0x01 (little-endian) + 2 padding bytes
    buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
    // String length (including null)
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    // String data
    buf.extend_from_slice(bytes);
    buf.push(0x00); // null terminator
    buf
}

fn parse_hex(s: &str) -> Result<Vec<u8>> {
    s.split_whitespace()
        .map(|tok| {
            u8::from_str_radix(tok, 16).map_err(|e| anyhow::anyhow!("Invalid hex '{tok}': {e}"))
        })
        .collect()
}
