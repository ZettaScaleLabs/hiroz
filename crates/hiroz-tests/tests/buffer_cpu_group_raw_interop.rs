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
//! One cell mixes a released subscriber with two raw subscribers. The other cell has the default
//! raw subscriber as the only subscriber, so a match of the publisher can only come from the token
//! of the raw subscriber. The publisher is the released `ros2 topic pub`, which waits for the
//! matched subscribers before it publishes. A `ros2 topic info -v` call, made while the publisher
//! runs, reads how many subscribers the graph shows, and the cell requires that number.
//!
//! The cells count totals, so a loss and a repeat that cancel out would pass.
//!
//! The cells call `buffer_rmw::buffer_aware_rmw_or_skip`. Without a buffer-aware rmw a cell prints
//! `Skipping <cell>: ...` and returns. Setting `HIROZ_REQUIRE_BUFFER_RMW` turns that skip into a
//! failure.

#![cfg(target_os = "linux")]
#![cfg(all(feature = "ros-interop", feature = "raw-sub-tests"))]

mod common;

use std::{
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use buffer_rmw::KeyCounts;
use common::{buffer_rmw, *};
use hiroz::{
    BufferGroup, Builder, MessageTypeInfo, context::ZContext, ffi::subscriber::RawSubscriber,
    node::ZNode,
};
use hiroz_msgs::std_msgs::UInt8MultiArray;

/// The messages the cell publishes, at 5 Hz. The `ros2 topic info -v` call runs while they are
/// published, so the run must outlast it.
const SAMPLES: u32 = 50;

/// The `data` bytes of each message.
const SIZE: usize = 64;

/// The length of one raw sample: the 4-byte CDR header, the length of the empty `layout.dim`,
/// `layout.data_offset`, the length of `data`, then `data`. `data` starts at byte 16.
const SAMPLE_LEN: usize = 16 + SIZE;

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

/// A released `ros2 topic pub`, with its output drained so a test can read it.
struct Publisher {
    guard: ProcessGuard,
    output: OutputCapture,
}

/// The released publisher. It waits for `subs` matched subscribers, publishes `SAMPLES` messages
/// of `SIZE` zero bytes at 5 Hz and exits. It gives up waiting after 20 seconds and exits with
/// an error, so a publisher that never matches fails the cell and does not hang it.
fn spawn_publisher(router: &TestRouter, topic: &str, subs: usize) -> Publisher {
    let data = format!("{{data: [{}]}}", vec!["0"; SIZE].join(", "));
    let mut child = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "--times",
            &SAMPLES.to_string(),
            "-r",
            "5",
            "-w",
            &subs.to_string(),
            "--max-wait-time-secs",
            "20",
            topic,
            "std_msgs/msg/UInt8MultiArray",
            &data,
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ros2 topic pub");
    let output = OutputCapture::start(&mut child);
    Publisher {
        guard: ProcessGuard::new(child, "ros2 topic pub"),
        output,
    }
}

/// The messages `ros2 topic pub` printed so far, one `publishing #<n>` line each.
fn published(output: &str) -> usize {
    output
        .lines()
        .filter(|l| l.starts_with("publishing #"))
        .count()
}

/// What the publisher did.
struct Outcome {
    /// Whether it exited with status 0. `None` means it did not exit in time.
    success: Option<bool>,
    output: String,
}

impl Publisher {
    /// Wait for the first published message. It returns false after `limit`.
    fn wait_publishing(&mut self, limit: Duration) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if published(&self.output.snapshot()) > 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Whether the publisher has not exited yet.
    fn running(&mut self) -> bool {
        self.guard
            .child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    }

    /// Wait for the publisher to exit, then give its last samples two seconds to arrive.
    fn finish(mut self) -> Outcome {
        let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
        let mut success = None;
        while success.is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            success = self
                .guard
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten())
                .map(|status| status.success());
        }
        std::thread::sleep(Duration::from_secs(2));
        Outcome {
            success,
            output: self.output.snapshot(),
        }
    }
}

/// The number of subscribers `ros2 topic info -v` lists for `topic`, from its
/// `Subscription count: N` line. The call asks the graph of a new node, so it needs the publisher
/// and the subscribers to be alive. It spins for 3 seconds to discover them and uses no daemon,
/// so no state outlives the call. It returns the number, or `None`, and the text of the call.
fn graph_subscribers(router: &TestRouter, topic: &str) -> (Option<usize>, String) {
    let out = Command::new("ros2")
        .args([
            "topic",
            "info",
            "-v",
            "--no-daemon",
            "--spin-time",
            "3",
            topic,
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdin(Stdio::null())
        .output()
        .expect("run ros2 topic info");
    let text = format!(
        "status {}\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let count = text
        .lines()
        .find_map(|l| l.strip_prefix("Subscription count:")?.trim().parse().ok());
    (count, text)
}

/// Counts one raw sample. A sample of another length counts for nothing.
fn record(count: &AtomicUsize, sample: &[u8]) {
    if sample.len() == SAMPLE_LEN {
        count.fetch_add(1, Ordering::Relaxed);
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
            .create_node(format!("{topic}_node"))
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

    /// A raw subscriber that counts into `count`. `None` builds it the default way.
    fn subscribe(&self, group: Option<BufferGroup>, count: &Arc<AtomicUsize>) -> RawSubscriber {
        let (type_name, hash) = (
            UInt8MultiArray::type_name(),
            UInt8MultiArray::type_hash().to_rihs_string(),
        );
        let sink = count.clone();
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

fn new_count() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

/// Print the result of one arm.
fn report(name: &str, arm: &str, count: &AtomicUsize, keys: &KeyCounts) {
    println!(
        "RESULT {name}/{arm}: received={} of {SAMPLES} group={} base={}",
        count.load(Ordering::Relaxed),
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

/// The publisher exited with status 0 after it published every message. A `ros2` that does not
/// start, a publisher that never matches and a publisher that publishes nothing all fail here.
fn assert_published(name: &str, outcome: &Outcome) {
    assert_eq!(
        outcome.success,
        Some(true),
        "{name}: ros2 topic pub did not exit with status 0: {}",
        outcome.output
    );
    assert_eq!(
        published(&outcome.output),
        SAMPLES as usize,
        "{name}: ros2 topic pub published a different number of messages: {}",
        outcome.output
    );
}

/// The subscriber received exactly as many samples as the publisher published. This counts
/// totals: a loss and a repeat that cancel out would pass.
fn assert_complete(name: &str, arm: &str, count: &AtomicUsize) {
    assert_eq!(
        count.load(Ordering::Relaxed),
        SAMPLES as usize,
        "{name}: {arm} received"
    );
}

/// A default raw subscriber receives every sample a publisher in buffer mode writes. An `Off` raw
/// subscriber on the same topic receives none, which shows the publisher writes the group key only.
/// The cell counts totals, so it cannot say which message is lost.
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
    let (default, off) = (new_count(), new_count());
    let _default_sub = cell.subscribe(None, &default);
    let _off_sub = cell.subscribe(Some(BufferGroup::Off), &off);

    let keys = cell.key_counts();
    wait_for_ready(Duration::from_secs(2));
    let _released = spawn_released_subscriber(&cell.router, &cell.topic);
    wait_for_ready(Duration::from_secs(2));
    // Two matched subscribers: the released one and the default raw one. The `Off` raw subscriber
    // declares no token, so it does not match.
    let mut publisher = spawn_publisher(&cell.router, &cell.topic, 2);
    assert!(
        publisher.wait_publishing(Duration::from_secs(30)),
        "{NAME}: ros2 topic pub published nothing"
    );
    let (graph, graph_text) = graph_subscribers(&cell.router, &cell.topic);
    assert!(
        publisher.running(),
        "{NAME}: the publisher exited before the graph call returned: raise SAMPLES"
    );
    let outcome = publisher.finish();
    println!("{}", outcome.output);

    for (arm, count) in [("default", &default), ("off", &off)] {
        report(NAME, arm, count, &keys);
    }
    assert_published(NAME, &outcome);
    assert_eq!(
        graph,
        Some(2),
        "{NAME}: the graph did not show two subscribers: {graph_text}"
    );
    assert_buffer_mode(NAME, &keys);
    assert_eq!(
        off.load(Ordering::Relaxed),
        0,
        "{NAME}: the Off raw subscriber received from a publisher in buffer mode, so the \
         negative control is not valid"
    );
    assert_complete(NAME, "default", &default);
}

/// A default raw subscriber that is the only subscriber of the topic matches a publisher in
/// buffer mode and receives every sample.
///
/// No released subscriber is present, so only the token of the raw subscriber can make the
/// publisher match and write the group key. The publisher waits for one matched subscriber before
/// it publishes, and the graph call shows one subscriber. The plain session reads every sample on
/// the group key and none on the base key. The raw subscriber receives as many samples as the
/// publisher published. The cell counts totals, so it cannot say which message is lost.
#[test]
fn raw_default_subscriber_alone_matches_a_released_publisher() {
    const NAME: &str = "raw_default_subscriber_alone_matches_a_released_publisher";
    let Some(cell) = Cell::start(NAME, "rawsole") else {
        return;
    };
    let default = new_count();
    let _default_sub = cell.subscribe(None, &default);

    let keys = cell.key_counts();
    wait_for_ready(Duration::from_secs(2));
    let mut publisher = spawn_publisher(&cell.router, &cell.topic, 1);
    assert!(
        publisher.wait_publishing(Duration::from_secs(30)),
        "{NAME}: ros2 topic pub published nothing"
    );
    let (graph, graph_text) = graph_subscribers(&cell.router, &cell.topic);
    assert!(
        publisher.running(),
        "{NAME}: the publisher exited before the graph call returned: raise SAMPLES"
    );
    let outcome = publisher.finish();
    println!("{}", outcome.output);

    report(NAME, "default", &default, &keys);
    assert_published(NAME, &outcome);
    assert_eq!(
        graph,
        Some(1),
        "{NAME}: the graph did not show one subscriber: {graph_text}"
    );
    assert_buffer_mode(NAME, &keys);
    assert_complete(NAME, "default", &default);
}
