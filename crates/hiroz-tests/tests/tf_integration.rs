//! Integration tests for hiroz-tf Buffer.
//!
//! Tests require a Zenoh router (provided by TestRouter) and compile only
//! when the `tf-tests` feature is enabled.

#![cfg(feature = "tf-tests")]

mod common;

use std::time::Duration;

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::Builder;
use hiroz::qos::{QosDurability, QosHistory, QosProfile, QosReliability};
use hiroz::time::ZTime;
use hiroz_msgs::builtin_interfaces::Time;
use hiroz_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
use hiroz_msgs::std_msgs::Header;
use hiroz_msgs::tf2_msgs::TFMessage;
use hiroz_tf::{Buffer, StaticTransformBroadcaster, TransformBroadcaster, WaitError};

fn make_tf(parent: &str, child: &str, sec: i32, x: f64) -> TransformStamped {
    TransformStamped {
        header: Header {
            frame_id: parent.to_string(),
            stamp: Time { sec, nanosec: 0 },
        },
        child_frame_id: child.to_string(),
        transform: Transform {
            translation: Vector3 { x, y: 0.0, z: 0.0 },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        },
    }
}

/// Publish `/tf` and verify the Buffer receives and exposes it.
#[tokio::test(flavor = "multi_thread")]
async fn tf_buffer_receives_dynamic_transform() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_test_node").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    // Publisher node on same router
    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_publisher").build().unwrap();
    let tf_pub = pub_node
        .create_pub::<TFMessage>("/tf")
        .with_qos(QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::Volatile,
            history: QosHistory::KeepLast(std::num::NonZeroUsize::new(100).unwrap()),
            ..Default::default()
        })
        .build()
        .unwrap();

    // Wait for subscription to be established
    tf_pub
        .wait_for_subscription(1, Duration::from_secs(5))
        .await;

    tf_pub
        .async_publish(&TFMessage {
            transforms: vec![make_tf("map", "odom", 10, 3.0)],
        })
        .await
        .unwrap();

    // Give the callback time to fire
    tokio::time::sleep(Duration::from_millis(200)).await;

    let tf = buffer
        .lookup_transform("map", "odom", ZTime::zero())
        .unwrap();
    assert!(
        (tf.transform.translation.x - 3.0).abs() < 1e-6,
        "expected x=3.0, got {}",
        tf.transform.translation.x
    );
}

/// `/tf_static` with TransientLocal: new subscriber gets old static transforms.
#[tokio::test(flavor = "multi_thread")]
async fn tf_static_transient_local_replayed_on_connect() {
    let router = TestRouter::new();

    // Publish static transform BEFORE creating the Buffer subscriber
    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_static_publisher").build().unwrap();
    let tf_static_pub = pub_node
        .create_pub::<TFMessage>("/tf_static")
        .with_qos(QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::TransientLocal,
            history: QosHistory::KeepLast(std::num::NonZeroUsize::new(100).unwrap()),
            ..Default::default()
        })
        .build()
        .unwrap();

    tf_static_pub
        .async_publish(&TFMessage {
            transforms: vec![make_tf("map", "sensor", 0, 0.5)],
        })
        .await
        .unwrap();

    // Small delay so the publication is stored in the TransientLocal publisher
    tokio::time::sleep(Duration::from_millis(200)).await;

    // NOW create the buffer — it subscribes AFTER the publish
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_test_node_static").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    // TransientLocal should replay the stored message
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        buffer.can_transform("map", "sensor", ZTime::zero()),
        "static transform should be available after TransientLocal replay"
    );
    let tf = buffer
        .lookup_transform("map", "sensor", ZTime::zero())
        .unwrap();
    assert!((tf.transform.translation.x - 0.5).abs() < 1e-6);
}

/// Two-frame chain: map→odom + odom→base_link, lookup map←base_link.
#[tokio::test(flavor = "multi_thread")]
async fn tf_two_frame_chain_composes_correctly() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_chain_node").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_chain_publisher").build().unwrap();
    let tf_pub = pub_node
        .create_pub::<TFMessage>("/tf")
        .with_qos(QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::Volatile,
            history: QosHistory::KeepLast(std::num::NonZeroUsize::new(100).unwrap()),
            ..Default::default()
        })
        .build()
        .unwrap();

    tf_pub
        .wait_for_subscription(1, Duration::from_secs(5))
        .await;

    // map→odom: x=1, odom→base_link: x=2 → base_link in map = x=3
    tf_pub
        .async_publish(&TFMessage {
            transforms: vec![
                make_tf("map", "odom", 10, 1.0),
                make_tf("odom", "base_link", 10, 2.0),
            ],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let tf = buffer
        .lookup_transform("map", "base_link", ZTime::zero())
        .unwrap();
    assert!(
        (tf.transform.translation.x - 3.0).abs() < 1e-5,
        "expected x=3.0, got {}",
        tf.transform.translation.x
    );
}

/// `can_transform` returns false before transforms arrive, true after.
#[tokio::test(flavor = "multi_thread")]
async fn can_transform_reflects_availability() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_can_transform_node").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    // Initially false
    assert!(!buffer.can_transform("map", "robot", ZTime::zero()));

    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_can_publisher").build().unwrap();
    let tf_pub = pub_node
        .create_pub::<TFMessage>("/tf")
        .with_qos(QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::Volatile,
            history: QosHistory::KeepLast(std::num::NonZeroUsize::new(100).unwrap()),
            ..Default::default()
        })
        .build()
        .unwrap();

    tf_pub
        .wait_for_subscription(1, Duration::from_secs(5))
        .await;
    tf_pub
        .async_publish(&TFMessage {
            transforms: vec![make_tf("map", "robot", 10, 1.0)],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(buffer.can_transform("map", "robot", ZTime::zero()));
}

/// `TransformBroadcaster` publishes to `/tf`; `Buffer` on the same network receives it.
#[tokio::test(flavor = "multi_thread")]
async fn broadcaster_dynamic_roundtrip() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_broadcaster_rx").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_broadcaster_tx").build().unwrap();
    let broadcaster = TransformBroadcaster::new(&pub_node).unwrap();

    // Give the subscription time to establish
    tokio::time::sleep(Duration::from_millis(300)).await;

    broadcaster
        .send_transform(make_tf("map", "base_link", 5, 2.5))
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let tf = buffer
        .lookup_transform("map", "base_link", ZTime::zero())
        .unwrap();
    assert!(
        (tf.transform.translation.x - 2.5).abs() < 1e-6,
        "expected x=2.5, got {}",
        tf.transform.translation.x
    );
}

/// `StaticTransformBroadcaster` publishes to `/tf_static` with TransientLocal;
/// a `Buffer` created after the publish receives the static transform via cache replay.
#[tokio::test(flavor = "multi_thread")]
async fn broadcaster_static_roundtrip_with_late_joiner() {
    let router = TestRouter::new();

    // Publish static transform BEFORE creating the Buffer
    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("tf_static_tx").build().unwrap();
    let broadcaster = StaticTransformBroadcaster::new(&pub_node).unwrap();

    broadcaster
        .send_transform(make_tf("world", "camera_link", 0, 0.3))
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Create Buffer after publication — should get replay
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("tf_static_rx").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        buffer.can_transform("world", "camera_link", ZTime::zero()),
        "static transform should be available via TransientLocal replay"
    );
    let tf = buffer
        .lookup_transform("world", "camera_link", ZTime::zero())
        .unwrap();
    assert!((tf.transform.translation.x - 0.3).abs() < 1e-6);
}

/// `wait_for_transform` returns once a matching transform arrives.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_transform_returns_when_data_arrives() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("wft_rx").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    let pub_ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let pub_node = pub_ctx.create_node("wft_tx").build().unwrap();
    let broadcaster = TransformBroadcaster::new(&pub_node).unwrap();

    // Publish after a short delay in the background
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        broadcaster
            .send_transform(make_tf("map", "lidar", 10, 1.0))
            .unwrap();
    });

    let result = buffer
        .wait_for_transform("map", "lidar", ZTime::zero(), Some(Duration::from_secs(3)))
        .await;

    assert!(result.is_ok(), "expected transform, got: {:?}", result);
    assert!((result.unwrap().transform.translation.x - 1.0).abs() < 1e-6);
}

/// `wait_for_transform` returns `WaitError::Timeout` when no data arrives.
#[tokio::test(flavor = "multi_thread")]
async fn wait_for_transform_times_out() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).unwrap();
    let node = ctx.create_node("wft_timeout_node").build().unwrap();
    let buffer = Buffer::new(&node).unwrap();

    let result = buffer
        .wait_for_transform(
            "ghost",
            "frame",
            ZTime::zero(),
            Some(Duration::from_millis(300)),
        )
        .await;

    assert!(
        matches!(result, Err(WaitError::Timeout)),
        "expected Timeout, got: {:?}",
        result
    );
}
