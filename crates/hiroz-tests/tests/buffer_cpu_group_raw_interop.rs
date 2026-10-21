//! A default raw subscriber must receive from a released `rmw_zenoh_cpp` publisher in buffer
//! mode.
//!
//! A buffer-aware `rmw_zenoh_cpp` writes a message with an unbounded `uint8[]` field only on
//! `<topic key>/_buf_cpu`, and only when a matched subscriber announces buffer support. A plain
//! Zenoh session counts the samples per key, so each cell checks that the publisher really is in
//! buffer mode: the group key carries the samples and the base key none. A raw subscriber built
//! with `BufferGroup::Off` stays out of the group, so it receives nothing from that publisher: it
//! is the negative control.
//!
//! One cell mixes a released subscriber with three raw subscribers. The other cell has the default
//! raw subscriber as the only subscriber, so a match of the publisher can only come from the token
//! of the raw subscriber. The publisher reports how many subscribers it matched, and the cell
//! requires one.
//!
//! The cells call `buffer_rmw::buffer_aware_rmw_or_skip`. Without a buffer-aware rmw a cell prints
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
use hiroz::{
    BufferGroup, Builder, MessageTypeInfo, context::ZContext, ffi::subscriber::RawSubscriber,
    node::ZNode,
};
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

/// The `rclpy` publisher, with its output drained so a test can read it.
struct SeqPublisher {
    guard: ProcessGuard,
    output: OutputCapture,
}

fn spawn_seq_publisher(router: &TestRouter, topic: &str, subs: usize) -> SeqPublisher {
    let mut child = Command::new("python3")
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the rclpy publisher");
    let output = OutputCapture::start(&mut child);
    SeqPublisher {
        guard: ProcessGuard::new(child, "rclpy publisher"),
        output,
    }
}

impl SeqPublisher {
    /// Wait for the publisher to exit, then give its last samples two seconds to arrive. It
    /// returns what the publisher printed.
    fn finish(mut self) -> String {
        let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
        let mut finished_at: Option<Instant> = None;
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            let done = self
                .guard
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
        self.output.snapshot()
    }
}

/// The number of subscribers the publisher reported it matched, from its `PUB matched=N` line.
fn matched_subscribers(output: &str) -> Option<usize> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("PUB matched=")?.trim().parse().ok())
}

/// Records the sequence number of one raw sample. A sample of another length counts for nothing.
fn record(set: &Mutex<SeqSet>, sample: &[u8]) {
    if sample.len() == SAMPLE_LEN {
        set.lock().unwrap().record(&sample[16..]);
    }
}

/// What one cell shares: a router, a node and the keys of one topic.
struct Cell {
    router: TestRouter,
    _ctx: ZContext,
    node: ZNode,
    topic: String,
    base_key: String,
}

impl Cell {
    /// The cell, or `None` when this host has no buffer-aware rmw and the test may skip.
    fn start(name: &str, topic: &str) -> Option<Self> {
        assert!(
            check_ros2_available(),
            "{name}: ros2 is required: run this test where a released ROS 2 is installed"
        );
        if !buffer_rmw::buffer_aware_rmw_or_skip(name) {
            return None;
        }
        let hash = UInt8MultiArray::type_hash().to_rihs_string();
        let base_key = format!("0/{topic}/{}/{hash}", UInt8MultiArray::type_name());
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_router(&router).expect("ctx");
        let node = ctx
            .create_node(&format!("{topic}_node"))
            .build()
            .expect("node");
        Some(Self {
            router,
            _ctx: ctx,
            node,
            topic: format!("/{topic}"),
            base_key,
        })
    }

    /// A raw subscriber that records into `set`. `None` builds it the default way.
    fn subscribe(&self, group: Option<BufferGroup>, set: &Arc<Mutex<SeqSet>>) -> RawSubscriber {
        let (type_name, hash) = (
            UInt8MultiArray::type_name(),
            UInt8MultiArray::type_hash().to_rihs_string(),
        );
        let sink = set.clone();
        let callback = move |bytes: &[u8]| record(&sink, bytes);
        match group {
            None => self
                .node
                .create_raw_subscriber(&self.topic, type_name, &hash, callback),
            Some(group) => self.node.create_raw_subscriber_with_group(
                &self.topic,
                type_name,
                &hash,
                callback,
                None,
                group,
            ),
        }
        .expect("raw subscriber")
    }

    /// A plain session that counts the samples per key of this topic.
    fn key_counts(&self) -> KeyCounts {
        KeyCounts::new(self.router.endpoint(), &self.base_key)
    }
}

fn new_set() -> Arc<Mutex<SeqSet>> {
    Arc::new(Mutex::new(SeqSet::new(SIZE)))
}

/// Print the result of one arm.
fn report(name: &str, arm: &str, set: &Mutex<SeqSet>, keys: &KeyCounts) {
    let set = set.lock().unwrap();
    println!(
        "RESULT {name}/{arm}: received={} of {SAMPLES} duplicates={} group={} base={}",
        set.received_of(SAMPLES),
        set.duplicates,
        keys.group(),
        keys.base()
    );
}

/// The publisher wrote every sample on the group key and none on the base key, so it was in
/// buffer mode. This is the positive control for every "received nothing on the base key" claim.
fn assert_buffer_mode(name: &str, keys: &KeyCounts) {
    let (group, base) = (keys.group(), keys.base());
    assert!(
        group == SAMPLES as usize && base == 0,
        "{name}: the plain session read group {group} and base {base} of {SAMPLES}, so the \
         publisher is not in buffer mode"
    );
}

/// The subscriber received every sample once.
fn assert_complete(name: &str, arm: &str, set: &Mutex<SeqSet>) {
    let set = set.lock().unwrap();
    assert_eq!(
        set.received_of(SAMPLES),
        SAMPLES as usize,
        "{name}: {arm} received"
    );
    assert_eq!(set.duplicates, 0, "{name}: {arm} repeats");
}

/// A default raw subscriber and a `Cpu` raw subscriber each receive every sample a publisher in
/// buffer mode writes. An `Off` raw subscriber on the same topic receives none, which shows the
/// publisher writes the group key only. The payload carries a sequence number, so a repeat and a
/// gap show.
///
/// A released subscriber also matches the publisher here, and it can make the publisher write the
/// group key by itself. This cell therefore does not show that the publisher matched a raw
/// subscriber. `raw_default_subscriber_alone_matches_a_released_publisher` shows that.
#[test]
fn raw_default_subscriber_receives_from_a_released_publisher() {
    const NAME: &str = "raw_default_subscriber_receives_from_a_released_publisher";
    let Some(cell) = Cell::start(NAME, "rawcell") else {
        return;
    };
    let (cpu, default, off) = (new_set(), new_set(), new_set());
    let _cpu_sub = cell.subscribe(Some(BufferGroup::Cpu), &cpu);
    let _default_sub = cell.subscribe(None, &default);
    let _off_sub = cell.subscribe(Some(BufferGroup::Off), &off);

    let keys = cell.key_counts();
    wait_for_ready(Duration::from_secs(2));
    let _released = spawn_released_subscriber(&cell.router, &cell.topic);
    wait_for_ready(Duration::from_secs(2));
    // Three matched subscribers: the released one, the default raw one and the `Cpu` raw one. The
    // `Off` raw subscriber declares no token, so it does not match.
    let output = spawn_seq_publisher(&cell.router, &cell.topic, 3).finish();
    println!("{output}");

    for (arm, set) in [("cpu", &cpu), ("default", &default), ("off", &off)] {
        report(NAME, arm, set, &keys);
    }
    assert_buffer_mode(NAME, &keys);
    assert_eq!(
        off.lock().unwrap().received_of(SAMPLES),
        0,
        "{NAME}: the Off raw subscriber received from a publisher in buffer mode, so the \
         negative control is not valid"
    );
    assert_complete(NAME, "cpu", &cpu);
    assert_complete(NAME, "default", &default);
}

/// A default raw subscriber that is the only subscriber of the topic matches a publisher in
/// buffer mode and receives every sample once.
///
/// No released subscriber is present, so only the token of the raw subscriber can make the
/// publisher match and write the group key. The cell requires three things. The publisher reports
/// one matched subscriber. The plain session reads every sample on the group key and none on the
/// base key. The raw subscriber receives every sample, with no repeat.
#[test]
fn raw_default_subscriber_alone_matches_a_released_publisher() {
    const NAME: &str = "raw_default_subscriber_alone_matches_a_released_publisher";
    let Some(cell) = Cell::start(NAME, "rawsole") else {
        return;
    };
    let default = new_set();
    let _default_sub = cell.subscribe(None, &default);

    let keys = cell.key_counts();
    wait_for_ready(Duration::from_secs(2));
    let output = spawn_seq_publisher(&cell.router, &cell.topic, 1).finish();
    println!("{output}");

    report(NAME, "default", &default, &keys);
    assert_eq!(
        matched_subscribers(&output),
        Some(1),
        "{NAME}: the publisher did not report one matched subscriber: {output}"
    );
    assert_buffer_mode(NAME, &keys);
    assert_complete(NAME, "default", &default);
}
