//! A hiroz subscriber must receive from a released `rmw_zenoh` publisher in buffer mode.
//!
//! The publisher is a released Lyrical `rmw_zenoh_cpp`. Lyrical writes a message with an unbounded
//! `uint8[]` field only on `<topic key>/_buf_cpu` to a subscriber that joined the CPU buffer group.
//! A plain Zenoh session counts the samples on both keys, so a cell knows the publisher is in
//! buffer mode without depending on a subscriber starving: the group key must carry the samples
//! and the base key none. A cell that reads a base key above 0 on Lyrical proves nothing.
//!
//! On an older distro the publisher has no buffer mode. The cells there are a no-regression check:
//! every sample arrives on the base key.

#![cfg(target_os = "linux")]
#![cfg(feature = "ros-interop")]

mod common;

use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use common::{t1_harness, *};
use hiroz::{Builder, MessageTypeInfo};
use hiroz_msgs::{sensor_msgs::PointCloud2, std_msgs::UInt8MultiArray};
use serde::{Deserialize, Serialize};
use t1_harness::{ModeDetector, SeqCounter};
use zenoh::Wait;
use zenoh_buffers::buffer::SplitBuffer;

/// `std_msgs/UInt8MultiArray` on the wire, without a message schema: the no-schema case.
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

/// `sensor_msgs/PointCloud2` on the wire, without a message schema: the no-schema case of the
/// payload of public report 369.
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

/// The messages one cell publishes per kind.
const SAMPLES: u32 = 30;

/// The Lyrical publisher is the only one with a buffer mode. Any other distro is a no-regression
/// cell: the samples arrive on the base key.
fn distro_has_buffer_mode() -> bool {
    std::env::var("ROS_DISTRO").is_ok_and(|d| d == "lyrical")
}

/// The base key of a topic for the domain 0, as `rmw_zenoh_cpp` writes it.
fn base_key(topic: &str, type_name: &str, hash: &hiroz::TypeHash) -> String {
    format!(
        "0/{}/{}/{}",
        topic.trim_start_matches('/'),
        type_name,
        hash.to_rihs_string()
    )
}

/// A released `rclpy` publisher of a message with a `data` field of `size` bytes. Message number
/// `seq` carries `seq` as 4 little-endian bytes in `data` and the sentinel 0xA5 as the last byte.
/// It waits for `subs` matched subscribers, then publishes `n` messages at 5 Hz.
const SEQ_PUBLISHER: &str = r#"
import sys, time, rclpy
kind, topic, n, size, subs = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
rclpy.init()
node = rclpy.create_node('t1_seq_pub')
if kind == 'uint8':
    from std_msgs.msg import UInt8MultiArray as Msg
else:
    from sensor_msgs.msg import PointCloud2 as Msg
pub = node.create_publisher(Msg, topic, 10)
t0 = time.time()
while pub.get_subscription_count() < subs and time.time() - t0 < 20:
    rclpy.spin_once(node, timeout_sec=0.1)
print('T1PUB matched=%d' % pub.get_subscription_count(), flush=True)
time.sleep(1.0)
for seq in range(n):
    data = bytearray(size)
    data[0:4] = seq.to_bytes(4, 'little')
    data[size - 1] = 0xA5
    m = Msg()
    if kind != 'uint8':
        m.header.frame_id = 't1'
        m.height = 1
        m.width = size
        m.point_step = 1
        m.row_step = size
        m.is_dense = True
    m.data = list(data)
    pub.publish(m)
    rclpy.spin_once(node, timeout_sec=0.2)
print('T1PUB published=%d' % n, flush=True)
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

/// A released subscriber of the topic, the anchor of the mode. It is the one matched subscriber
/// that announces buffer support whether or not hiroz joins, so the publisher is in buffer mode
/// in every run and the detector reads the group key. Measured at the S0 tip: with the four hiroz
/// subscribers alone and no anchor, the detector read 0 on both keys, so the cell proved nothing.
fn spawn_anchor(router: &TestRouter, kind: &str, topic: &str) -> ProcessGuard {
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
        .expect("spawn the released anchor subscriber");
    ProcessGuard::new(child, "released anchor subscriber")
}

/// The `data` bytes of a dynamic message: `Bytes`, or an array of `uint8`.
fn dynamic_bytes(msg: &hiroz::dynamic::DynamicMessage) -> Vec<u8> {
    let value = msg.get_dynamic("data").expect("data field");
    match value.as_bytes() {
        Some(bytes) => bytes.to_vec(),
        None => value
            .as_array()
            .expect("data is bytes or an array")
            .iter()
            .filter_map(|v| v.as_u8())
            .collect(),
    }
}

/// What one arm received of one kind.
struct Arm {
    name: &'static str,
    received: usize,
    duplicates: usize,
    corrupt: usize,
}

/// What one kind of payload read: the detector counts and the arms.
struct KindRun {
    kind: &'static str,
    group: usize,
    base: usize,
    arms: Vec<Arm>,
}

/// Runs the four non-raw subscriber arms of one payload kind against one released publisher.
/// A typed message holds `data` as a `ZBuf` and a no-schema message as a `Vec<u8>`.
macro_rules! run_kind {
    ($kind:expr, $py:expr, $topic:expr, $size:expr, $M:ty, $B:ty) => {{
        let topic: &str = $topic;
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_router(&router).expect("ctx");
        let node = ctx.create_node("t1cell_node").build().expect("node");
        let typed = node.create_sub::<$M>(topic).build().unwrap();
        let blob = node.create_sub::<$B>(topic).build().unwrap();
        let dynamic = node
            .create_dyn_sub(topic, <$M>::message_schema().unwrap())
            .build()
            .unwrap();
        let cache = node
            .create_cache::<$M>(topic, 4 * SAMPLES as usize)
            .build()
            .unwrap();
        let base = base_key(topic, <$M>::type_name(), &<$M>::type_hash());
        let detector = ModeDetector::new(router.endpoint(), &base);
        wait_for_ready(Duration::from_secs(2));

        let _anchor = spawn_anchor(&router, $py, topic);
        wait_for_ready(Duration::from_secs(2));
        let mut guard = spawn_seq_publisher(&router, $py, topic, $size, 5);
        let (mut c_typed, mut c_blob, mut c_dyn) =
            (SeqCounter::new(), SeqCounter::new(), SeqCounter::new());
        let poll = Duration::from_millis(10);
        let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
        let mut finished_at: Option<Instant> = None;
        while Instant::now() < end {
            if let Ok(m) = typed.recv_timeout(poll) {
                c_typed.record(&m.data.contiguous());
            }
            if let Ok(m) = blob.recv_timeout(poll) {
                c_blob.record(&m.data);
            }
            if let Ok(m) = dynamic.recv_timeout(poll) {
                c_dyn.record(&dynamic_bytes(&m));
            }
            let done = guard
                .child
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_some());
            if done && finished_at.is_none() {
                finished_at = Some(Instant::now());
            }
            // The publisher has exited: give the last samples two seconds, then stop.
            if finished_at.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                break;
            }
        }
        let mut c_cache = SeqCounter::new();
        let horizon = SystemTime::now() + Duration::from_secs(3600);
        for m in cache.get_interval(UNIX_EPOCH, horizon) {
            c_cache.record(&m.data.contiguous());
        }

        let arm = |name: &'static str, c: &SeqCounter| Arm {
            name,
            received: c.received_of(SAMPLES),
            duplicates: c.duplicates(),
            corrupt: c.corrupt(),
        };
        let reading = detector.read();
        KindRun {
            kind: $kind,
            group: reading.group,
            base: reading.base,
            arms: vec![
                arm("typed", &c_typed),
                arm("no_schema", &c_blob),
                arm("dynamic", &c_dyn),
                arm("cache", &c_cache),
            ],
        }
    }};
}

/// Cycle 2.12: when a Lyrical publisher is in buffer mode, the typed, the no-schema, the dynamic
/// and the cache subscriber each receive every sample, for `UInt8MultiArray` and for
/// `PointCloud2`. The plain session is the mode detector. Each arm counts by sequence number and
/// sentinel, so a repeat, a gap and a truncated payload are all visible.
///
/// The test prints one `T1ARM` line per arm and one `T1RED` line: the least any arm received, with
/// the detector counts of the worst kind. The line is the one J-RED-T1 reads.
#[test]
fn joined_subscribers_receive_from_a_released_publisher() {
    const NAME: &str = "joined_subscribers_receive_from_a_released_publisher";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a released ROS 2 is installed"
    );
    t1_harness::require_buffer_aware_rmw(NAME);
    let runs = vec![
        run_kind!("uint8", "uint8", "/bufcell", 64, UInt8MultiArray, Blob),
        run_kind!("cloud", "cloud", "/bufcloud", 65536, PointCloud2, CloudBlob),
    ];

    let mut least = SAMPLES as usize;
    for run in &runs {
        println!(
            "T1MODE {NAME}/{} GROUP={} BASE={}",
            run.kind, run.group, run.base
        );
        for arm in &run.arms {
            println!(
                "T1ARM {NAME}/{}/{} RECEIVED={} OF={SAMPLES} DUPLICATES={} CORRUPT={}",
                run.kind, arm.name, arm.received, arm.duplicates, arm.corrupt
            );
            least = least.min(arm.received);
        }
    }
    let group = runs.iter().map(|r| r.group).min().unwrap();
    let base = runs.iter().map(|r| r.base).max().unwrap();
    println!("T1RED {NAME} RECEIVED={least} OF={SAMPLES} DETECTOR=group:{group},base:{base}");

    // The detector is the control. On Lyrical the group key carries every sample and the base
    // key none. Elsewhere the base key carries every sample. A cell that fails the control
    // proves nothing, and its failure carries no R1ASSERT marker.
    for run in &runs {
        if distro_has_buffer_mode() {
            assert!(
                run.group == SAMPLES as usize && run.base == 0,
                "UNKNOWN DETECTOR {}: the detector read group {} and base {} of {SAMPLES}, so \
                 the publisher is not in buffer mode and the cell proves nothing",
                run.kind,
                run.group,
                run.base
            );
        } else {
            assert!(
                run.base >= SAMPLES as usize && run.group == 0,
                "UNKNOWN DETECTOR {}: the detector read group {} and base {} of {SAMPLES} on a \
                 distro with no buffer mode",
                run.kind,
                run.group,
                run.base
            );
        }
    }
    for run in &runs {
        for arm in &run.arms {
            let what = format!("{}/{} received", run.kind, arm.name);
            t1_harness::r1_assert_eq(NAME, arm.received, SAMPLES as usize, &what);
            t1_harness::r1_assert_eq(NAME, arm.duplicates, 0, &format!("{what}: repeats"));
            t1_harness::r1_assert_eq(NAME, arm.corrupt, 0, &format!("{what}: corrupt payloads"));
        }
    }
}

const MODE_NOBUF_BASE: &str = "0/modecell/std_msgs::msg::dds_::Float64MultiArray_/RIHS01_1025ddc6b9552d191f89ef1a8d2f60f3d373e28b283d8891ddcc974e8c55397f";

/// The positive control of the mode detector: a publisher that is not in buffer mode. A
/// `Float64MultiArray` has no unbounded `uint8[]` field, so the publisher writes the base key only.
/// The detector must refuse it with the base count above 0 and say the cell proves nothing.
#[test]
fn mode_detector_fails_on_a_publisher_that_is_not_in_buffer_mode() {
    const NAME: &str = "mode_detector_fails_on_a_publisher_that_is_not_in_buffer_mode";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a Lyrical ROS 2 is installed"
    );
    let router = TestRouter::new();
    // A publisher writes only while a ROS subscriber matches it. The detector is a plain session
    // and is not one, so a hiroz subscriber of the same topic makes the publisher write.
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("modecell_node").build().expect("node");
    let _matched = node
        .create_sub::<hiroz_msgs::std_msgs::Float64MultiArray>("/modecell")
        .build()
        .unwrap();
    let detector = t1_harness::ModeDetector::new(router.endpoint(), MODE_NOBUF_BASE);
    wait_for_ready(Duration::from_secs(2));

    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "/modecell",
            "std_msgs/msg/Float64MultiArray",
            "{data: [1.0, 2.0]}",
            "--rate",
            "5",
            "--times",
            "30",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let judged = detector.require_buffer_mode(Duration::from_secs(10));
    detector.report_mode(NAME);
    match judged {
        Err(t1_harness::DetectorError::NotInBufferMode { base, group }) => {
            t1_harness::r1_assert_eq(NAME, group, 0, "group key count");
            if base == 0 {
                t1_harness::r1_fail(NAME, "the refusal reports a base key count of 0");
            }
            let text = t1_harness::DetectorError::NotInBufferMode { base, group }.to_string();
            if !text.contains("proves nothing") {
                t1_harness::r1_fail(NAME, &format!("the refusal does not say so: {text}"));
            }
        }
        other => t1_harness::r1_fail(
            NAME,
            &format!(
                "the detector accepted or could not judge a publisher outside buffer mode: {other:?}"
            ),
        ),
    }
}

const MODE_BUF_BASE: &str = "0/modebuf/std_msgs::msg::dds_::UInt8MultiArray_/RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";

/// The detector accepts a publisher in buffer mode: the group key carries the samples and the
/// base key none. A typed subscriber that joined the group makes the Lyrical publisher switch.
#[test]
fn mode_detector_reads_the_group_key_on_a_publisher_in_buffer_mode() {
    const NAME: &str = "mode_detector_reads_the_group_key_on_a_publisher_in_buffer_mode";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a Lyrical ROS 2 is installed"
    );
    t1_harness::require_buffer_aware_rmw(NAME);
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("modebuf_node").build().expect("node");
    let _joined = node
        .create_sub::<UInt8MultiArray>("/modebuf")
        .build()
        .unwrap();
    let detector = t1_harness::ModeDetector::new(router.endpoint(), MODE_BUF_BASE);
    wait_for_ready(Duration::from_secs(2));

    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "/modebuf",
            "std_msgs/msg/UInt8MultiArray",
            "{data: [1, 2, 3]}",
            "--rate",
            "5",
            "--times",
            "30",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let judged = detector.require_buffer_mode(Duration::from_secs(10));
    detector.report_mode(NAME);
    match judged {
        Ok(group) if group > 0 => {}
        other => t1_harness::r1_fail(
            NAME,
            &format!("the detector did not read the group key: {other:?}"),
        ),
    }
    t1_harness::r1_assert_eq(NAME, detector.read().base, 0, "base key count");
}

/// Fixture F5, the row for a namespaced topic: the group key a released Lyrical publisher writes
/// for `/f5ns/cell`, captured by `f5_capture_reads_the_group_key_a_real_publisher_writes` from the
/// released Lyrical `rmw_zenoh_cpp` 0.10.6 in the container image of the cell runner.
const F5_NAMESPACED_GROUP_KEY: &str = "0/f5ns/cell/std_msgs::msg::dds_::UInt8MultiArray_/RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385/_buf_cpu";

/// Captures the group key of a namespaced topic from a real publisher in buffer mode and prints
/// it as `F5_KEY=<key>`. A plain session subscribes on every group key and records the key
/// expression of each sample, so the key is read from the wire and not derived from hiroz code.
#[test]
fn f5_capture_reads_the_group_key_a_real_publisher_writes() {
    const NAME: &str = "f5_capture_reads_the_group_key_a_real_publisher_writes";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a Lyrical ROS 2 is installed"
    );
    t1_harness::require_buffer_aware_rmw(NAME);
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("f5cell_node").build().expect("node");
    let _joined = node
        .create_sub::<UInt8MultiArray>("/f5ns/cell")
        .build()
        .unwrap();

    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", router.endpoint()))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let plain = zenoh::open(config).wait().expect("plain session");
    let keys = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = keys.clone();
    let _sub = plain
        .declare_subscriber("0/**/_buf_cpu")
        .callback(move |sample| {
            let key = sample.key_expr().to_string();
            let mut keys = seen.lock().unwrap();
            if !keys.contains(&key) {
                keys.push(key);
            }
        })
        .wait()
        .expect("capture subscriber");
    wait_for_ready(Duration::from_secs(2));

    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "/f5ns/cell",
            "std_msgs/msg/UInt8MultiArray",
            "{data: [1, 2, 3]}",
            "--rate",
            "5",
            "--times",
            "30",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && keys.lock().unwrap().is_empty() {
        thread::sleep(Duration::from_millis(50));
    }
    let captured = keys.lock().unwrap().clone();
    println!("F5_KEYS={}", captured.len());
    for key in &captured {
        println!("F5_KEY={key}");
    }
    t1_harness::r1_assert_eq(NAME, captured.len(), 1, "group keys captured");
    let key = &captured[0];
    if !(key.starts_with("0/f5ns") && key.ends_with("/_buf_cpu")) {
        t1_harness::r1_fail(NAME, &format!("unexpected key shape: {key}"));
    }
    t1_harness::r1_assert_eq(
        NAME,
        key.as_str(),
        F5_NAMESPACED_GROUP_KEY,
        "recorded F5 key",
    );
}

/// A plain client session on the router: it is no ROS entity, so it never matches a publisher.
fn plain_session(endpoint: &str) -> zenoh::Session {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    zenoh::open(config).wait().expect("plain session")
}

/// The liveliness token of the one entity on `topic`, once it is visible. A token that never
/// appears fails the test with the R1ASSERT marker of `name`.
fn token_for(name: &str, session: &zenoh::Session, topic: &str) -> String {
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
        if Instant::now() >= deadline {
            t1_harness::r1_fail(
                name,
                &format!("{} tokens for {topic}: {hits:?}", hits.len()),
            );
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Cycle 2.11: an older distro accepts the joined token and lists the subscriber. The test reads
/// the token the older parser has to parse and the `ros2 topic info -v` text of the released
/// CLI. The token must end in `backends:cpu:`: that field is what an older parser must tolerate,
/// and a subscriber that never joined has none, so the test cannot pass without the join.
#[test]
fn old_parsers_list_a_joined_subscriber() {
    const NAME: &str = "old_parsers_list_a_joined_subscriber";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a released ROS 2 is installed"
    );
    let router = TestRouter::new();
    let plain = plain_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("oldp_node").build().expect("node");
    let _joined = node.create_sub::<UInt8MultiArray>("/oldp").build().unwrap();

    let token = token_for(NAME, &plain, "oldp");
    println!("T1TOKEN {NAME} {token}");
    t1_harness::r1_assert_eq(
        NAME,
        token.ends_with("/backends:cpu:"),
        true,
        "the token ends in backends:cpu:",
    );

    // The released CLI parses the token of the joined subscriber. Discovery needs a moment, so
    // read the listing until the subscriber shows or the time is out.
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
    println!("T1INFO {NAME}\n{text}");
    t1_harness::r1_assert_eq(
        NAME,
        text.contains("Subscription count: 1"),
        true,
        &format!("the CLI lists one subscription: {text}"),
    );
    t1_harness::r1_assert_eq(
        NAME,
        text.contains("oldp_node") && text.contains("SUBSCRIPTION"),
        true,
        &format!("the CLI names the hiroz node as a subscription endpoint: {text}"),
    );
}

#[cfg(not(any(feature = "kilted", feature = "lyrical")))]
use hiroz_msgs::action_tutorials_interfaces::{FibonacciGoal, action::Fibonacci};
#[cfg(any(feature = "kilted", feature = "lyrical"))]
use hiroz_msgs::example_interfaces::{FibonacciGoal, action::Fibonacci};

/// Cycle 2.15: the hiroz action client receives each feedback message of the released Fibonacci
/// server once. The server of `action_tutorials_cpp` publishes the whole sequence after each
/// step, so feedback number `i` holds `i + 3` numbers: the received lengths must be exactly
/// `3, 4, ..., order + 1`, each once. The group key carries no sample, as the feedback is not a
/// buffer topic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn action_feedback_arrives_once_each_with_the_group_joined() {
    use std::{
        os::unix::process::CommandExt,
        sync::atomic::{AtomicUsize, Ordering},
    };

    const NAME: &str = "action_feedback_arrives_once_each_with_the_group_joined";
    const ORDER: i32 = 6;
    assert!(
        check_ros2_available() && check_action_tutorials_cpp_available(),
        "ros2 and action_tutorials_cpp are required: run this test where they are installed"
    );
    let router = TestRouter::new();
    let plain = plain_session(router.endpoint());
    let group = Arc::new(AtomicUsize::new(0));
    let seen = group.clone();
    let _group_sub = plain
        .declare_subscriber("0/**/_buf_cpu")
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
    let node = ctx.create_node("t1_fib_client").build().expect("node");
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
            Ok(Err(e)) => t1_harness::r1_fail(NAME, &format!("the server never accepted: {e}")),
            Err(_) => {
                t1_harness::r1_fail(NAME, "the server never accepted: the goal call timed out")
            }
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
    let result = tokio::time::timeout(Duration::from_secs(30), goal_handle.result())
        .await
        .expect("the goal finishes")
        .expect("result");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let got = lengths.lock().unwrap().clone();
    let expected: Vec<usize> = (3..=(ORDER as usize + 1)).collect();
    println!(
        "T1FEEDBACK {NAME} LENGTHS={got:?} EXPECTED={expected:?} GROUP={} RESULT_LEN={}",
        group.load(Ordering::SeqCst),
        result.sequence.len()
    );
    t1_harness::r1_assert_eq(NAME, got, expected, "feedback lengths, each once in order");
    t1_harness::r1_assert_eq(
        NAME,
        group.load(Ordering::SeqCst),
        0,
        "samples on the group key",
    );
}

// ---- cells below need the join machinery of the lane: they name BufferGroup ----

/// Cycle 2.13: a topic that is not a buffer topic delivers each sample once to a subscriber that
/// joined the group: its count equals the base key count, and the group key carries nothing.
#[test]
fn non_buffer_topic_delivers_each_sample_once_with_the_group_joined() {
    const NAME: &str = "non_buffer_topic_delivers_each_sample_once_with_the_group_joined";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a released ROS 2 is installed"
    );
    const FLOAT_BASE: &str = "0/plaincell/std_msgs::msg::dds_::Float64MultiArray_/RIHS01_1025ddc6b9552d191f89ef1a8d2f60f3d373e28b283d8891ddcc974e8c55397f";
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("plaincell_node").build().expect("node");
    let joined = node
        .create_sub::<hiroz_msgs::std_msgs::Float64MultiArray>("/plaincell")
        .with_buffer_group(hiroz::BufferGroup::Cpu)
        .build()
        .unwrap();
    let detector = ModeDetector::new(router.endpoint(), FLOAT_BASE);
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
    while Instant::now() < end && detector.read().base < PUBLISHED {
        received += joined.recv_timeout(Duration::from_millis(100)).is_ok() as usize;
    }
    while joined.recv_timeout(Duration::from_millis(500)).is_ok() {
        received += 1;
    }

    let reading = detector.read();
    println!(
        "T1MODE {NAME} GROUP={} BASE={} RECEIVED={received}",
        reading.group, reading.base
    );
    // The control: the base key carries the samples, so the topic is not a buffer topic.
    assert!(
        reading.base >= PUBLISHED * 2 / 3,
        "UNKNOWN DETECTOR: the base key carried {} of {PUBLISHED}",
        reading.base
    );
    t1_harness::r1_assert_eq(NAME, reading.group, 0, "group key count");
    t1_harness::r1_assert_eq(
        NAME,
        received,
        reading.base,
        "received against the base key",
    );
}

/// What one arm of a mixed-set cell read: the detector counts and each subscriber.
struct MixRun {
    group: usize,
    base: usize,
    joined: Arm,
    legacy: Option<Arm>,
}

/// One cell of the mixed matched set. A joined hiroz subscriber reads a topic that a released
/// `rclpy` publisher writes. `legacy` adds a hiroz subscriber that stays out of the group
/// (`BufferGroup::Off`), whose token has no `backends:` field, so the publisher sees a subscriber
/// that is not buffer-aware. `anchor` adds the released buffer-aware subscriber.
fn mixed_run(topic: &str, legacy: bool, anchor: bool) -> MixRun {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("t1mix_node").build().expect("node");
    let joined = node.create_sub::<UInt8MultiArray>(topic).build().unwrap();
    let off = legacy.then(|| {
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
    let detector = ModeDetector::new(router.endpoint(), &base);
    wait_for_ready(Duration::from_secs(2));
    let _anchor = anchor.then(|| spawn_anchor(&router, "uint8", topic));
    wait_for_ready(Duration::from_secs(2));
    let matched = 1 + legacy as usize + anchor as usize;
    let mut guard = spawn_seq_publisher(&router, "uint8", topic, 64, matched);

    let (mut c_joined, mut c_off) = (SeqCounter::new(), SeqCounter::new());
    let poll = Duration::from_millis(10);
    let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 40);
    let mut finished_at: Option<Instant> = None;
    while Instant::now() < end {
        if let Ok(m) = joined.recv_timeout(poll) {
            c_joined.record(&m.data.contiguous());
        }
        if let Some(off) = &off
            && let Ok(m) = off.recv_timeout(poll)
        {
            c_off.record(&m.data.contiguous());
        }
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
    let arm = |name: &'static str, c: &SeqCounter| Arm {
        name,
        received: c.received_of(SAMPLES),
        duplicates: c.duplicates(),
        corrupt: c.corrupt(),
    };
    let reading = detector.read();
    MixRun {
        group: reading.group,
        base: reading.base,
        joined: arm("joined", &c_joined),
        legacy: off.is_some().then(|| arm("legacy", &c_off)),
    }
}

/// A buffer-aware `rmw_zenoh` publisher writes each sample once, on one key, whatever mix of
/// subscribers is matched, and a joined hiroz subscriber that reads the base key and the group
/// key delivers each sample once. The cell runs three matched sets and prints the detector
/// counts and each subscriber's counts per set:
///
/// - `aware`: the joined subscriber and the released anchor. Every subscriber is buffer-aware.
///   This is the control: the group key must carry every sample, so the publisher is in buffer
///   mode and the cell can tell the keys apart.
/// - `mixed`: the joined subscriber and an `Off` subscriber. One subscriber is not buffer-aware.
/// - `mixed_anchor`: the joined subscriber, an `Off` subscriber and the anchor.
#[test]
fn mixed_matched_set_delivers_each_sample_once() {
    const NAME: &str = "mixed_matched_set_delivers_each_sample_once";
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a released ROS 2 is installed"
    );
    t1_harness::require_buffer_aware_rmw(NAME);
    let runs = [
        ("aware", mixed_run("/mixaware", false, true)),
        ("mixed", mixed_run("/mixmixed", true, false)),
        ("mixed_anchor", mixed_run("/mixanchor", true, true)),
    ];
    for (set, run) in &runs {
        println!(
            "T1MIX {NAME}/{set} GROUP={} BASE={} JOINED_RECEIVED={} JOINED_DUPLICATES={} \
             JOINED_CORRUPT={} LEGACY_RECEIVED={} LEGACY_DUPLICATES={} OF={SAMPLES}",
            run.group,
            run.base,
            run.joined.received,
            run.joined.duplicates,
            run.joined.corrupt,
            run.legacy
                .as_ref()
                .map_or("none".into(), |a| a.received.to_string()),
            run.legacy
                .as_ref()
                .map_or("none".into(), |a| a.duplicates.to_string()),
        );
    }

    // The control: with every subscriber buffer-aware the group key carries every sample and the
    // base key none. A cell that fails it proves nothing about the mixed sets.
    let (_, aware) = &runs[0];
    assert!(
        aware.group == SAMPLES as usize && aware.base == 0,
        "UNKNOWN DETECTOR: the detector read group {} and base {} of {SAMPLES} with every \
         subscriber buffer-aware, so the publisher is not in buffer mode",
        aware.group,
        aware.base
    );
    for (set, run) in &runs {
        let arms = std::iter::once(&run.joined).chain(run.legacy.iter());
        for arm in arms {
            let what = format!("{set}/{} received", arm.name);
            t1_harness::r1_assert_eq(NAME, arm.received, SAMPLES as usize, &what);
            t1_harness::r1_assert_eq(NAME, arm.duplicates, 0, &format!("{what}: repeats"));
            t1_harness::r1_assert_eq(NAME, arm.corrupt, 0, &format!("{what}: corrupt payloads"));
        }
    }
}
