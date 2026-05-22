//! Pub/sub interoperability tests between hiroz and rmw_zenoh_cpp.
//!
//! Tests cover a matrix of message types at different nesting depths to verify
//! that RIHS01 type hashes are computed correctly. If the hash is wrong, Zenoh
//! rejects the connection and nothing arrives — delivery itself is the proof.
//!
//! # Test matrix
//!
//! | Type | Depth | Why |
//! |------|-------|-----|
//! | `std_msgs/msg/String`            | 0 | Baseline — no nested deps |
//! | `std_msgs/msg/Header`            | 1 | `builtin_interfaces/Time` |
//! | `geometry_msgs/msg/Twist`        | 1 | `Vector3` × 2 |
//! | `geometry_msgs/msg/TwistStamped` | 2 | Canonical bug repro from issue #99 |
//! | `geometry_msgs/msg/PoseStamped`  | 2 | `Header` + `Pose` → `Point`, `Quaternion` |
//! | `sensor_msgs/msg/Imu`            | 2 | `Header`, `Quaternion`, `Vector3` × 3 |
//! | `nav_msgs/msg/Odometry`          | 3 | Deepest nesting in common types |
//!
//! Each `#[test]` function creates its own `TestRouter` on a unique port, so all
//! 14 tests (7 types × 2 directions) run in parallel under cargo nextest.

#![cfg(feature = "ros-interop")]

mod common;

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use common::*;
use hiroz::{
    Builder, WithTypeInfo,
    msg::{ZDeserializer, ZMessage, ZSerializer},
};
use hiroz_msgs::{
    geometry_msgs::{PoseStamped, Twist, TwistStamped},
    nav_msgs::Odometry,
    sensor_msgs::Imu,
    std_msgs::{Header, String as RosString},
};

// ---------------------------------------------------------------------------
// Test case definitions
// ---------------------------------------------------------------------------

struct InteropCase {
    /// ROS 2 fully-qualified type name, e.g. `"std_msgs/msg/String"`
    type_name: &'static str,
    /// Unique topic to avoid cross-test message pollution
    topic: &'static str,
    /// YAML payload for `ros2 topic pub` (rmw_zenoh_cpp publisher direction)
    payload: &'static str,
}

const CASES: &[InteropCase] = &[
    InteropCase {
        type_name: "std_msgs/msg/String",
        topic: "/it_string",
        payload: "{data: hello}",
    },
    InteropCase {
        type_name: "std_msgs/msg/Header",
        topic: "/it_header",
        payload: "{frame_id: world}",
    },
    InteropCase {
        type_name: "geometry_msgs/msg/Twist",
        topic: "/it_twist",
        payload: "{linear: {x: 1.0, y: 0.0, z: 0.0}, angular: {x: 0.0, y: 0.0, z: 0.5}}",
    },
    InteropCase {
        type_name: "geometry_msgs/msg/TwistStamped",
        topic: "/it_twiststamped",
        payload: "{twist: {linear: {x: 1.0, y: 0.0, z: 0.0}}}",
    },
    InteropCase {
        type_name: "geometry_msgs/msg/PoseStamped",
        topic: "/it_posestamped",
        payload: "{pose: {position: {x: 1.0, y: 2.0, z: 3.0}}}",
    },
    InteropCase {
        type_name: "sensor_msgs/msg/Imu",
        topic: "/it_imu",
        payload: "{linear_acceleration: {x: 0.0, y: 0.0, z: 9.8}}",
    },
    InteropCase {
        type_name: "nav_msgs/msg/Odometry",
        topic: "/it_odom",
        payload: "{twist: {twist: {linear: {x: 0.5, y: 0.0, z: 0.0}}}}",
    },
];

// ---------------------------------------------------------------------------
// Generic helpers
// ---------------------------------------------------------------------------

/// Test direction: hiroz publisher → rmw_zenoh_cpp subscriber.
///
/// Publishes `msg` from a hiroz node; asserts that `ros2 topic echo --once`
/// (rmw_zenoh_cpp) exits cleanly, indicating it received the message.
/// If the type hash is wrong, Zenoh rejects the connection and the echo
/// process never receives anything and does not exit.
fn hiroz_pub_to_ros2_sub<T>(case: &InteropCase)
where
    T: ZMessage + WithTypeInfo + Default + 'static,
    T::Serdes: for<'a> ZSerializer<Input<'a> = &'a T>,
{
    if !check_ros2_available() {
        eprintln!("Skipping {}: ros2 CLI not available", case.type_name);
        return;
    }

    let router = TestRouter::new();
    println!(
        "\n=== hiroz pub → ros2 sub: {} on {} ===",
        case.type_name, case.topic
    );

    // Start rmw_zenoh_cpp subscriber
    let mut ros2_sub = Command::new("ros2")
        .args(["topic", "echo", case.topic, case.type_name, "--once"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ros2 topic echo");

    // Give it time to connect to the router and start listening
    wait_for_ready(Duration::from_secs(2));

    // Publish from hiroz in a background thread
    let endpoint = router.endpoint().to_string();
    let topic = case.topic.to_string();
    let pub_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create hiroz context");
        let node = ctx
            .create_node("hiroz_pub_interop")
            .build()
            .expect("Failed to create node");
        let publisher = node
            .create_pub::<T>(&topic)
            .build()
            .expect("Failed to create publisher");
        for _ in 0..5 {
            publisher.publish(&T::default()).ok();
            thread::sleep(Duration::from_millis(500));
        }
    });

    pub_handle.join().expect("Publisher thread panicked");

    // Give ros2 a moment to process the last message
    wait_for_ready(Duration::from_secs(2));

    // ros2 topic echo --once exits after receiving one message
    let received = ros2_sub.try_wait().unwrap_or(None).is_some();

    // Kill the subprocess (in case it's still running due to hash mismatch)
    let _ = ros2_sub.kill();
    let output = ros2_sub
        .wait_with_output()
        .expect("Failed to wait for ros2 output");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        received || !stdout.trim().is_empty(),
        "rmw_zenoh_cpp subscriber received nothing for {} — type hash mismatch?",
        case.type_name
    );

    println!(
        "PASS: rmw_zenoh_cpp received {} on {}",
        case.type_name, case.topic
    );
}

/// Test direction: rmw_zenoh_cpp publisher → hiroz subscriber.
///
/// Publishes via `ros2 topic pub` (rmw_zenoh_cpp); asserts that the hiroz
/// subscriber receives at least one message. If the type hash is wrong, Zenoh
/// rejects the connection and nothing arrives.
fn ros2_pub_to_hiroz_sub<T>(case: &InteropCase)
where
    T: ZMessage + WithTypeInfo + 'static,
    T::Serdes: for<'a> ZDeserializer<Input<'a> = &'a [u8]>,
{
    if !check_ros2_available() {
        eprintln!("Skipping {}: ros2 CLI not available", case.type_name);
        return;
    }

    let router = TestRouter::new();
    println!(
        "\n=== ros2 pub → hiroz sub: {} on {} ===",
        case.type_name, case.topic
    );

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    // Start hiroz subscriber in a background thread
    let endpoint = router.endpoint().to_string();
    let topic = case.topic.to_string();
    let sub_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&endpoint).expect("Failed to create hiroz context");
        let node = ctx
            .create_node("hiroz_sub_interop")
            .build()
            .expect("Failed to create node");
        let subscriber = node
            .create_sub::<T>(&topic)
            .build()
            .expect("Failed to create subscriber");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if subscriber.recv_timeout(Duration::from_millis(100)).is_ok() {
                received_clone.store(true, Ordering::Relaxed);
                break;
            }
        }
    });

    // Give the subscriber time to connect
    wait_for_ready(Duration::from_secs(2));

    // Publish from rmw_zenoh_cpp
    let publisher = Command::new("ros2")
        .args([
            "topic",
            "pub",
            case.topic,
            case.type_name,
            case.payload,
            "--rate",
            "2",
            "--times",
            "10",
        ])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ros2 topic pub");

    let _pub_guard = ProcessGuard::new(publisher, "ros2 pub");

    sub_handle.join().expect("Subscriber thread panicked");

    assert!(
        received.load(Ordering::Relaxed),
        "hiroz subscriber received nothing from rmw_zenoh_cpp for {} — type hash mismatch?",
        case.type_name
    );

    println!("PASS: hiroz received {} on {}", case.type_name, case.topic);
}

// ---------------------------------------------------------------------------
// hiroz pub → ros2 sub  (7 types)
// ---------------------------------------------------------------------------

#[test]
fn hiroz_pub_string() {
    hiroz_pub_to_ros2_sub::<RosString>(&CASES[0]);
}

#[test]
fn hiroz_pub_header() {
    hiroz_pub_to_ros2_sub::<Header>(&CASES[1]);
}

#[test]
fn hiroz_pub_twist() {
    hiroz_pub_to_ros2_sub::<Twist>(&CASES[2]);
}

#[test]
fn hiroz_pub_twist_stamped() {
    hiroz_pub_to_ros2_sub::<TwistStamped>(&CASES[3]);
}

#[test]
fn hiroz_pub_pose_stamped() {
    hiroz_pub_to_ros2_sub::<PoseStamped>(&CASES[4]);
}

#[test]
fn hiroz_pub_imu() {
    hiroz_pub_to_ros2_sub::<Imu>(&CASES[5]);
}

#[test]
fn hiroz_pub_odometry() {
    hiroz_pub_to_ros2_sub::<Odometry>(&CASES[6]);
}

// ---------------------------------------------------------------------------
// ros2 pub → hiroz sub  (7 types)
// ---------------------------------------------------------------------------

#[test]
fn ros2_pub_string() {
    ros2_pub_to_hiroz_sub::<RosString>(&CASES[0]);
}

#[test]
fn ros2_pub_header() {
    ros2_pub_to_hiroz_sub::<Header>(&CASES[1]);
}

#[test]
fn ros2_pub_twist() {
    ros2_pub_to_hiroz_sub::<Twist>(&CASES[2]);
}

#[test]
fn ros2_pub_twist_stamped() {
    ros2_pub_to_hiroz_sub::<TwistStamped>(&CASES[3]);
}

#[test]
fn ros2_pub_pose_stamped() {
    ros2_pub_to_hiroz_sub::<PoseStamped>(&CASES[4]);
}

#[test]
fn ros2_pub_imu() {
    ros2_pub_to_hiroz_sub::<Imu>(&CASES[5]);
}

#[test]
fn ros2_pub_odometry() {
    ros2_pub_to_hiroz_sub::<Odometry>(&CASES[6]);
}
