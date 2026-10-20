//! A hiroz subscriber must receive from a released `rmw_zenoh_cpp` publisher in buffer mode.
//!
//! A buffer-aware `rmw_zenoh_cpp` writes a message with an unbounded `uint8[]` field only on
//! `<topic key>/_buf_cpu`, and only when a matched subscriber announces buffer support. A plain
//! Zenoh session counts the samples per key, so a test checks that the publisher really is in
//! buffer mode: the group key carries the samples and the base key none.
//!
//! Tests that need a buffer-aware `rmw_zenoh_cpp` call `buffer_rmw::buffer_aware_rmw_or_skip`.
//! Without one they print `Skipping <test>: ...` and return. Setting `HIROZ_REQUIRE_BUFFER_RMW`
//! turns that skip into a failure. The other tests run on every distro.

#![cfg(target_os = "linux")]
#![cfg(feature = "ros-interop")]

mod common;

use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use buffer_rmw::{GROUP_SUFFIX, KeyCounts, SeqSet, client_session};
use common::{buffer_rmw, *};
use hiroz::{BufferGroup, Builder, MessageTypeInfo};
use hiroz_msgs::{sensor_msgs::PointCloud2, std_msgs::UInt8MultiArray};
use serde::{Deserialize, Serialize};
use zenoh_buffers::buffer::SplitBuffer;

/// `std_msgs/UInt8MultiArray` on the wire, without a message schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Dim {
    label: String,
    size: u32,
    stride: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Layout {
    dim: Vec<Dim>,
    data_offset: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Blob {
    layout: Layout,
    data: Vec<u8>,
}
impl MessageTypeInfo for Blob {
    fn type_name() -> &'static str {
        UInt8MultiArray::type_name()
    }
    fn type_hash() -> hiroz::TypeHash {
        UInt8MultiArray::type_hash()
    }
}
impl hiroz::ros_msg::WithTypeInfo for Blob {}
impl hiroz::msg::ZMessage for Blob {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Blob>;
}

/// `sensor_msgs/PointCloud2` on the wire, without a message schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stamp {
    sec: i32,
    nanosec: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Header {
    stamp: Stamp,
    frame_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Field {
    name: String,
    offset: u32,
    datatype: u8,
    count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CloudBlob {
    header: Header,
    height: u32,
    width: u32,
    fields: Vec<Field>,
    is_bigendian: bool,
    point_step: u32,
    row_step: u32,
    data: Vec<u8>,
    is_dense: bool,
}
impl MessageTypeInfo for CloudBlob {
    fn type_name() -> &'static str {
        PointCloud2::type_name()
    }
    fn type_hash() -> hiroz::TypeHash {
        PointCloud2::type_hash()
    }
}
impl hiroz::ros_msg::WithTypeInfo for CloudBlob {}
impl hiroz::msg::ZMessage for CloudBlob {
    type Serdes = hiroz::msg::SerdeCdrSerdes<CloudBlob>;
}

/// The messages one test publishes.
const SAMPLES: u32 = 30;

/// The base key of a topic for the domain 0, as `rmw_zenoh_cpp` writes it.
fn base_key(topic: &str, type_name: &str, hash: &hiroz::TypeHash) -> String {
    format!(
        "0/{}/{}/{}",
        topic.trim_start_matches('/'),
        type_name,
        hash.to_rihs_string()
    )
}

/// The base key a released publisher writes for the topic of `entity`, in the key format of the
/// distro this binary is built for. The type-hash segment is `RIHS01_...` on Jazzy and later and
/// `TypeHashNotSupported` on Humble, so a test that reads the base key must not spell the hash.
fn entity_base_key(entity: &hiroz_protocol::EndpointEntity) -> String {
    hiroz_protocol::KeyExprFormat::default()
        .topic_key_expr(entity)
        .expect("base key")
        .as_str()
        .to_owned()
}

/// A released `rclpy` publisher of a message with a `data` field of `size` bytes. Message number
/// `seq` carries `seq` as 4 little-endian bytes in `data`. It waits for `subs` matched
/// subscribers, then publishes `n` messages at 5 Hz.
const SEQ_PUBLISHER: &str = r#"
import sys, time, rclpy
kind, topic, n, size, subs = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
rclpy.init()
node = rclpy.create_node('seq_pub')
if kind == 'uint8':
    from std_msgs.msg import UInt8MultiArray as Msg
else:
    from sensor_msgs.msg import PointCloud2 as Msg
pub = node.create_publisher(Msg, topic, 10)
t0 = time.time()
while pub.get_subscription_count() < subs and time.time() - t0 < 20:
    rclpy.spin_once(node, timeout_sec=0.1)
print('PUB matched=%d' % pub.get_subscription_count(), flush=True)
time.sleep(1.0)
for seq in range(n):
    data = bytearray(size)
    data[0:4] = seq.to_bytes(4, 'little')
    m = Msg()
    if kind != 'uint8':
        m.header.frame_id = 'cloud'
        m.height = 1
        m.width = size
        m.point_step = 1
        m.row_step = size
        m.is_dense = True
    m.data = list(data)
    pub.publish(m)
    rclpy.spin_once(node, timeout_sec=0.2)
print('PUB published=%d' % n, flush=True)
time.sleep(1.0)
node.destroy_node()
rclpy.shutdown()
"#;

fn spawn_seq_publisher(
    router: &TestRouter,
    kind: &str,
    topic: &str,
    size: usize,
    subs: usize,
) -> ProcessGuard {
    let child = Command::new("python3")
        .args([
            "-c",
            SEQ_PUBLISHER,
            kind,
            topic,
            &SAMPLES.to_string(),
            &size.to_string(),
            &subs.to_string(),
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the rclpy publisher");
    ProcessGuard::new(child, "rclpy seq publisher")
}

/// A released `ros2 topic echo` subscriber. It announces buffer support whether or not hiroz
/// joins, and a publisher only writes the group key when at least one matched subscriber
/// announces it.
fn spawn_released_subscriber(router: &TestRouter, kind: &str, topic: &str) -> ProcessGuard {
    use std::os::unix::process::CommandExt;
    let ros_type = match kind {
        "uint8" => "std_msgs/msg/UInt8MultiArray",
        _ => "sensor_msgs/msg/PointCloud2",
    };
    let child = Command::new("ros2")
        .args(["topic", "echo", topic, ros_type, "--no-arr"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .expect("spawn the released subscriber");
    ProcessGuard::new(child, "released subscriber")
}

/// Calls `drain` every 10 ms until the publisher has exited and its last samples had two
/// seconds to arrive.
fn drain_until_published(guard: &mut ProcessGuard, mut drain: impl FnMut(Duration)) {
    let poll = Duration::from_millis(10);
    let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
    let mut finished_at: Option<Instant> = None;
    while Instant::now() < end {
        drain(poll);
        let done = guard
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
}

/// What one subscriber received.
struct Arm {
    name: &'static str,
    received: usize,
    duplicates: usize,
}

fn arm(name: &'static str, set: &SeqSet) -> Arm {
    Arm {
        name,
        received: set.received_of(SAMPLES),
        duplicates: set.duplicates,
    }
}

/// What one payload kind read: the key counts and the arms.
struct KindRun {
    kind: &'static str,
    group: usize,
    base: usize,
    arms: Vec<Arm>,
}

/// Runs four joined subscribers of one payload kind against one released publisher: a typed one,
/// a no-schema one, a dynamic one that gets its schema from the message type, and a cache. A typed
/// message holds `data` as a `ZBuf`, a no-schema message as a `Vec<u8>` and a dynamic message as
/// bytes. The cache counts each message its stamp extractor sees, so a repeat shows although a
/// cache keeps one entry per stamp.
macro_rules! run_kind {
    ($kind:expr, $topic:expr, $size:expr, $M:ty, $B:ty) => {{
        let topic: &str = $topic;
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_router(&router).expect("ctx");
        let node = ctx.create_node("cell_node").build().expect("node");
        let typed = node
            .create_sub::<$M>(topic)
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap();
        let blob = node
            .create_sub::<$B>(topic)
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap();
        let dynamic = node
            .create_dyn_sub(topic, <$M>::message_schema().expect("message schema"))
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap();
        let cached = Arc::new(Mutex::new(SeqSet::new($size)));
        let sink = cached.clone();
        let _cache = node
            .create_cache::<$M>(topic, 4 * SAMPLES as usize)
            .with_stamp(move |m: &$M| {
                let data = m.data.contiguous();
                sink.lock().unwrap().record(&data);
                // A stamp per sequence number, so the cache never merges two messages.
                let seq = data
                    .get(..4)
                    .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                std::time::UNIX_EPOCH + Duration::from_millis(seq as u64)
            })
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap();
        let base = base_key(topic, <$M>::type_name(), &<$M>::type_hash());
        let keys = KeyCounts::new(router.endpoint(), &base);
        wait_for_ready(Duration::from_secs(2));

        let _released = spawn_released_subscriber(&router, $kind, topic);
        wait_for_ready(Duration::from_secs(2));
        let mut guard = spawn_seq_publisher(&router, $kind, topic, $size, 5);
        let (mut got_typed, mut got_blob, mut got_dyn) =
            (SeqSet::new($size), SeqSet::new($size), SeqSet::new($size));
        drain_until_published(&mut guard, |poll| {
            if let Ok(m) = typed.recv_timeout(poll) {
                got_typed.record(&m.data.contiguous());
            }
            if let Ok(m) = blob.recv_timeout(poll) {
                got_blob.record(&m.data);
            }
            if let Ok(m) = dynamic.recv_timeout(poll) {
                let data = m.get_dynamic("data").expect("data field");
                got_dyn.record(data.as_bytes().expect("data bytes"));
            }
        });
        let got_cache = cached.lock().unwrap();
        KindRun {
            kind: $kind,
            group: keys.group(),
            base: keys.base(),
            arms: vec![
                arm("typed", &got_typed),
                arm("no_schema", &got_blob),
                arm("dynamic", &got_dyn),
                arm("cache", &got_cache),
            ],
        }
    }};
}

/// A joined typed, no-schema, dynamic and cache subscriber each receive every sample from a
/// released publisher in buffer mode, for `UInt8MultiArray` and for `PointCloud2`. A plain session counts
/// the samples per key: the group key carries all of them and the base key none, so the test
/// cannot pass against a publisher that is not in buffer mode. The payload carries a sequence
/// number, so a repeat and a gap show.
#[test]
fn joined_subscribers_receive_from_a_released_publisher() {
    const NAME: &str = "joined_subscribers_receive_from_a_released_publisher";
    assert!(
        check_ros2_available(),
        "{NAME}: ros2 is required: run this test where a released ROS 2 is installed"
    );
    if !buffer_rmw::buffer_aware_rmw_or_skip(NAME) {
        return;
    }
    let runs = [
        run_kind!("uint8", "/bufcell", 64, UInt8MultiArray, Blob),
        run_kind!("cloud", "/bufcloud", 65536, PointCloud2, CloudBlob),
    ];
    for run in &runs {
        for arm in &run.arms {
            println!(
                "RESULT {NAME}/{}/{}: received={} of {SAMPLES} duplicates={} group={} base={}",
                run.kind, arm.name, arm.received, arm.duplicates, run.group, run.base
            );
        }
    }
    for run in &runs {
        assert!(
            run.group == SAMPLES as usize && run.base == 0,
            "{NAME}: {}: the plain session read group {} and base {} of {SAMPLES}, so the \
             publisher is not in buffer mode",
            run.kind,
            run.group,
            run.base
        );
        for arm in &run.arms {
            assert_eq!(
                arm.received, SAMPLES as usize,
                "{NAME}: {}/{} received",
                run.kind, arm.name
            );
            assert_eq!(
                arm.duplicates, 0,
                "{NAME}: {}/{} repeats",
                run.kind, arm.name
            );
        }
    }
}

/// The liveliness token of the one entity on `topic`, once it is visible.
fn token_for(name: &str, session: &zenoh::Session, topic: &str) -> String {
    use zenoh::Wait;
    let needle = format!("/%{topic}/");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let replies = session
            .liveliness()
            .get("@ros2_lv/**")
            .timeout(Duration::from_secs(2))
            .wait()
            .expect("liveliness get");
        let mut hits = Vec::new();
        while let Ok(reply) = replies.recv() {
            if let Ok(sample) = reply.result() {
                let key = sample.key_expr().to_string();
                if key.contains(&needle) {
                    hits.push(key);
                }
            }
        }
        if hits.len() == 1 {
            return hits.remove(0);
        }
        assert!(
            Instant::now() < deadline,
            "{name}: {} tokens for {topic}: {hits:?}",
            hits.len()
        );
        thread::sleep(Duration::from_millis(200));
    }
}

/// A released `ros2` CLI lists a joined subscriber. The token ends in `backends:cpu:`, the field
/// an older parser must tolerate, and a subscriber that never joined has none.
#[test]
fn old_parsers_list_a_joined_subscriber() {
    const NAME: &str = "old_parsers_list_a_joined_subscriber";
    assert!(
        check_ros2_available(),
        "{NAME}: ros2 is required: run this test where a released ROS 2 is installed"
    );
    let router = TestRouter::new();
    let plain = client_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("oldp_node").build().expect("node");
    let _joined = node
        .create_sub::<UInt8MultiArray>("/oldp")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();

    let token = token_for(NAME, &plain, "oldp");
    assert!(
        token.ends_with("/backends:cpu:"),
        "{NAME}: the token does not end in backends:cpu:: {token}"
    );

    // Discovery needs a moment, so read the listing until the subscriber shows or time is out.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut text;
    loop {
        let out = Command::new("ros2")
            .args([
                "topic",
                "info",
                "/oldp",
                "-v",
                "--no-daemon",
                "--spin-time",
                "3",
            ])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
            .output()
            .expect("run ros2 topic info");
        text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if text.contains("Subscription count: 1") || Instant::now() >= deadline {
            break;
        }
    }
    assert!(
        text.contains("Subscription count: 1"),
        "{NAME}: the CLI lists no subscription: {text}"
    );
    assert!(
        text.contains("oldp_node") && text.contains("SUBSCRIPTION"),
        "{NAME}: the CLI does not name the hiroz node as a subscription endpoint: {text}"
    );
}

#[cfg(not(any(feature = "kilted", feature = "lyrical")))]
use hiroz_msgs::action_tutorials_interfaces::{FibonacciGoal, action::Fibonacci};
#[cfg(any(feature = "kilted", feature = "lyrical"))]
use hiroz_msgs::example_interfaces::{FibonacciGoal, action::Fibonacci};

/// The hiroz action client receives each feedback message of the released Fibonacci server once.
/// The server of `action_tutorials_cpp` publishes the whole sequence after each step, so
/// feedback number `i` holds `i + 3` numbers: the received lengths must be exactly
/// `3, 4, ..., order + 1`. The action client has no `BufferGroup` switch, so it stays out of the
/// group. The group key carries no sample, as feedback is not a buffer topic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn action_feedback_arrives_once_each() {
    use std::{
        os::unix::process::CommandExt,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use zenoh::Wait;

    const NAME: &str = "action_feedback_arrives_once_each";
    const ORDER: i32 = 6;
    assert!(
        check_ros2_available() && check_action_tutorials_cpp_available(),
        "{NAME}: ros2 and action_tutorials_cpp are required: run this test where they are installed"
    );
    let router = TestRouter::new();
    let plain = client_session(router.endpoint());
    let group = Arc::new(AtomicUsize::new(0));
    let seen = group.clone();
    let _group_sub = plain
        .declare_subscriber(format!("0/**/{GROUP_SUFFIX}"))
        .callback(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .wait()
        .expect("group subscriber");

    let server = Command::new("ros2")
        .args(["run", "action_tutorials_cpp", "fibonacci_action_server"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn the Fibonacci action server");
    let _server_guard = ProcessGuard::new(server, "fibonacci action server");
    wait_for_ros_node("fibonacci_action_server", &router, Duration::from_secs(15));

    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("fib_client").build().expect("node");
    let client = node
        .create_action_client::<Fibonacci>("fibonacci")
        .build()
        .expect("client");

    // The goal call waits for discovery of the server: retry it until the server accepts.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut goal_handle = loop {
        match tokio::time::timeout(
            Duration::from_secs(5),
            client.send_goal(FibonacciGoal { order: ORDER }),
        )
        .await
        {
            Ok(Ok(handle)) => break handle,
            _ if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(500)).await,
            Ok(Err(e)) => panic!("{NAME}: the server never accepted: {e}"),
            Err(_) => panic!("{NAME}: the server never accepted: the goal call timed out"),
        }
    };

    let lengths = Arc::new(Mutex::new(Vec::<usize>::new()));
    let mut feedback = goal_handle.feedback().expect("feedback stream");
    let sink = lengths.clone();
    tokio::spawn(async move {
        while let Some(fb) = feedback.recv().await {
            #[cfg(any(feature = "kilted", feature = "lyrical"))]
            let n = fb.sequence.len();
            #[cfg(not(any(feature = "kilted", feature = "lyrical")))]
            let n = fb.partial_sequence.len();
            sink.lock().unwrap().push(n);
        }
    });
    tokio::time::timeout(Duration::from_secs(30), goal_handle.result())
        .await
        .expect("the goal finishes")
        .expect("result");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let got = lengths.lock().unwrap().clone();
    let expected: Vec<usize> = (3..=(ORDER as usize + 1)).collect();
    assert_eq!(
        got, expected,
        "{NAME}: feedback lengths, each once in order"
    );
    assert_eq!(
        group.load(Ordering::SeqCst),
        0,
        "{NAME}: samples on the group key"
    );
}

/// A topic that is not a buffer topic delivers each sample once to a subscriber that joined the
/// group: its count equals the base key count, and the group key carries nothing.
#[test]
fn non_buffer_topic_delivers_each_sample_once_with_the_group_joined() {
    const NAME: &str = "non_buffer_topic_delivers_each_sample_once_with_the_group_joined";
    assert!(
        check_ros2_available(),
        "{NAME}: ros2 is required: run this test where a released ROS 2 is installed"
    );
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("plaincell_node").build().expect("node");
    let joined = node
        .create_sub::<hiroz_msgs::std_msgs::Float64MultiArray>("/plaincell")
        .with_buffer_group(hiroz::BufferGroup::Cpu)
        .build()
        .unwrap();
    let keys = KeyCounts::new(router.endpoint(), &entity_base_key(joined.entity()));
    wait_for_ready(Duration::from_secs(2));

    const PUBLISHED: usize = 30;
    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "/plaincell",
            "std_msgs/msg/Float64MultiArray",
            "{data: [1.0, 2.0]}",
            "--rate",
            "5",
            "--times",
            &PUBLISHED.to_string(),
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let mut received = 0;
    let end = Instant::now() + Duration::from_secs(PUBLISHED as u64 / 5 + 8);
    while Instant::now() < end && keys.base() < PUBLISHED {
        received += joined.recv_timeout(Duration::from_millis(100)).is_ok() as usize;
    }
    while joined.recv_timeout(Duration::from_millis(500)).is_ok() {
        received += 1;
    }

    println!(
        "RESULT {NAME}: received={received} of {PUBLISHED} group={} base={}",
        keys.group(),
        keys.base()
    );
    // The base key carries the samples, so the topic is not a buffer topic.
    assert!(
        keys.base() >= PUBLISHED * 2 / 3,
        "{NAME}: the base key carried {} of {PUBLISHED}",
        keys.base()
    );
    assert_eq!(keys.group(), 0, "{NAME}: group key count");
    assert_eq!(
        received,
        keys.base(),
        "{NAME}: received against the base key"
    );
}

/// What one matched set read: the key counts and each subscriber.
struct MixRun {
    /// Every key under the base key that carried samples, relative to it, with its count.
    keys: Vec<(String, usize)>,
    joined: Arm,
    off: Option<Arm>,
}

/// One matched set. A joined hiroz subscriber reads a topic that a released `rclpy` publisher
/// writes. `with_off` adds a hiroz subscriber set to `BufferGroup::Off`, whose token has no
/// `backends:` field, so the publisher sees a subscriber that is not buffer-aware.
/// `with_released` adds a released buffer-aware subscriber.
fn mixed_run(topic: &str, with_off: bool, with_released: bool) -> MixRun {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("mix_node").build().expect("node");
    let joined = node
        .create_sub::<UInt8MultiArray>(topic)
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let off = with_off.then(|| {
        node.create_sub::<UInt8MultiArray>(topic)
            .with_buffer_group(hiroz::BufferGroup::Off)
            .build()
            .unwrap()
    });
    let base = base_key(
        topic,
        UInt8MultiArray::type_name(),
        &UInt8MultiArray::type_hash(),
    );
    let keys = KeyCounts::new(router.endpoint(), &base);
    wait_for_ready(Duration::from_secs(2));
    let _released = with_released.then(|| spawn_released_subscriber(&router, "uint8", topic));
    wait_for_ready(Duration::from_secs(2));
    let matched = 1 + with_off as usize + with_released as usize;
    let mut guard = spawn_seq_publisher(&router, "uint8", topic, 64, matched);

    let (mut got_joined, mut got_off) = (SeqSet::new(64), SeqSet::new(64));
    drain_until_published(&mut guard, |poll| {
        if let Ok(m) = joined.recv_timeout(poll) {
            got_joined.record(&m.data.contiguous());
        }
        if let Some(off) = &off
            && let Ok(m) = off.recv_timeout(poll)
        {
            got_off.record(&m.data.contiguous());
        }
    });
    MixRun {
        keys: keys.all(),
        joined: arm("joined", &got_joined),
        off: off.is_some().then(|| arm("off", &got_off)),
    }
}

/// A buffer-aware publisher writes each sample once, on one key, whatever mix of subscribers is
/// matched, and a joined hiroz subscriber delivers each sample once. The test runs three matched
/// sets: the joined subscriber with a released one, with an `Off` subscriber, and with both. A
/// plain Zenoh subscriber on `<topic key>/**` counts the samples per key. In every set the group
/// key carries all samples and the base key none. The test prints every key that carried samples
/// and asserts only on those two counts. The `Off` subscriber reads the base key only, so it
/// receives nothing.
#[test]
fn mixed_matched_set_delivers_each_sample_once() {
    const NAME: &str = "mixed_matched_set_delivers_each_sample_once";
    assert!(
        check_ros2_available(),
        "{NAME}: ros2 is required: run this test where a released ROS 2 is installed"
    );
    if !buffer_rmw::buffer_aware_rmw_or_skip(NAME) {
        return;
    }
    let runs = [
        ("released", mixed_run("/mixreleased", false, true)),
        ("off", mixed_run("/mixoff", true, false)),
        ("off_and_released", mixed_run("/mixboth", true, true)),
    ];
    for (set, run) in &runs {
        println!(
            "RESULT {NAME}/{set}: joined received={} of {SAMPLES} duplicates={} off received={} keys={:?}",
            run.joined.received,
            run.joined.duplicates,
            run.off
                .as_ref()
                .map_or("none".into(), |a| a.received.to_string()),
            run.keys
        );
    }
    for (set, run) in &runs {
        let count = |key: &str| {
            run.keys
                .iter()
                .find(|(k, _)| k == key)
                .map_or(0, |(_, n)| *n)
        };
        assert_eq!(
            count(GROUP_SUFFIX),
            SAMPLES as usize,
            "{NAME}: {set}: samples on the group key"
        );
        assert_eq!(count(""), 0, "{NAME}: {set}: samples on the base key");
        assert_eq!(
            run.joined.received, SAMPLES as usize,
            "{NAME}: {set}: joined received"
        );
        assert_eq!(run.joined.duplicates, 0, "{NAME}: {set}: joined repeats");
        if let Some(off) = &run.off {
            // A subscriber with no `backends:` field gets nothing from a buffer-aware publisher.
            // This changes when the upstream publisher serves such subscribers on the base key.
            assert_eq!(off.received, 0, "{NAME}: {set}: off received");
        }
    }
}
