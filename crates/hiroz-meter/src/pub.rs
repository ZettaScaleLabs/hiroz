use anyhow::Result;
use clap::Args;
use tokio::time::{Duration, interval};
use zenoh::bytes::ZBytes;

use crate::context::Ctx;

#[derive(Args)]
pub struct PubArgs {
    /// Topic name (e.g. /chatter)
    pub topic: String,

    /// Message type (e.g. std_msgs/msg/String) — used for --yaml encoding
    #[arg(long)]
    pub msg_type: Option<String>,

    /// Payload as space-separated hex bytes (CDR-encoded)
    #[arg(long, conflicts_with = "file")]
    pub payload: Option<String>,

    /// Payload from file (raw CDR bytes)
    #[arg(long, conflicts_with = "payload")]
    pub file: Option<String>,

    /// Convenience: publish a plain string wrapped in CDR std_msgs/String encoding
    #[arg(long, conflicts_with_all = &["payload", "file", "yaml"])]
    pub string: Option<String>,

    /// Publish message from YAML (requires --msg-type).
    /// Supports std_msgs primitives: String, Bool, Int8/16/32/64, UInt8/16/32/64,
    /// Float32/64, and geometry_msgs/msg/Vector3.
    /// Example: --yaml '{data: hello}' --msg-type std_msgs/msg/String
    #[arg(long, conflicts_with_all = &["payload", "file", "string"])]
    pub yaml: Option<String>,

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
    if let Some(yaml_str) = &args.yaml {
        let msg_type = args.msg_type.as_deref().unwrap_or("");
        return yaml_to_cdr(yaml_str, msg_type);
    }
    // Default: empty CDR (4-byte header only)
    Ok(vec![0u8; 4])
}

/// Encode a YAML string to CDR bytes for supported std_msgs primitive types.
/// msg_type may be "std_msgs/msg/String", "std_msgs/String", or just "String".
fn yaml_to_cdr(yaml_str: &str, msg_type: &str) -> Result<Vec<u8>> {
    let v: serde_yaml::Value =
        serde_yaml::from_str(yaml_str).map_err(|e| anyhow::anyhow!("Invalid YAML: {e}"))?;

    // Normalise type string: "std_msgs/msg/String" → "String", "geometry_msgs/msg/Vector3" → "geometry_msgs/Vector3"
    let type_short = msg_type
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(msg_type);
    let type_pkg = if msg_type.starts_with("geometry_msgs") {
        format!("geometry_msgs/{type_short}")
    } else {
        type_short.to_owned()
    };

    match type_pkg.as_str() {
        "String" => {
            let s = yaml_field_str(&v, "data")?;
            Ok(encode_cdr_string(&s))
        }
        "Bool" => {
            let b = yaml_field_bool(&v, "data")?;
            Ok(encode_cdr_primitive(&[b as u8]))
        }
        "Int8" | "Byte" | "Char" => {
            let n = yaml_field_i64(&v, "data")?;
            Ok(encode_cdr_primitive(&[n as i8 as u8]))
        }
        "Int16" => {
            let n = yaml_field_i64(&v, "data")?;
            Ok(encode_cdr_primitive(&(n as i16).to_le_bytes()))
        }
        "Int32" => {
            let n = yaml_field_i64(&v, "data")?;
            Ok(encode_cdr_primitive(&(n as i32).to_le_bytes()))
        }
        "Int64" => {
            let n = yaml_field_i64(&v, "data")?;
            Ok(encode_cdr_primitive(&n.to_le_bytes()))
        }
        "UInt8" => {
            let n = yaml_field_u64(&v, "data")?;
            Ok(encode_cdr_primitive(&[n as u8]))
        }
        "UInt16" => {
            let n = yaml_field_u64(&v, "data")?;
            Ok(encode_cdr_primitive(&(n as u16).to_le_bytes()))
        }
        "UInt32" => {
            let n = yaml_field_u64(&v, "data")?;
            Ok(encode_cdr_primitive(&(n as u32).to_le_bytes()))
        }
        "UInt64" => {
            let n = yaml_field_u64(&v, "data")?;
            Ok(encode_cdr_primitive(&n.to_le_bytes()))
        }
        "Float32" => {
            let f = yaml_field_f64(&v, "data")?;
            Ok(encode_cdr_primitive(&(f as f32).to_le_bytes()))
        }
        "Float64" => {
            let f = yaml_field_f64(&v, "data")?;
            Ok(encode_cdr_primitive(&f.to_le_bytes()))
        }
        "geometry_msgs/Vector3" => {
            let x = yaml_field_f64(&v, "x")?;
            let y = yaml_field_f64(&v, "y")?;
            let z = yaml_field_f64(&v, "z")?;
            let mut buf = cdr_header();
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
            buf.extend_from_slice(&z.to_le_bytes());
            Ok(buf)
        }
        other => {
            anyhow::bail!(
                "YAML encoding not supported for type '{other}'. \
                 Supported: std_msgs/msg/{{String,Bool,Int8/16/32/64,UInt8/16/32/64,Float32/64}}, \
                 geometry_msgs/msg/Vector3. For other types use --payload <hex> or --file."
            )
        }
    }
}

fn cdr_header() -> Vec<u8> {
    vec![0x00, 0x01, 0x00, 0x00]
}

fn encode_cdr_primitive(payload: &[u8]) -> Vec<u8> {
    let mut buf = cdr_header();
    buf.extend_from_slice(payload);
    buf
}

fn yaml_field_str(v: &serde_yaml::Value, field: &str) -> Result<String> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let key = serde_yaml::Value::String(field.to_string());
            match m.get(&key) {
                Some(serde_yaml::Value::String(s)) => Ok(s.clone()),
                Some(other) => Ok(format!("{other:?}")),
                None => anyhow::bail!("Missing field '{field}' in YAML"),
            }
        }
        serde_yaml::Value::String(s) => Ok(s.clone()),
        _ => anyhow::bail!("Expected YAML mapping with '{field}' field"),
    }
}

fn yaml_field_bool(v: &serde_yaml::Value, field: &str) -> Result<bool> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let key = serde_yaml::Value::String(field.to_string());
            match m.get(&key) {
                Some(serde_yaml::Value::Bool(b)) => Ok(*b),
                Some(other) => anyhow::bail!("Expected bool for '{field}', got {other:?}"),
                None => anyhow::bail!("Missing field '{field}' in YAML"),
            }
        }
        serde_yaml::Value::Bool(b) => Ok(*b),
        _ => anyhow::bail!("Expected YAML mapping with '{field}' field"),
    }
}

fn yaml_field_i64(v: &serde_yaml::Value, field: &str) -> Result<i64> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let key = serde_yaml::Value::String(field.to_string());
            match m.get(&key) {
                Some(serde_yaml::Value::Number(n)) => n
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Cannot convert '{field}' to i64")),
                Some(other) => anyhow::bail!("Expected number for '{field}', got {other:?}"),
                None => anyhow::bail!("Missing field '{field}' in YAML"),
            }
        }
        serde_yaml::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Cannot convert to i64")),
        _ => anyhow::bail!("Expected YAML mapping with '{field}' field"),
    }
}

fn yaml_field_u64(v: &serde_yaml::Value, field: &str) -> Result<u64> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let key = serde_yaml::Value::String(field.to_string());
            match m.get(&key) {
                Some(serde_yaml::Value::Number(n)) => n
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("Cannot convert '{field}' to u64")),
                Some(other) => anyhow::bail!("Expected number for '{field}', got {other:?}"),
                None => anyhow::bail!("Missing field '{field}' in YAML"),
            }
        }
        serde_yaml::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Cannot convert to u64")),
        _ => anyhow::bail!("Expected YAML mapping with '{field}' field"),
    }
}

fn yaml_field_f64(v: &serde_yaml::Value, field: &str) -> Result<f64> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            let key = serde_yaml::Value::String(field.to_string());
            match m.get(&key) {
                Some(serde_yaml::Value::Number(n)) => n
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("Cannot convert '{field}' to f64")),
                Some(other) => anyhow::bail!("Expected number for '{field}', got {other:?}"),
                None => anyhow::bail!("Missing field '{field}' in YAML"),
            }
        }
        serde_yaml::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Cannot convert to f64")),
        _ => anyhow::bail!("Expected YAML mapping with '{field}' field"),
    }
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
