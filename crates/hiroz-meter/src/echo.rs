use anyhow::Result;
use clap::Args;
use hiroz::dynamic::{DynamicMessage, DynamicValue};
use std::sync::Arc;
use tokio::time::Duration;

use crate::{context::Ctx, qos_warn};

#[derive(Args)]
pub struct EchoArgs {
    /// Topic name
    pub topic: String,

    /// Number of messages to print (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub count: usize,

    /// Print raw bytes as hex instead of type-aware output
    #[arg(long)]
    pub raw: bool,

    /// Timeout in seconds waiting for each message (0 = no timeout)
    #[arg(long, default_value = "0")]
    pub timeout: f64,
}

pub async fn run(ctx: &Ctx, args: EchoArgs, json: bool) -> Result<()> {
    tokio::spawn(qos_warn::warn_if_qos_mismatch(
        ctx.graph.clone(),
        args.topic.clone(),
    ));

    let topic = args.topic.trim_start_matches('/').to_string();
    let ke = format!("{}/{}/**", ctx.domain, topic);
    let count = args.count;
    let topic_name = args.topic.clone();

    // Discover schema unless --raw is requested.
    let schema = if !args.raw {
        match ctx
            .node
            .discover_topic_schema(&args.topic, Duration::from_secs(5))
            .await
        {
            Ok(d) => {
                if !json {
                    eprintln!(
                        "[info] Subscribed to {} ({})",
                        args.topic, d.schema.type_name
                    );
                }
                Some(Arc::clone(&d.schema))
            }
            Err(e) => {
                eprintln!("[warn] Type discovery failed ({}), printing hex", e);
                None
            }
        }
    } else {
        None
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received_cb = received.clone();

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

    let timeout_dur = (args.timeout > 0.0).then(|| Duration::from_secs_f64(args.timeout));
    let mut printed = 0usize;
    loop {
        let maybe_payload = if let Some(t) = timeout_dur {
            match tokio::time::timeout(t, rx.recv()).await {
                Ok(Some(p)) => Some(p),
                Ok(None) | Err(_) => None,
            }
        } else {
            rx.recv().await
        };
        let Some(payload) = maybe_payload else { break };

        printed += 1;

        if let Some(ref s) = schema {
            match DynamicMessage::from_cdr(&payload, s) {
                Ok(msg) => print_dynamic(&topic_name, printed, &msg, json),
                Err(_) => print_hex(&topic_name, printed, &payload, json),
            }
        } else {
            print_hex(&topic_name, printed, &payload, json);
        }

        if count > 0 && printed >= count {
            break;
        }
    }
    Ok(())
}

fn print_dynamic(topic: &str, seq: usize, msg: &DynamicMessage, json: bool) {
    if json {
        let mut map = serde_json::Map::new();
        map.insert("topic".into(), serde_json::Value::String(topic.to_string()));
        map.insert("seq".into(), serde_json::Value::Number(seq.into()));
        map.insert("data".into(), to_json(msg));
        println!("{}", serde_json::Value::Object(map));
    } else {
        println!("---");
        print!("{}", fmt_msg(msg, 0));
    }
}

fn print_hex(topic: &str, seq: usize, payload: &[u8], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "topic": topic,
                "seq": seq,
                "bytes": payload.len(),
                "data": hex_encode(payload),
            })
        );
    } else {
        println!(
            "[{}] {} bytes  {}",
            seq,
            payload.len(),
            cdr_string_preview(payload).unwrap_or_else(|| hex_encode(payload))
        );
    }
}

fn to_json(msg: &DynamicMessage) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, value) in msg.iter() {
        map.insert(name.to_string(), value_to_json(value));
    }
    serde_json::Value::Object(map)
}

fn value_to_json(value: &DynamicValue) -> serde_json::Value {
    match value {
        DynamicValue::Bool(b) => serde_json::Value::Bool(*b),
        DynamicValue::Int8(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int16(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int32(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Int64(i) => serde_json::Value::Number((*i).into()),
        DynamicValue::Uint8(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint16(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint32(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Uint64(u) => serde_json::Value::Number((*u).into()),
        DynamicValue::Float32(f) => serde_json::Number::from_f64(*f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DynamicValue::String(s) => serde_json::Value::String(s.clone()),
        DynamicValue::Bytes(b) => serde_json::Value::Array(
            b.iter()
                .map(|&byte| serde_json::Value::Number(byte.into()))
                .collect(),
        ),
        DynamicValue::Message(m) => to_json(m),
        DynamicValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(value_to_json).collect())
        }
    }
}

fn fmt_msg(msg: &DynamicMessage, indent: usize) -> String {
    let mut out = String::new();
    for (name, value) in msg.iter() {
        fmt_value(&mut out, name, value, indent);
    }
    out
}

fn fmt_value(out: &mut String, name: &str, value: &DynamicValue, indent: usize) {
    let pad = "  ".repeat(indent);
    match value {
        DynamicValue::Message(m) => {
            out.push_str(&format!("{}{name}:\n", pad));
            for (n2, v2) in m.iter() {
                fmt_value(out, n2, v2, indent + 1);
            }
        }
        DynamicValue::Array(arr) => {
            out.push_str(&format!("{}{name}:\n", pad));
            for (i, item) in arr.iter().enumerate() {
                fmt_value(out, &format!("[{i}]"), item, indent + 1);
            }
        }
        DynamicValue::Bool(b) => out.push_str(&format!("{}{name}: {b}\n", pad)),
        DynamicValue::Int8(i) => out.push_str(&format!("{}{name}: {i}\n", pad)),
        DynamicValue::Int16(i) => out.push_str(&format!("{}{name}: {i}\n", pad)),
        DynamicValue::Int32(i) => out.push_str(&format!("{}{name}: {i}\n", pad)),
        DynamicValue::Int64(i) => out.push_str(&format!("{}{name}: {i}\n", pad)),
        DynamicValue::Uint8(u) => out.push_str(&format!("{}{name}: {u}\n", pad)),
        DynamicValue::Uint16(u) => out.push_str(&format!("{}{name}: {u}\n", pad)),
        DynamicValue::Uint32(u) => out.push_str(&format!("{}{name}: {u}\n", pad)),
        DynamicValue::Uint64(u) => out.push_str(&format!("{}{name}: {u}\n", pad)),
        DynamicValue::Float32(f) => out.push_str(&format!("{}{name}: {f}\n", pad)),
        DynamicValue::Float64(f) => out.push_str(&format!("{}{name}: {f}\n", pad)),
        DynamicValue::String(s) => out.push_str(&format!("{}{name}: \"{s}\"\n", pad)),
        DynamicValue::Bytes(b) => out.push_str(&format!("{}{name}: <{} bytes>\n", pad, b.len())),
    }
}

fn hex_encode(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{:02x}", x))
        .collect::<Vec<_>>()
        .join(" ")
}

fn cdr_string_preview(payload: &[u8]) -> Option<String> {
    if payload.len() < 8 {
        return None;
    }
    let data = &payload[4..];
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
