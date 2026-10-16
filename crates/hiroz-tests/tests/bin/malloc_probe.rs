//! Helper process of `malloc_pin_receive`: one publisher or one typed receiver.
//!
//! The receiver loop must run on the process main thread, because glibc serves the main
//! thread from the main arena and every other thread from its own arena. A libtest test body
//! never runs on the main thread, so the measurement lives in a separate binary.
//!
//! Usage:
//!   malloc_probe pub  <size_bytes> <hz> <count>
//!   malloc_probe recv <arm> <count>
//! Arms: `none`, `lib` (the pin of the library), `raw:<mmap_mib>:<trim_mib>` (a probe value),
//! each optionally followed by `+spawn` (receive on a spawned thread) or `+delay<ms>`
//! (sleep before the receive time is read, a deliberately slowed arm).
#![cfg(all(target_os = "linux", feature = "ros-msgs"))]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hiroz::{Builder, MessageTypeInfo, context::ZContextBuilder, malloc_pin::PinOutcome};
use hiroz_msgs::std_msgs::UInt8MultiArray;
use zenoh::Wait;
use zenoh_buffers::buffer::SplitBuffer;

/// Environment variable that names the endpoint, for example `tcp/127.0.0.1:41000`.
const ENDPOINT_VAR: &str = "MALLOC_PROBE_ENDPOINT";
const WARM: usize = 10;
/// Offset of the data field in the CDR payload: header 4, empty dim sequence 4, offset 4, length 4.
const DATA_AT: usize = 16;

fn endpoint() -> String {
    std::env::var(ENDPOINT_VAR).unwrap_or_else(|_| panic!("{ENDPOINT_VAR} is not set"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64
}

fn cpu_ticks() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/stat").expect("stat");
    let f: Vec<&str> = s
        .rsplit(')')
        .next()
        .expect("stat")
        .split_whitespace()
        .collect();
    (f[11].parse().expect("utime"), f[12].parse().expect("stime"))
}

fn publisher(size: usize, hz: f64, count: usize) {
    let mut c = zenoh::Config::default();
    c.insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    c.insert_json5("transport/shared_memory/enabled", "false")
        .unwrap();
    c.insert_json5("mode", "\"peer\"").unwrap();
    c.insert_json5("listen/endpoints", &format!("[\"{}\"]", endpoint()))
        .unwrap();
    let s = zenoh::open(c).wait().expect("open");
    let t0 = Instant::now();
    while s.info().peers_zid().wait().count() == 0 {
        if t0.elapsed() > Duration::from_secs(60) {
            eprintln!("PUB no peer");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(4));
    let key = format!(
        "0/p32/{}/{}",
        UInt8MultiArray::type_name(),
        UInt8MultiArray::type_hash().to_rihs_string()
    );
    let mut p = vec![0u8; DATA_AT + size];
    p[0..4].copy_from_slice(&[0x00, 0x01, 0x00, 0x00]);
    p[12..16].copy_from_slice(&(size as u32).to_le_bytes());
    let per = Duration::from_secs_f64(1.0 / hz);
    let mut next = Instant::now();
    let total = count + WARM + 20;
    for _ in 0..total {
        let mut v = p.clone();
        v[DATA_AT..DATA_AT + 8].copy_from_slice(&now_ns().to_le_bytes());
        s.put(&key, v).wait().expect("put");
        next += per;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    eprintln!("PUB sent={total}");
    std::thread::sleep(Duration::from_secs(2));
}

/// Exit status of a receiver that cannot give a valid arm. The test treats it as a failure.
const EXIT_PIN_GATE: i32 = 4;

/// Whether the outcome of the pin lets the `lib` arm count as pinned.
fn pin_gate(outcome: PinOutcome) -> Result<(), String> {
    match outcome {
        PinOutcome::Pinned { .. } => Ok(()),
        other => Err(format!("the lib arm is not pinned: {other:?}")),
    }
}

/// Apply the arm and return the text of the pin outcome for the RESULT line.
///
/// Exits with `EXIT_PIN_GATE` and the marker `PINGATE` when an operator variable of glibc is
/// set (it changes every arm) or when the `lib` arm does not end up pinned. A comparison of
/// two unpinned arms would pass without measuring the pin.
fn apply_arm(arm: &str) -> String {
    for var in ["MALLOC_TRIM_THRESHOLD_", "MALLOC_MMAP_THRESHOLD_"] {
        if std::env::var_os(var).is_some() {
            eprintln!("PINGATE {var} is set; every arm would run under it");
            std::process::exit(EXIT_PIN_GATE);
        }
    }
    if arm == "lib" {
        let outcome = hiroz::malloc_pin::pin_large_message_thresholds();
        eprintln!("PINOUTCOME {outcome:?}");
        if let Err(why) = pin_gate(outcome) {
            eprintln!("PINGATE {why}");
            std::process::exit(EXIT_PIN_GATE);
        }
        "Pinned".to_string()
    } else if let Some(v) = arm.strip_prefix("raw:") {
        let (mmap, trim) = v.split_once(':').expect("raw:<mmap>:<trim>");
        let mib = |s: &str| s.parse::<i32>().expect("mib") << 20;
        unsafe extern "C" {
            fn mallopt(param: core::ffi::c_int, value: core::ffi::c_int) -> core::ffi::c_int;
        }
        let a = unsafe { mallopt(-3, mib(mmap)) };
        let b = unsafe { mallopt(-1, mib(trim)) };
        eprintln!("RAWPIN mmap_rc={a} trim_rc={b}");
        "raw".to_string()
    } else {
        assert_eq!(arm, "none", "unknown arm {arm}");
        "none".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pinned_passes_the_gate() {
        assert!(
            pin_gate(PinOutcome::Pinned {
                mmap_rc: 1,
                trim_rc: 1
            })
            .is_ok()
        );
        for bad in [
            PinOutcome::Rejected {
                mmap_rc: 0,
                trim_rc: 1,
            },
            PinOutcome::OperatorSet,
            PinOutcome::Unsupported,
        ] {
            assert!(pin_gate(bad).is_err(), "{bad:?} passed the gate");
        }
    }
}

fn receive(arm_full: &str, count: usize) {
    let mut parts = arm_full.split('+');
    let arm = parts.next().expect("arm");
    let mut spawn = false;
    let mut delay_ms = 0u64;
    for m in parts {
        if m == "spawn" {
            spawn = true;
        } else if let Some(d) = m.strip_prefix("delay") {
            delay_ms = d.parse().expect("delay ms");
        }
    }
    let pin = apply_arm(arm);
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint()])
        .with_mode("peer")
        .build()
        .expect("context");
    let node = ctx.create_node("p32_node").build().expect("node");
    let qos = hiroz::qos::QosProfile {
        history: hiroz::qos::QosHistory::KeepLast(std::num::NonZeroUsize::new(10).unwrap()),
        ..Default::default()
    };
    let sub = node
        .create_sub::<UInt8MultiArray>("/p32")
        .with_qos(qos)
        .build()
        .expect("sub");
    let body = move || {
        let mut lat = Vec::new();
        let mut first = None;
        let mut n = 0usize;
        let mut idle = 0;
        while n < WARM + count && idle < 8 {
            match sub.recv_timeout(Duration::from_secs(1)) {
                Ok(m) => {
                    idle = 0;
                    if delay_ms > 0 {
                        std::thread::sleep(Duration::from_millis(delay_ms));
                    }
                    let t = now_ns();
                    n += 1;
                    let c = m.data.0.contiguous();
                    let ts = u64::from_le_bytes(c[0..8].try_into().unwrap());
                    if n == WARM {
                        first = Some((cpu_ticks(), Instant::now()));
                    } else if n > WARM {
                        lat.push(t.saturating_sub(ts) as f64 / 1e6);
                    }
                }
                Err(_) => idle += 1,
            }
        }
        let last = cpu_ticks();
        (lat, first, last)
    };
    let (mut lat, first, last) = if spawn {
        std::thread::spawn(body).join().expect("join")
    } else {
        body()
    };
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = lat.len();
    let p50 = if k == 0 { f64::NAN } else { lat[(k - 1) / 2] };
    let (user, sys) = match first {
        Some(((u0, s0), _)) if k > 0 => (
            (last.0 - u0) as f64 * 10.0 / k as f64,
            (last.1 - s0) as f64 * 10.0 / k as f64,
        ),
        _ => (f64::NAN, f64::NAN),
    };
    println!(
        "RESULT arm={arm_full} pin={pin} n={k}/{count} p50_ms={p50:.3} user_ms={user:.2} sys_ms={sys:.2}"
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("pub") => publisher(
            a[2].parse().unwrap(),
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
        ),
        Some("recv") => receive(&a[2], a[3].parse().unwrap()),
        _ => {
            eprintln!("usage: malloc_probe pub SIZE HZ COUNT | recv ARM COUNT");
            std::process::exit(2);
        }
    }
}
