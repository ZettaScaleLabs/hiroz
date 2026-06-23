//! Parameterized benchmark comparing `hu meter hz` against `ros2 topic hz`.
//!
//! Run:
//!   hu-compare --rates 100,500,1000,sat --duration 10 --output results.json

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use hiroz::ZContextBuilder;
use hiroz_msgs::std_msgs::String as RosString;
use serde_json::json;

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "hu-compare", about = "Compare hu meter hz vs ros2 topic hz")]
struct Cli {
    /// Comma-separated rate specs: numbers (Hz) or "sat" for saturation.
    #[arg(long, default_value = "100,500,1000,2000,5000,sat")]
    rates: String,

    /// Measurement window in seconds for each case.
    #[arg(long, default_value_t = 10.0)]
    duration: f64,

    /// Topic base name (a suffix is appended per case).
    #[arg(long, default_value = "hu_compare")]
    topic_base: String,

    /// Zenoh router endpoint. Defaults to spawning an in-process router per case.
    #[arg(long)]
    router: Option<String>,

    /// Write JSON results to this path.
    #[arg(long)]
    output: Option<String>,
}

// ─── Rate spec ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum RateSpec {
    Fixed(f64),
    Saturation,
}

impl RateSpec {
    fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s == "sat" || s == "saturation" {
            Ok(RateSpec::Saturation)
        } else {
            s.parse::<f64>()
                .map(RateSpec::Fixed)
                .map_err(|_| format!("invalid rate spec: '{s}'"))
        }
    }

    fn label(&self) -> String {
        match self {
            RateSpec::Fixed(hz) => format!("{hz}"),
            RateSpec::Saturation => "sat".to_string(),
        }
    }
}

// ─── Output parsers ──────────────────────────────────────────────────────────

fn parse_hu_meter_hz(stdout: &str) -> Option<f64> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(rate) = v.get("rate_hz").and_then(|r| r.as_f64()) {
                return Some(rate);
            }
        }
    }
    None
}

fn parse_ros2_hz(stdout: &str) -> Option<f64> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("average rate:") {
            if let Ok(hz) = rest.trim().parse::<f64>() {
                return Some(hz);
            }
        }
    }
    None
}

// ─── Per-case runner ─────────────────────────────────────────────────────────

struct CaseResult {
    rate_spec: String,
    target_hz: Option<f64>,
    ground_truth_hz: f64,
    hu_hz: f64,
    ros2_hz: Option<f64>,
}

fn run_case(spec: &RateSpec, duration_secs: f64, topic: &str, endpoint: &str) -> CaseResult {
    let target_hz = match spec {
        RateSpec::Fixed(hz) => Some(*hz),
        RateSpec::Saturation => None,
    };

    // Publisher thread — runs for duration + 5s margin.
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();
    let endpoint2 = endpoint.to_string();
    let topic2 = topic.to_string();
    let spec_clone = spec.clone();

    let pub_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let ctx = ZContextBuilder::default()
                .with_connect_endpoints([endpoint2.as_str()])
                .build()
                .unwrap();
            let node = ctx.create_node("hu_compare_pub").build().unwrap();
            let pub_ = node
                .create_pub::<RosString>(&format!("/{topic2}"))
                .build()
                .unwrap();
            let mut count: u64 = 0;
            let start = Instant::now();
            let deadline = start + Duration::from_secs_f64(duration_secs + 5.0);

            match spec_clone {
                RateSpec::Fixed(hz) => {
                    let interval = Duration::from_secs_f64(1.0 / hz);
                    let mut next = start;
                    while Instant::now() < deadline && !stop_clone.load(Ordering::Relaxed) {
                        let _ = pub_
                            .async_publish(&RosString {
                                data: "x".to_string(),
                            })
                            .await;
                        count += 1;
                        next += interval;
                        let now = Instant::now();
                        if next > now {
                            tokio::time::sleep(next - now).await;
                        }
                    }
                }
                RateSpec::Saturation => {
                    while Instant::now() < deadline && !stop_clone.load(Ordering::Relaxed) {
                        let _ = pub_
                            .async_publish(&RosString {
                                data: "x".to_string(),
                            })
                            .await;
                        count += 1;
                        tokio::task::yield_now().await;
                    }
                }
            }

            let elapsed = start.elapsed().as_secs_f64();
            count as f64 / elapsed
        })
    });

    // Let the publisher settle.
    std::thread::sleep(Duration::from_millis(500));

    // hu meter hz measurement.
    let hu_out = Command::new("hu")
        .args([
            "meter",
            "--router",
            endpoint,
            "hz",
            &format!("/{topic}"),
            "--duration",
            &duration_secs.to_string(),
            "--json",
        ])
        .output()
        .expect("failed to run 'hu meter hz' — is hu in PATH?");
    let hu_stdout = String::from_utf8_lossy(&hu_out.stdout).into_owned();
    if !hu_out.status.success() {
        eprintln!("[hu] stderr: {}", String::from_utf8_lossy(&hu_out.stderr));
    }
    let hu_hz = parse_hu_meter_hz(&hu_stdout).unwrap_or_else(|| {
        eprintln!("[hu] failed to parse rate from: {hu_stdout}");
        0.0
    });

    // ros2 topic hz measurement (concurrent with publisher, optional).
    let ros2_hz = if Command::new("ros2").arg("--help").output().is_ok() {
        let mut child = Command::new("ros2")
            .args(["topic", "hz", &format!("/{topic}"), "--window", "50"])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env(
                "ZENOH_CONFIG_OVERRIDE",
                format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn ros2 topic hz");

        std::thread::sleep(Duration::from_secs_f64(duration_secs));
        let _ = child.kill();
        let out = child.wait_with_output().ok();
        out.as_ref()
            .and_then(|o| parse_ros2_hz(&String::from_utf8_lossy(&o.stdout)))
    } else {
        eprintln!("ros2 CLI not found — skipping ros2 measurement");
        None
    };

    // Stop publisher and recover ground-truth rate.
    stop_flag.store(true, Ordering::Relaxed);
    let ground_truth_hz = pub_handle.join().unwrap_or(0.0);

    CaseResult {
        rate_spec: spec.label(),
        target_hz,
        ground_truth_hz,
        hu_hz,
        ros2_hz,
    }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let rates: Vec<RateSpec> = cli
        .rates
        .split(',')
        .map(|s| RateSpec::parse(s).unwrap_or_else(|e| panic!("{e}")))
        .collect();

    let mut results = Vec::new();

    for (i, spec) in rates.iter().enumerate() {
        let topic = format!("{}_{i}", cli.topic_base);

        // Determine router endpoint — either user-supplied or spin up an in-process one.
        let router_guard;
        let endpoint: String;

        if let Some(ref ep) = cli.router {
            router_guard = None;
            endpoint = ep.clone();
        } else {
            let router = TestRouter::new();
            endpoint = router.endpoint().to_string();
            router_guard = Some(router);
        }

        eprintln!(
            "[{}/{}] rate={} topic=/{} router={}",
            i + 1,
            rates.len(),
            spec.label(),
            topic,
            endpoint,
        );

        let case = run_case(spec, cli.duration, &topic, &endpoint);

        // Keep router alive until case is done.
        drop(router_guard);

        let hu_error_pct = if case.ground_truth_hz > 0.0 {
            ((case.hu_hz - case.ground_truth_hz) / case.ground_truth_hz * 100.0).abs()
        } else {
            0.0
        };
        let advantage_x = case.ros2_hz.map(|r2| {
            if r2 > 0.0 {
                case.hu_hz / r2
            } else {
                f64::INFINITY
            }
        });

        // Human-readable line.
        let ros2_str = case
            .ros2_hz
            .map(|r| format!("{r:.1}"))
            .unwrap_or_else(|| "n/a".to_string());
        let adv_str = advantage_x.map(|x| format!("{x:.1}×")).unwrap_or_default();

        eprintln!(
            "  ground_truth={:.1} hu={:.1} ({hu_error_pct:.1}% err) ros2={ros2_str} adv={adv_str}",
            case.ground_truth_hz, case.hu_hz,
        );

        results.push(json!({
            "rate_spec": case.rate_spec,
            "target_hz": case.target_hz,
            "ground_truth_hz": case.ground_truth_hz,
            "hu_hz": case.hu_hz,
            "hu_error_pct": hu_error_pct,
            "ros2_hz": case.ros2_hz,
            "hu_advantage_x": advantage_x,
        }));
    }

    // Print summary table.
    println!(
        "\n{:<8} {:>14} {:>14} {:>12} {:>12} {:>10}",
        "Rate", "Ground truth", "hu meter hz", "Error %", "ros2 hz", "Adv"
    );
    println!("{}", "-".repeat(74));
    for r in &results {
        let spec = r["rate_spec"].as_str().unwrap_or("-");
        let gt = r["ground_truth_hz"].as_f64().unwrap_or(0.0);
        let hu = r["hu_hz"].as_f64().unwrap_or(0.0);
        let err = r["hu_error_pct"].as_f64().unwrap_or(0.0);
        let ros2 = r["ros2_hz"]
            .as_f64()
            .map(|x| format!("{x:.1}"))
            .unwrap_or_else(|| "n/a".to_string());
        let adv = r["hu_advantage_x"]
            .as_f64()
            .map(|x| format!("{x:.1}×"))
            .unwrap_or_default();
        println!("{spec:<8} {gt:>14.1} {hu:>14.1} {err:>11.1}% {ros2:>12} {adv:>10}");
    }

    // JSON output.
    let json_out = serde_json::to_string_pretty(&results)?;
    if let Some(path) = &cli.output {
        std::fs::write(path, &json_out)?;
        eprintln!("\nResults written to {path}");
    } else {
        println!("\n{json_out}");
    }

    Ok(())
}

// ─── In-process test router ──────────────────────────────────────────────────

struct TestRouter {
    _session: zenoh::Session,
    endpoint: String,
}

impl TestRouter {
    fn new() -> Self {
        use zenoh::Wait;
        use zenoh::config::WhatAmI;

        for _ in 0..5u32 {
            let port = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind port 0");
                listener.local_addr().unwrap().port()
            };
            let endpoint = format!("tcp/127.0.0.1:{port}");

            let mut config = zenoh::Config::default();
            config.set_mode(Some(WhatAmI::Router)).unwrap();
            config
                .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
                .unwrap();
            config
                .insert_json5("scouting/multicast/enabled", "false")
                .unwrap();

            if let Ok(session) = zenoh::open(config).wait() {
                std::thread::sleep(Duration::from_millis(500));
                return Self {
                    _session: session,
                    endpoint,
                };
            }
        }
        panic!("failed to start in-process Zenoh router after 5 attempts");
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
