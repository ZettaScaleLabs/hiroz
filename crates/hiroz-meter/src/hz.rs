use anyhow::Result;
use clap::Args;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc, time::Instant};
use tokio::time::{Duration, sleep};

use crate::{context::Ctx, qos_warn};

#[derive(Args)]
pub struct HzArgs {
    /// Topic name(s) to monitor (omit with --all to monitor all topics)
    pub topics: Vec<String>,

    /// Monitor all topics discovered in the graph
    #[arg(long, conflicts_with = "topics")]
    pub all: bool,

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

struct TopicHz {
    name: String,
    times: Arc<Mutex<VecDeque<Instant>>>,
}

pub async fn run(ctx: &Ctx, args: HzArgs, json: bool) -> Result<()> {
    if !args.all && args.topics.is_empty() {
        anyhow::bail!("specify at least one topic or use --all");
    }

    let topic_names: Vec<String> = if args.all {
        // Brief wait for graph discovery.
        sleep(Duration::from_millis(500)).await;
        ctx.graph
            .get_topic_names_and_types()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    } else {
        args.topics.clone()
    };

    let window = args.window;
    let mut trackers: Vec<TopicHz> = Vec::new();
    let mut _subs = Vec::new();

    for topic_name in &topic_names {
        let topic = topic_name.trim_start_matches('/').to_string();
        let ke = format!("{}/{}/**", ctx.domain, topic);
        let times: Arc<Mutex<VecDeque<Instant>>> = Arc::new(Mutex::new(VecDeque::new()));
        let times_cb = times.clone();

        let sub = ctx
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

        tokio::spawn(qos_warn::warn_if_qos_mismatch(
            ctx.graph.clone(),
            topic_name.clone(),
        ));

        trackers.push(TopicHz {
            name: topic_name.clone(),
            times,
        });
        _subs.push(sub);
    }

    let interval = Duration::from_secs_f64(args.interval);
    let deadline = (args.duration > 0.0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_secs_f64(args.duration));

    loop {
        sleep(interval).await;
        if let Some(dl) = deadline
            && std::time::Instant::now() >= dl
        {
            break Ok(());
        }

        for tracker in &trackers {
            let t = tracker.times.lock();
            let n = t.len();
            if n < 2 {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"topic": tracker.name, "status": "waiting"})
                    );
                } else {
                    println!("Waiting for messages on {}...", tracker.name);
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
            let variance =
                deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / deltas.len() as f64;
            let std_dev = variance.sqrt();

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "topic": tracker.name,
                        "rate_hz": rate,
                        "min_delta_s": min,
                        "max_delta_s": max,
                        "std_dev_s": std_dev,
                        "window": n,
                    })
                );
            } else {
                println!(
                    "{}: average rate: {:.3}\n\tmin: {:.3}s max: {:.3}s std dev: {:.5}s window: {}",
                    tracker.name, rate, min, max, std_dev, n
                );
            }
        }
    }
}
