//! A hiroz subscriber must receive from a released `rmw_zenoh` publisher in buffer mode.
//!
//! The publisher is `ros2 topic pub` of a Lyrical `rmw_zenoh_cpp`. Lyrical writes a message with
//! an unbounded `uint8[]` field only on `<topic key>/_buf_cpu` when every matched subscriber
//! announces buffer support. A plain Zenoh session counts the samples on both keys, so the test
//! knows the publisher is in buffer mode without depending on a subscriber starving: the group
//! key must carry the samples and the base key none.

#![cfg(feature = "ros-interop")]

mod common;

use std::{
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use common::{t1_harness, *};
use hiroz::{Builder, MessageTypeInfo};
use hiroz_msgs::std_msgs::UInt8MultiArray;
use serde::{Deserialize, Serialize};
use zenoh::Wait;

const BASE: &str = "0/bufcell/std_msgs::msg::dds_::UInt8MultiArray_/RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";

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

fn counter(
    session: &zenoh::Session,
    key: &str,
) -> (zenoh::pubsub::Subscriber<()>, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let sub = session
        .declare_subscriber(key.to_owned())
        .callback(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .wait()
        .expect("counter");
    (sub, count)
}

/// Cycle 2.12: a typed subscriber and a no-schema subscriber receive every sample a Lyrical
/// publisher in buffer mode writes. The plain session is the mode detector and the count.
#[test]
fn joined_subscribers_receive_from_a_released_publisher() {
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a Lyrical ROS 2 is installed"
    );
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("bufcell_node").build().expect("node");
    let typed = node
        .create_sub::<UInt8MultiArray>("/bufcell")
        .build()
        .unwrap();
    let blob = node.create_sub::<Blob>("/bufcell").build().unwrap();

    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", router.endpoint()))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let plain = zenoh::open(config).wait().expect("plain session");
    let (_group_sub, group) = counter(&plain, &format!("{BASE}/_buf_cpu"));
    let (_base_sub, base) = counter(&plain, BASE);
    wait_for_ready(Duration::from_secs(2));

    const SAMPLES: usize = 30;
    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            "/bufcell",
            "std_msgs/msg/UInt8MultiArray",
            "{data: [1, 2, 3]}",
            "--rate",
            "5",
            "--times",
            &SAMPLES.to_string(),
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let (mut typed_n, mut blob_n) = (0, 0);
    let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 8);
    while Instant::now() < end {
        typed_n += typed.recv_timeout(Duration::from_millis(50)).is_ok() as usize;
        blob_n += blob.recv_timeout(Duration::from_millis(50)).is_ok() as usize;
        if group.load(Ordering::SeqCst) >= SAMPLES && typed_n >= SAMPLES && blob_n >= SAMPLES {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(500));
    while typed.recv_timeout(Duration::from_millis(50)).is_ok() {
        typed_n += 1;
    }
    while blob.recv_timeout(Duration::from_millis(50)).is_ok() {
        blob_n += 1;
    }

    let (group_n, base_n) = (group.load(Ordering::SeqCst), base.load(Ordering::SeqCst));
    // Kill criterion: samples on the base key mean the publisher is not in buffer mode, and the
    // cell proves nothing.
    assert_eq!(base_n, 0, "the publisher is not in buffer mode");
    assert!(
        group_n >= SAMPLES * 2 / 3,
        "group key carried {group_n} of {SAMPLES}"
    );
    assert_eq!(
        typed_n, group_n,
        "typed subscriber against the group key count"
    );
    assert_eq!(
        blob_n, group_n,
        "no-schema subscriber against the group key count"
    );
}

/// Cycle 2.13: a topic that is not a buffer topic delivers each sample once to a subscriber that
/// joined the group: its count equals the base key count, and the group key carries nothing.
#[test]
fn non_buffer_topic_delivers_each_sample_once_with_the_group_joined() {
    assert!(
        check_ros2_available(),
        "ros2 is required: run this test where a Lyrical ROS 2 is installed"
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

    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", router.endpoint()))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let plain = zenoh::open(config).wait().expect("plain session");
    let (_group_sub, group) = counter(&plain, &format!("{FLOAT_BASE}/_buf_cpu"));
    let (_base_sub, base) = counter(&plain, FLOAT_BASE);
    wait_for_ready(Duration::from_secs(2));

    const SAMPLES: usize = 30;
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
            &SAMPLES.to_string(),
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ros2 topic pub");
    let _guard = ProcessGuard::new(publisher, "ros2 pub");

    let mut received = 0;
    let end = Instant::now() + Duration::from_secs(SAMPLES as u64 / 5 + 8);
    while Instant::now() < end && base.load(Ordering::SeqCst) < SAMPLES {
        received += joined.recv_timeout(Duration::from_millis(100)).is_ok() as usize;
    }
    while joined.recv_timeout(Duration::from_millis(500)).is_ok() {
        received += 1;
    }

    let base_n = base.load(Ordering::SeqCst);
    assert!(
        base_n >= SAMPLES * 2 / 3,
        "base key carried {base_n} of {SAMPLES}"
    );
    assert_eq!(
        group.load(Ordering::SeqCst),
        0,
        "the group key carried a sample"
    );
    assert_eq!(received, base_n, "each sample must arrive once");
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

    match detector.require_buffer_mode(Duration::from_secs(10)) {
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
