//! An opted-in raw subscriber must receive from a released `rmw_zenoh_cpp` publisher in buffer
//! mode.
//!
//! A buffer-aware `rmw_zenoh_cpp` writes a message with an unbounded `uint8[]` field only on
//! `<topic key>/_buf_cpu`, and only when a matched subscriber announces buffer support. A plain
//! Zenoh session counts the samples per key, so the cell checks that the publisher really is in
//! buffer mode: the group key carries the samples and the base key none. A raw subscriber built
//! the default way stays out of the group, so it receives nothing from that publisher: it is the
//! negative control.
//!
//! The cell calls `buffer_rmw::buffer_aware_rmw_or_skip`. Without a buffer-aware rmw it prints
//! `Skipping <cell>: ...` and returns. Setting `HIROZ_REQUIRE_BUFFER_RMW` turns that skip into a
//! failure.

#![cfg(target_os = "linux")]
#![cfg(all(feature = "ros-interop", feature = "raw-sub-tests"))]

mod common;

use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use buffer_rmw::{KeyCounts, SeqSet};
use common::{buffer_rmw, *};
use hiroz::{Builder, MessageTypeInfo};
use hiroz_msgs::std_msgs::UInt8MultiArray;

/// The messages the cell publishes.
const SAMPLES: u32 = 30;

/// The `data` bytes of each message.
const SIZE: usize = 64;

/// The length of one raw sample: the 4-byte CDR header, the length of the empty `layout.dim`,
/// `layout.data_offset`, the length of `data`, then `data`. `data` starts at byte 16.
const SAMPLE_LEN: usize = 16 + SIZE;

/// A released `rclpy` publisher of `std_msgs/UInt8MultiArray`. Message number `seq` carries
/// `seq` as 4 little-endian bytes at the start of `data`. It waits for `subs` matched
/// subscribers, then publishes `n` messages at 5 Hz.
const SEQ_PUBLISHER: &str = r#"
import sys, time, rclpy
from std_msgs.msg import UInt8MultiArray
topic, n, size, subs = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
rclpy.init()
node = rclpy.create_node('seq_pub')
pub = node.create_publisher(UInt8MultiArray, topic, 10)
t0 = time.time()
while pub.get_subscription_count() < subs and time.time() - t0 < 20:
    rclpy.spin_once(node, timeout_sec=0.1)
print('PUB matched=%d' % pub.get_subscription_count(), flush=True)
time.sleep(1.0)
for seq in range(n):
    data = bytearray(size)
    data[0:4] = seq.to_bytes(4, 'little')
    m = UInt8MultiArray()
    m.data = list(data)
    pub.publish(m)
    rclpy.spin_once(node, timeout_sec=0.2)
print('PUB published=%d' % n, flush=True)
time.sleep(1.0)
node.destroy_node()
rclpy.shutdown()
"#;

/// A released `ros2 topic echo` subscriber. It announces buffer support, and a publisher only
/// writes the group key when at least one matched subscriber announces it.
fn spawn_released_subscriber(router: &TestRouter, topic: &str) -> ProcessGuard {
    use std::os::unix::process::CommandExt;
    let child = Command::new("ros2")
        .args([
            "topic",
            "echo",
            topic,
            "std_msgs/msg/UInt8MultiArray",
            "--no-arr",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .expect("spawn the released subscriber");
    ProcessGuard::new(child, "released subscriber")
}

fn spawn_seq_publisher(router: &TestRouter, topic: &str, subs: usize) -> ProcessGuard {
    let child = Command::new("python3")
        .args([
            "-c",
            SEQ_PUBLISHER,
            topic,
            &SAMPLES.to_string(),
            &SIZE.to_string(),
            &subs.to_string(),
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the rclpy publisher");
    ProcessGuard::new(child, "rclpy publisher")
}

/// Records the sequence number of one raw sample. A sample of another length counts for nothing.
fn record(set: &Mutex<SeqSet>, sample: &[u8]) {
    if sample.len() == SAMPLE_LEN {
        set.lock().unwrap().record(&sample[16..]);
    }
}

/// An opted-in raw subscriber receives every sample a publisher in buffer mode writes. A default
/// raw subscriber on the same topic receives none, which shows the publisher writes the group key
/// only. The payload carries a sequence number, so a repeat and a gap show.
#[test]
fn raw_opt_in_subscriber_receives_from_a_released_publisher() {
    const NAME: &str = "raw_opt_in_subscriber_receives_from_a_released_publisher";
    assert!(
        check_ros2_available(),
        "{NAME}: ros2 is required: run this test where a released ROS 2 is installed"
    );
    if !buffer_rmw::buffer_aware_rmw_or_skip(NAME) {
        return;
    }
    let topic = "/rawcell";
    let hash = UInt8MultiArray::type_hash().to_rihs_string();
    let type_name = UInt8MultiArray::type_name();
    let base_key = format!("0/rawcell/{type_name}/{hash}");

    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("rawcell_node").build().expect("node");

    let opt_in = Arc::new(Mutex::new(SeqSet::new(SIZE)));
    let default = Arc::new(Mutex::new(SeqSet::new(SIZE)));
    let sink = opt_in.clone();
    let _opt_in_sub = node
        .create_raw_subscriber_with_group(
            topic,
            type_name,
            &hash,
            move |bytes: &[u8]| record(&sink, bytes),
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in raw subscriber");
    let sink = default.clone();
    let _default_sub = node
        .create_raw_subscriber(topic, type_name, &hash, move |bytes: &[u8]| {
            record(&sink, bytes)
        })
        .expect("default raw subscriber");

    let keys = KeyCounts::new(router.endpoint(), &base_key);
    wait_for_ready(Duration::from_secs(2));
    let _released = spawn_released_subscriber(&router, topic);
    wait_for_ready(Duration::from_secs(2));
    // Three matched subscribers: the released one and the two raw ones.
    let mut publisher = spawn_seq_publisher(&router, topic, 3);

    // Wait for the publisher to exit, then give its last samples two seconds to arrive.
    let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
    let mut finished_at: Option<Instant> = None;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
        let done = publisher
            .child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_some());
        if done && finished_at.is_none() {
            finished_at = Some(Instant::now());
        }
        if finished_at.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
            break;
        }
    }

    let (group, base) = (keys.group(), keys.base());
    for (arm, set) in [("opt_in", &opt_in), ("default", &default)] {
        let set = set.lock().unwrap();
        println!(
            "RESULT {NAME}/{arm}: received={} of {SAMPLES} duplicates={} group={group} base={base}",
            set.received_of(SAMPLES),
            set.duplicates
        );
    }

    assert!(
        group == SAMPLES as usize && base == 0,
        "{NAME}: the plain session read group {group} and base {base} of {SAMPLES}, so the \
         publisher is not in buffer mode"
    );
    let default = default.lock().unwrap();
    assert_eq!(
        default.received_of(SAMPLES),
        0,
        "{NAME}: the default raw subscriber received from a publisher in buffer mode, so the \
         negative control is not valid"
    );
    let opt_in = opt_in.lock().unwrap();
    assert_eq!(
        opt_in.received_of(SAMPLES),
        SAMPLES as usize,
        "{NAME}: opt_in received"
    );
    assert_eq!(opt_in.duplicates, 0, "{NAME}: opt_in repeats");
}
