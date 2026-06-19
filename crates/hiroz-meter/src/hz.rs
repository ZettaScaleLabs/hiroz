use anyhow::Result;
use clap::Args;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc, time::Instant};
use tokio::time::{Duration, sleep};

use crate::context::Ctx;

#[derive(Args)]
pub struct HzArgs {
    /// Topic name (e.g. /chatter)
    pub topic: String,

    /// Sliding window size (number of messages)
    #[arg(long, default_value = "100")]
    pub window: usize,

    /// Reporting interval in seconds
    #[arg(long, default_value = "1.0")]
    pub interval: f64,

    /// Stop after this many seconds (0 = run indefinitely)
    #[arg(long, default_value = "0")]
    pub duration: f64,
}

pub async fn run(ctx: &Ctx, args: HzArgs, json: bool) -> Result<()> {
    let topic = args.topic.trim_start_matches('/').to_string();
    let ke = format!("{}/{}/**", ctx.domain, topic);

    let times: Arc<Mutex<VecDeque<Instant>>> = Arc::new(Mutex::new(VecDeque::new()));
    let window = args.window;
    let times_cb = times.clone();

    let _sub = ctx
        .session
        .declare_subscriber(&ke)
        .callback(move |_sample| {
            let mut t = times_cb.lock();
            t.push_back(Instant::now());
            if t.len() > window {
                t.pop_front();
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let interval = Duration::from_secs_f64(args.interval);
    let deadline = (args.duration > 0.0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_secs_f64(args.duration));

    loop {
        sleep(interval).await;
        if let Some(dl) = deadline {
            if std::time::Instant::now() >= dl {
                break Ok(());
            }
        }

        let t = times.lock();
        let n = t.len();
        if n < 2 {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"topic": args.topic, "status": "waiting"})
                );
            } else {
                println!("Waiting for messages on {}...", args.topic);
            }
            continue;
        }

        let deltas: Vec<f64> = t
            .iter()
            .zip(t.iter().skip(1))
            .map(|(a, b)| b.duration_since(*a).as_secs_f64())
            .collect();

        let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
        let rate = if mean > 0.0 { 1.0 / mean } else { 0.0 };
        let min = deltas.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = deltas.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let variance = deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / deltas.len() as f64;
        let std_dev = variance.sqrt();

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "topic": args.topic,
                    "rate_hz": rate,
                    "min_delta_s": min,
                    "max_delta_s": max,
                    "std_dev_s": std_dev,
                    "window": n,
                })
            );
        } else {
            println!(
                "average rate: {:.3}\n\tmin: {:.3}s max: {:.3}s std dev: {:.5}s window: {}",
                rate, min, max, std_dev, n
            );
        }
    }
}
