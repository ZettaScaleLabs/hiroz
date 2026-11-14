//! A cache applies the same subscriber checks as a plain subscriber.
//!
//! A control in the same test shows that the checked behaviour differs from the default, so a green
//! result cannot come from a cache that never receives anything.

use std::{
    num::NonZeroUsize,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod common;

use common::{connected_nodes, isolated_node, wait_until};
use hiroz::{
    BufferGroup, Builder,
    attachment::Attachment,
    event::ZenohEventType,
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};
use hiroz_msgs::std_msgs::String as RosString;
use zenoh::{Wait, sample::Locality};

fn message(text: &str) -> RosString {
    RosString {
        data: text.to_string(),
    }
}

/// A cache built with a transient-local QoS replays the history of a publisher that started
/// earlier. A cache with the default QoS does not.
///
/// Control in the same test: the volatile cache receives a live message after the replay and
/// holds nothing else, so the replay comes from the durability and the volatile subscription
/// works.
#[test]
fn cache_replays_transient_local_history() {
    const TOPIC: &str = "/cache_parity_transient_local";
    let (_ctx, node) = isolated_node("cache_transient_local");
    let qos = QosProfile {
        durability: QosDurability::TransientLocal,
        reliability: QosReliability::Reliable,
        history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        ..Default::default()
    };
    let publisher = node
        .create_pub::<RosString>(TOPIC)
        .with_qos(qos)
        .build()
        .expect("publisher");
    for i in 0..3 {
        publisher
            .publish(&message(&format!("history {i}")))
            .expect("publish");
    }

    let durable = node
        .create_cache::<RosString>(TOPIC, 10)
        .with_qos(qos)
        .build()
        .expect("transient-local cache");
    let volatile = node
        .create_cache::<RosString>(TOPIC, 10)
        .build()
        .expect("volatile cache");

    wait_until("the transient-local cache to replay 3 messages", || {
        durable.len() == 3
    });
    let everything = |cache: &hiroz::cache::ZCache<RosString>| {
        cache.get_interval(UNIX_EPOCH, SystemTime::now() + Duration::from_secs(86_400))
    };
    let mut replayed: Vec<String> = everything(&durable)
        .iter()
        .map(|m| m.data.clone())
        .collect();
    replayed.sort();
    assert_eq!(replayed, ["history 0", "history 1", "history 2"]);

    publisher.publish(&message("live")).expect("publish");
    wait_until("the volatile cache to receive a live message", || {
        !volatile.is_empty()
    });
    wait_until(
        "the transient-local cache to receive the live message",
        || durable.len() == 4,
    );
    let seen: Vec<String> = everything(&volatile)
        .iter()
        .map(|m| m.data.clone())
        .collect();
    assert_eq!(
        seen,
        ["live"],
        "a volatile cache received history that was published before it existed"
    );
}

/// A cache with `Locality::Remote` ignores a publisher on its own session. A cache with the
/// default locality receives the same message.
///
/// Control in the same test: the default cache receives the message, so the empty cache is the
/// locality and not a publisher that never delivered.
#[test]
fn cache_honours_locality() {
    const TOPIC: &str = "/cache_parity_locality";
    let (_ctx, node) = isolated_node("cache_locality");
    let remote_only = node
        .create_cache::<RosString>(TOPIC, 10)
        .with_locality(Locality::Remote)
        .build()
        .expect("remote-only cache");
    let any = node
        .create_cache::<RosString>(TOPIC, 10)
        .build()
        .expect("default cache");
    let publisher = node
        .create_pub::<RosString>(TOPIC)
        .build()
        .expect("publisher");

    publisher.publish(&message("local")).expect("publish");

    wait_until("the default cache to receive the message", || {
        any.len() == 1
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        remote_only.len(),
        0,
        "a cache restricted to remote publishers received a local message"
    );
}

/// Publish one real message, forge sequence numbers 0, 1 and 5 on its key, and check that the
/// cache reports exactly the gap 2, 3, 4.
///
/// The cache is built by the caller, before the publisher, so each build path can run the same
/// scenario.
fn assert_cache_reports_gap(
    node: &hiroz::node::ZNode,
    topic: &str,
    cache: hiroz::cache::ZCache<RosString>,
) {
    let publisher = node
        .create_pub::<RosString>(&format!("/{topic}"))
        .build()
        .expect("publisher");
    let probe = node
        .session()
        .declare_subscriber(format!("**/{topic}/**"))
        .wait()
        .expect("probe");

    publisher.publish(&message("real")).expect("publish");
    let sample = probe
        .recv_timeout(Duration::from_secs(10))
        .expect("the probe receive")
        .expect("the probe sees the real publication");
    wait_until("the cache to store the real message", || cache.len() == 1);

    let gid = [9u8; 16];
    let put = |sn: i64| {
        node.session()
            .put(sample.key_expr().clone(), sample.payload().clone())
            .attachment(Attachment::new(sn, gid))
            .wait()
            .expect("forged put");
    };
    let lost = || {
        cache
            .events_mgr()
            .lock()
            .unwrap()
            .take_event_status(ZenohEventType::MessageLost)
            .total_count
    };
    put(0);
    put(1);
    wait_until(
        "the cache to store the baseline and the contiguous sample",
        || cache.len() == 3,
    );
    assert_eq!(
        lost(),
        0,
        "the baseline and a contiguous sample are not loss"
    );
    put(5);
    wait_until("the cache to store the sample after the gap", || {
        cache.len() == 4
    });
    assert_eq!(lost(), 3, "expected the gap 2, 3, 4 and no other loss");
}

/// A cache counts the messages that a gap in the sequence numbers of one publisher skips, and
/// reports them as a `MessageLost` event on its own event manager.
///
/// The samples are forged with sequence numbers 0, 1 and 5. The first is the baseline, the
/// second is contiguous, and the third skips 2, 3 and 4. Control in the same scenario: the count
/// is 0 after the contiguous sample, so the count of 3 is the gap and not a phantom loss.
#[test]
fn cache_reports_message_loss() {
    const TOPIC: &str = "cache_parity_loss";
    let (_ctx, node) = isolated_node("cache_loss");
    let cache = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .build()
        .expect("cache");
    assert_cache_reports_gap(&node, TOPIC, cache);
}

/// A cache that takes its timestamps from the messages reports the same loss as one that takes
/// them from Zenoh. The two build paths share a builder, and this test runs the loss scenario on
/// the extractor path.
#[test]
fn cache_with_a_stamp_extractor_reports_message_loss() {
    const TOPIC: &str = "cache_parity_stamp_loss";
    let (_ctx, node) = isolated_node("cache_stamp_loss");
    // Every message needs its own stamp, because the cache is keyed by stamp.
    let next = AtomicU64::new(0);
    let cache = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .with_stamp(move |_: &RosString| {
            UNIX_EPOCH + Duration::from_secs(next.fetch_add(1, Ordering::Relaxed))
        })
        .build()
        .expect("cache");
    assert_cache_reports_gap(&node, TOPIC, cache);
}

/// The locality of a cache restricts the buffer-group subscription as well as the topic one.
///
/// Both caches join the CPU buffer group. A plain put on the group key of the topic from the
/// cache's own session reaches the cache with the default locality and must not reach the cache
/// that accepts remote publishers only. Controls in the same test: the default cache stores that
/// put, and the restricted cache stores a put on the group key from another session, so the
/// empty cache is the locality and not a group key that nobody reads.
#[test]
fn cache_locality_covers_the_buffer_group_key() {
    const TOPIC: &str = "cache_parity_group_locality";
    let (_ctx_a, node, _ctx_b, node_b) = connected_nodes("cache_group_locality");
    let remote_only = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .with_buffer_group(BufferGroup::Cpu)
        .with_locality(Locality::Remote)
        .build()
        .expect("remote-only cache");
    let any = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .expect("default cache");
    let publisher = node
        .create_pub::<RosString>(&format!("/{TOPIC}"))
        .build()
        .expect("publisher");
    let probe = node
        .session()
        .declare_subscriber(format!("**/{TOPIC}/**"))
        .wait()
        .expect("probe");

    publisher.publish(&message("real")).expect("publish");
    let sample = probe
        .recv_timeout(Duration::from_secs(10))
        .expect("the probe receive")
        .expect("the probe sees the real publication");
    wait_until("the default cache to store the real message", || {
        any.len() == 1
    });

    let group_key = format!("{}/_buf_cpu", sample.key_expr());
    node.session()
        .put(group_key.clone(), sample.payload().clone())
        .wait()
        .expect("put on the group key");
    wait_until("the default cache to store the group-key put", || {
        any.len() == 2
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        remote_only.len(),
        0,
        "a cache restricted to remote publishers received a local message on the group key"
    );

    // The sessions need a moment to learn each other's declarations, so the remote put is sent
    // until the restricted cache has stored one.
    let deadline = Instant::now() + Duration::from_secs(10);
    while remote_only.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the restricted cache never stored a remote message on the group key"
        );
        node_b
            .session()
            .put(group_key.clone(), sample.payload().clone())
            .wait()
            .expect("remote put on the group key");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Run the remote-locality scenario for a cache built with `qos`.
///
/// A cache with `Locality::Remote` stores what a publisher on another session sends, and nothing
/// that a publisher on its own session sends.
///
/// Control in the same test: the cache with the default locality stores the local message, so the
/// absence of that message in the restricted cache is the locality. The restricted cache must
/// store a remote message, so it is not a cache that never receives anything.
fn assert_remote_locality_stores_remote_only(topic: &str, name: &str, qos: QosProfile) {
    let (_ctx_a, node_a, _ctx_b, node_b) = connected_nodes(name);
    let remote_only = node_a
        .create_cache::<RosString>(topic, 100)
        .with_qos(qos)
        .with_locality(Locality::Remote)
        .build()
        .expect("remote-only cache");
    let any = node_a
        .create_cache::<RosString>(topic, 100)
        .with_qos(qos)
        .build()
        .expect("default cache");
    let local_publisher = node_a
        .create_pub::<RosString>(topic)
        .build()
        .expect("local publisher");
    let remote_publisher = node_b
        .create_pub::<RosString>(topic)
        .build()
        .expect("remote publisher");

    // The sessions need a moment to learn each other's declarations, so the remote message is
    // sent until the restricted cache has stored one.
    let deadline = Instant::now() + Duration::from_secs(10);
    while remote_only.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the restricted cache never stored a remote message"
        );
        local_publisher.publish(&message("local")).expect("publish");
        remote_publisher
            .publish(&message("remote"))
            .expect("publish");
        std::thread::sleep(Duration::from_millis(200));
    }

    let everything = |cache: &hiroz::cache::ZCache<RosString>| {
        cache.get_interval(UNIX_EPOCH, SystemTime::now() + Duration::from_secs(86_400))
    };
    wait_until("the default cache to store a local message", || {
        everything(&any).iter().any(|m| m.data == "local")
    });
    let kept = everything(&remote_only);
    assert!(
        kept.iter().all(|m| m.data == "remote"),
        "a cache restricted to remote publishers stored a local message"
    );
}

/// A volatile cache with `Locality::Remote` stores remote messages only.
#[test]
fn cache_with_remote_locality_stores_remote_messages_only() {
    assert_remote_locality_stores_remote_only(
        "/cache_parity_remote_locality",
        "cache_remote_locality",
        QosProfile::default(),
    );
}

/// The same holds for a transient-local cache, which subscribes through a different build path
/// than the volatile one.
#[test]
fn transient_local_cache_with_remote_locality_stores_remote_messages_only() {
    assert_remote_locality_stores_remote_only(
        "/cache_parity_remote_locality_tl",
        "cache_remote_locality_tl",
        QosProfile {
            durability: QosDurability::TransientLocal,
            reliability: QosReliability::Reliable,
            history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
            ..Default::default()
        },
    );
}
