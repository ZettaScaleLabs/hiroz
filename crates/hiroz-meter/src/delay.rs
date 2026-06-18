use anyhow::Result;
use clap::Args;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct DelayArgs {
    /// Topic name (must carry std_msgs/Header or header field with stamp)
    pub topic: String,

    /// Sliding window size
    #[arg(long, default_value = "100")]
    pub window: usize,

    /// Reporting interval in seconds
    #[arg(long, default_value = "1.0")]
    pub interval: f64,
}

/// Extract a nanosecond timestamp from the first 12 bytes of CDR payload:
/// CDR header (4 bytes) + sec (4 bytes u32 LE) + nanosec (4 bytes u32 LE).
/// Works for any message whose first field is std_msgs/Header.
fn extract_stamp_ns(payload: &[u8]) -> Option<u64> {
    if payload.len() < 12 {
        return None;
    }
    let sec = u32::from_le_bytes(payload[4..8].try_into().ok()?) as u64;
    let nsec = u32::from_le_bytes(payload[8..12].try_into().ok()?) as u64;
    Some(sec * 1_000_000_000 + nsec)
}

pub async fn run(ctx: &Ctx, args: DelayArgs, json: bool) -> Result<()> {
    let topic = args.topic.trim_start_matches('/').to_string();
    let ke = format!("{}/{}/**", ctx.domain, topic);

    let delays: Arc<Mutex<VecDeque<f64>>> = Arc::new(Mutex::new(VecDeque::new()));
    let window = args.window;
    let delays_cb = delays.clone();

    let _sub = ctx
        .session
        .declare_subscriber(&ke)
        .callback(move |sample| {
            let recv_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);

            let payload: Vec<u8> = sample.payload().to_bytes().into_owned();
            if let Some(stamp_ns) = extract_stamp_ns(&payload) {
                if recv_ns >= stamp_ns {
                    let delay_s = (recv_ns - stamp_ns) as f64 / 1e9;
                    let mut d = delays_cb.lock();
                    d.push_back(delay_s);
                    if d.len() > window {
                        d.pop_front();
                    }
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let interval = Duration::from_secs_f64(args.interval);

    loop {
        sleep(interval).await;

        let d = delays.lock();
        let n = d.len();
        if n == 0 {
            if !json {
                println!("Waiting for stamped messages on {}...", args.topic);
            }
            continue;
        }

        let mean = d.iter().sum::<f64>() / n as f64;
        let min = d.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = d.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let std_dev = (d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt();

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "topic": args.topic,
                    "mean_delay_s": mean,
                    "min_delay_s": min,
                    "max_delay_s": max,
                    "std_dev_s": std_dev,
                    "window": n,
                })
            );
        } else {
            println!(
                "delay: mean {:.3}s  min {:.3}s  max {:.3}s  std dev {:.5}s  window: {}",
                mean, min, max, std_dev, n
            );
        }
    }
}
