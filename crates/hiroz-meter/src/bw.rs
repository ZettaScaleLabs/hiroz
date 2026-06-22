use anyhow::Result;
use clap::Args;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};
use tokio::time::{Duration, Instant, sleep};

use crate::{context::Ctx, qos_warn};

#[derive(Args)]
pub struct BwArgs {
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

struct Sample {
    time: Instant,
    bytes: usize,
}

struct TopicBw {
    name: String,
    samples: Arc<Mutex<VecDeque<Sample>>>,
}

pub async fn run(ctx: &Ctx, args: BwArgs, json: bool) -> Result<()> {
    if !args.all && args.topics.is_empty() {
        anyhow::bail!("specify at least one topic or use --all");
    }

    let topic_names: Vec<String> = if args.all {
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
    let mut trackers: Vec<TopicBw> = Vec::new();
    let mut _subs = Vec::new();

    for topic_name in &topic_names {
        let topic = topic_name.trim_start_matches('/').to_string();
        let ke = format!("{}/{}/**", ctx.domain, topic);
        let samples: Arc<Mutex<VecDeque<Sample>>> = Arc::new(Mutex::new(VecDeque::new()));
        let samples_cb = samples.clone();

        let sub = ctx
            .session
            .declare_subscriber(&ke)
            .callback(move |sample| {
                let bytes = sample.payload().len();
                let mut s = samples_cb.lock();
                s.push_back(Sample {
                    time: Instant::now(),
                    bytes,
                });
                if s.len() > window {
                    s.pop_front();
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        tokio::spawn(qos_warn::warn_if_qos_mismatch(
            ctx.graph.clone(),
            topic_name.clone(),
        ));

        trackers.push(TopicBw {
            name: topic_name.clone(),
            samples,
        });
        _subs.push(sub);
    }

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

        for tracker in &trackers {
            let s = tracker.samples.lock();
            let n = s.len();
            if n < 2 {
                if !json {
                    println!("Waiting for messages on {}...", tracker.name);
                }
                continue;
            }

            let window_secs = s
                .back()
                .unwrap()
                .time
                .duration_since(s.front().unwrap().time)
                .as_secs_f64();

            let total_bytes: usize = s.iter().map(|x| x.bytes).sum();
            let mean_bytes = total_bytes as f64 / n as f64;
            let bw_bps = if window_secs > 0.0 {
                total_bytes as f64 / window_secs
            } else {
                0.0
            };
            let min_bytes = s.iter().map(|x| x.bytes).min().unwrap_or(0);
            let max_bytes = s.iter().map(|x| x.bytes).max().unwrap_or(0);

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "topic": tracker.name,
                        "bandwidth_bps": bw_bps,
                        "bandwidth_kbps": bw_bps / 1000.0,
                        "mean_msg_bytes": mean_bytes,
                        "min_msg_bytes": min_bytes,
                        "max_msg_bytes": max_bytes,
                        "window": n,
                    })
                );
            } else {
                let (bw_str, unit) = if bw_bps >= 1_000_000.0 {
                    (bw_bps / 1_000_000.0, "MB/s")
                } else if bw_bps >= 1_000.0 {
                    (bw_bps / 1_000.0, "KB/s")
                } else {
                    (bw_bps, "B/s")
                };
                println!(
                    "{}: average: {:.2} {}  mean msg size: {:.0} B  min: {} B  max: {} B  window: {}",
                    tracker.name, bw_str, unit, mean_bytes, min_bytes, max_bytes, n
                );
            }
        }
    }
}
