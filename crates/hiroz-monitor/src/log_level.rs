use anyhow::Result;
use clap::{Args, Subcommand};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct LogLevelArgs {
    #[command(subcommand)]
    pub action: LogLevelAction,
}

#[derive(Subcommand)]
pub enum LogLevelAction {
    /// Get logger levels for a node
    Get {
        /// Fully-qualified node name (e.g. /talker)
        node: String,
    },
    /// Set a logger level for a node
    Set {
        /// Fully-qualified node name
        node: String,
        /// Logger name (use node name for the root logger)
        logger: String,
        /// Level: DEBUG, INFO, WARN, ERROR, FATAL, UNSET
        level: String,
    },
}

/// Encode a get_logger_levels request CDR: empty array.
/// rcl_interfaces/srv/GetLoggerLevels_Request: string[] logger_names
fn encode_get_logger_levels_request(logger_names: &[&str]) -> Vec<u8> {
    let mut buf = vec![0x00u8, 0x01, 0x00, 0x00]; // CDR LE header
    let n = logger_names.len() as u32;
    buf.extend_from_slice(&n.to_le_bytes());
    for name in logger_names {
        let bytes = name.as_bytes();
        let len = (bytes.len() + 1) as u32; // +1 for null terminator
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(bytes);
        buf.push(0u8);
        // align
        while !buf.len().is_multiple_of(4) {
            buf.push(0u8);
        }
    }
    buf
}

/// Encode a set_logger_levels request CDR.
/// rcl_interfaces/srv/SetLoggerLevels_Request:
///   rcl_interfaces/msg/LoggerLevel[] levels
///   LoggerLevel: string name, uint32 level
fn encode_set_logger_levels_request(logger: &str, level: &str) -> Vec<u8> {
    let level_val = match level.to_uppercase().as_str() {
        "UNSET" => 0u32,
        "DEBUG" => 10,
        "INFO" => 20,
        "WARN" | "WARNING" => 30,
        "ERROR" => 40,
        "FATAL" => 50,
        _ => 20,
    };

    let mut buf = vec![0x00u8, 0x01, 0x00, 0x00]; // CDR LE header
    // array length = 1
    buf.extend_from_slice(&1u32.to_le_bytes());
    // name string
    let bytes = logger.as_bytes();
    let len = (bytes.len() + 1) as u32;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(bytes);
    buf.push(0u8);
    while !buf.len().is_multiple_of(4) {
        buf.push(0u8);
    }
    // level u32
    buf.extend_from_slice(&level_val.to_le_bytes());
    buf
}

struct LoggerLevel {
    name: String,
    level: u32,
}

impl LoggerLevel {
    fn level_name(&self) -> &'static str {
        match self.level {
            0 => "UNSET",
            1..=10 => "DEBUG",
            11..=20 => "INFO",
            21..=30 => "WARN",
            31..=40 => "ERROR",
            _ => "FATAL",
        }
    }
}

/// Decode `GetLoggerLevels_Response` CDR.
/// Layout: 4-byte header | u32 array_len | repeated { u32 str_len | bytes | null | pad(4) | u32 level }
fn decode_get_logger_levels_response(buf: &[u8]) -> Result<Vec<LoggerLevel>> {
    if buf.len() < 8 {
        anyhow::bail!("CDR too short");
    }
    // buf[1] == 0x01 → little-endian; we only support LE (hiroz always writes LE)
    let mut pos = 4usize; // skip CDR header
    let array_len = u32::from_le_bytes(buf[pos..pos + 4].try_into()?) as usize;
    pos += 4;

    let mut levels = Vec::with_capacity(array_len);
    for _ in 0..array_len {
        if pos + 4 > buf.len() {
            anyhow::bail!("CDR truncated at string length");
        }
        let str_len = u32::from_le_bytes(buf[pos..pos + 4].try_into()?) as usize;
        pos += 4;
        if pos + str_len > buf.len() {
            anyhow::bail!("CDR truncated at string data");
        }
        // str_len includes null terminator
        let name = std::str::from_utf8(&buf[pos..pos + str_len.saturating_sub(1)])
            .unwrap_or("<invalid>")
            .to_owned();
        pos += str_len;
        // align to 4 bytes
        let rem = pos % 4;
        if rem != 0 {
            pos += 4 - rem;
        }
        if pos + 4 > buf.len() {
            anyhow::bail!("CDR truncated at level");
        }
        let level = u32::from_le_bytes(buf[pos..pos + 4].try_into()?);
        pos += 4;
        levels.push(LoggerLevel { name, level });
    }
    Ok(levels)
}

fn print_logger_levels(node: &str, levels: &[LoggerLevel], json: bool) {
    if json {
        let entries: Vec<serde_json::Value> = levels
            .iter()
            .map(|l| serde_json::json!({"name": l.name, "level": l.level, "level_name": l.level_name()}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).unwrap_or_default()
        );
    } else {
        if levels.is_empty() {
            println!("No loggers reported for {}", node);
        } else {
            for l in levels {
                println!("  {}: {}", l.name, l.level_name());
            }
        }
    }
}

pub async fn run(ctx: &Ctx, args: LogLevelArgs, json: bool) -> Result<()> {
    sleep(Duration::from_millis(300)).await;

    match args.action {
        LogLevelAction::Get { node } => {
            let n = node.trim_start_matches('/');
            let ke = format!("{}/{}/get_logger_levels/**", ctx.domain, n);
            let request = encode_get_logger_levels_request(&[]);

            let replies = ctx
                .session
                .get(&ke)
                .payload(zenoh::bytes::ZBytes::from(request))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let mut got = false;
            while let Ok(reply) = replies.recv_async().await {
                got = true;
                match reply.result() {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes().into_owned();
                        match decode_get_logger_levels_response(&bytes) {
                            Ok(levels) => print_logger_levels(&node, &levels, json),
                            Err(_) => {
                                if json {
                                    println!(
                                        "{}",
                                        serde_json::json!({"raw": bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")})
                                    );
                                } else {
                                    println!(
                                        "response: {} bytes (raw CDR — parse failed)",
                                        bytes.len()
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => anyhow::bail!("{e}"),
                }
            }
            if !got {
                anyhow::bail!("No response from {}", node);
            }
        }

        LogLevelAction::Set {
            node,
            logger,
            level,
        } => {
            let n = node.trim_start_matches('/');
            let ke = format!("{}/{}/set_logger_levels/**", ctx.domain, n);
            let request = encode_set_logger_levels_request(&logger, &level);

            let replies = ctx
                .session
                .get(&ke)
                .payload(zenoh::bytes::ZBytes::from(request))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let mut got = false;
            while let Ok(reply) = replies.recv_async().await {
                got = true;
                match reply.result() {
                    Ok(_) => {
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({"node": node, "logger": logger, "level": level, "ok": true})
                            );
                        } else {
                            println!("Set {} logger {} to {}", node, logger, level);
                        }
                    }
                    Err(e) => anyhow::bail!("{e}"),
                }
            }
            if !got {
                anyhow::bail!("No response from {}", node);
            }
        }
    }

    Ok(())
}
