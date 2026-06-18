use anyhow::Result;
use clap::Args;

use crate::context::Ctx;

#[derive(Args)]
pub struct EchoArgs {
    /// Topic name
    pub topic: String,

    /// Number of messages to print (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub count: usize,

    /// Print raw bytes as hex instead of trying to decode
    #[arg(long)]
    pub raw: bool,
}

pub async fn run(ctx: &Ctx, args: EchoArgs, json: bool) -> Result<()> {
    let topic = args.topic.trim_start_matches('/').to_string();
    let ke = format!("{}/{}/**", ctx.domain, topic);
    let count = args.count;
    let raw = args.raw;

    let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received_cb = received.clone();
    let topic_name = args.topic.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    let _sub = ctx
        .session
        .declare_subscriber(&ke)
        .callback(move |sample| {
            let n = received_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if count == 0 || n <= count {
                let payload = sample.payload().to_bytes().into_owned();
                let _ = tx.try_send(payload);
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut printed = 0usize;
    while let Some(payload) = rx.recv().await {
        printed += 1;

        if json {
            let hex = if raw || payload.len() > 512 {
                hex_encode(&payload)
            } else {
                // Try to decode as CDR string after 4-byte header
                decode_cdr_preview(&payload).unwrap_or_else(|| hex_encode(&payload))
            };
            println!(
                "{}",
                serde_json::json!({
                    "topic": topic_name,
                    "seq": printed,
                    "bytes": payload.len(),
                    "data": hex,
                })
            );
        } else if raw {
            println!(
                "[{}] {} bytes: {}",
                printed,
                payload.len(),
                hex_encode(&payload)
            );
        } else {
            println!(
                "[{}] {} bytes  {}",
                printed,
                payload.len(),
                decode_cdr_preview(&payload).unwrap_or_else(|| "(binary)".into())
            );
        }

        if count > 0 && printed >= count {
            break;
        }
    }

    Ok(())
}

fn hex_encode(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{:02x}", x))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Try to show a short human-readable preview of a CDR payload.
/// CDR header is 4 bytes; after that, common types:
/// - string: 4-byte length LE + UTF-8 bytes
fn decode_cdr_preview(payload: &[u8]) -> Option<String> {
    if payload.len() < 8 {
        return None;
    }
    // Skip 4-byte CDR header
    let data = &payload[4..];
    // Try string: 4-byte length then UTF-8
    if data.len() >= 4 {
        let len = u32::from_le_bytes(data[..4].try_into().ok()?) as usize;
        if len > 0
            && len <= 1024
            && data.len() >= 4 + len
            && let Ok(s) = std::str::from_utf8(&data[4..4 + len])
        {
            let s = s.trim_end_matches('\0');
            if s.chars().all(|c| !c.is_control() || c == '\n') {
                return Some(format!("\"{}\"", s));
            }
        }
    }
    None
}
