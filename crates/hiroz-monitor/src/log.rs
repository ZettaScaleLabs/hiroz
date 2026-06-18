use anyhow::Result;
use clap::Args;

use crate::context::Ctx;

#[derive(Args)]
pub struct LogArgs {
    /// Minimum log level to display (DEBUG, INFO, WARN, ERROR, FATAL)
    #[arg(long, default_value = "INFO")]
    pub level: String,

    /// Filter by node name substring
    #[arg(long)]
    pub node: Option<String>,

    /// Number of messages (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub count: usize,
}

/// rcl_interfaces/msg/Log CDR layout (ROS 2):
/// CDR header:    4 bytes (0x00 0x01 0x00 0x00 for LE)
/// stamp.sec:     4 bytes u32 LE
/// stamp.nanosec: 4 bytes u32 LE
/// level:         1 byte u8
/// name:          4 bytes u32 LE (len) + bytes + null
/// msg:           4 bytes u32 LE (len) + bytes + null
/// file:          4 bytes u32 LE (len) + bytes + null
/// function:      4 bytes u32 LE (len) + bytes + null
/// line:          4 bytes u32 LE
fn decode_log(payload: &[u8]) -> Option<LogMsg> {
    if payload.len() < 14 {
        return None;
    }

    let mut pos = 4; // skip CDR header

    let sec = u32::from_le_bytes(payload[pos..pos + 4].try_into().ok()?);
    pos += 4;
    let nanosec = u32::from_le_bytes(payload[pos..pos + 4].try_into().ok()?);
    pos += 4;

    let level = *payload.get(pos)?;
    pos += 1;

    // align to 4
    pos = (pos + 3) & !3;

    let (name, next) = read_cdr_string(payload, pos)?;
    pos = next;
    let (msg, next) = read_cdr_string(payload, pos)?;
    pos = next;
    let (file, next) = read_cdr_string(payload, pos)?;
    pos = next;
    let (function, _next) = read_cdr_string(payload, pos)?;

    Some(LogMsg {
        sec,
        nanosec,
        level,
        name,
        msg,
        file,
        function,
    })
}

fn read_cdr_string(buf: &[u8], pos: usize) -> Option<(String, usize)> {
    if buf.len() < pos + 4 {
        return None;
    }
    let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?) as usize;
    let start = pos + 4;
    let end = start + len;
    if buf.len() < end {
        return None;
    }
    let s = std::str::from_utf8(&buf[start..end])
        .unwrap_or("")
        .trim_end_matches('\0')
        .to_string();
    // align next read to 4 bytes
    let next = (end + 3) & !3;
    Some((s, next))
}

#[derive(Debug)]
struct LogMsg {
    sec: u32,
    nanosec: u32,
    level: u8,
    name: String,
    msg: String,
    file: String,
    function: String,
}

fn level_name(level: u8) -> &'static str {
    match level {
        10 => "DEBUG",
        20 => "INFO",
        30 => "WARN",
        40 => "ERROR",
        50 => "FATAL",
        _ => "UNKN",
    }
}

fn level_value(s: &str) -> u8 {
    match s.to_uppercase().as_str() {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARN" | "WARNING" => 30,
        "ERROR" => 40,
        "FATAL" => 50,
        _ => 20,
    }
}

pub async fn run(ctx: &Ctx, args: LogArgs, json: bool) -> Result<()> {
    let min_level = level_value(&args.level);
    let topic = format!("{}/rosout/**", ctx.domain);

    let count = args.count;
    let node_filter = args.node.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received_cb = received.clone();

    let _sub = ctx
        .session
        .declare_subscriber(&topic)
        .callback(move |sample| {
            let n = received_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if count == 0 || n <= count {
                let payload = sample.payload().to_bytes().into_owned();
                let _ = tx.try_send(payload);
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !json {
        eprintln!("Subscribing to /rosout (Ctrl+C to stop)...");
    }

    let mut printed = 0usize;
    while let Some(payload) = rx.recv().await {
        if let Some(log) = decode_log(&payload) {
            if log.level < min_level {
                continue;
            }
            if let Some(ref nf) = node_filter {
                if !log.name.contains(nf.as_str()) {
                    continue;
                }
            }

            printed += 1;

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "stamp_sec": log.sec,
                        "stamp_nanosec": log.nanosec,
                        "level": level_name(log.level),
                        "name": log.name,
                        "msg": log.msg,
                        "file": log.file,
                        "function": log.function,
                    })
                );
            } else {
                let ts = format!("{}.{:09}", log.sec, log.nanosec);
                println!(
                    "[{}] [{}] [{}]: {}",
                    ts,
                    level_name(log.level),
                    log.name,
                    log.msg
                );
            }
        }

        if count > 0 && printed >= count {
            break;
        }
    }

    Ok(())
}
