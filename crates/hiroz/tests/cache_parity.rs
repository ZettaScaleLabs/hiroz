//! A cache applies the same subscriber checks as a plain subscriber.
//!
//! Each test builds its entities on one session with no router and no peer. A control in the same
//! test shows that the checked behaviour differs from the default, so a green result cannot come
//! from a cache that never receives anything.

use std::{
    net::TcpListener,
    num::NonZeroUsize,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hiroz::{
    BufferGroup, Builder,
    attachment::Attachment,
    cache::ZCache,
    context::{ZContext, ZContextBuilder},
    event::ZenohEventType,
    node::ZNode,
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};
use hiroz_msgs::std_msgs::String as RosString;
use zenoh::{Wait, sample::Locality};

/// A node on a session that has no router and no peer.
fn isolated_node(name: &str) -> (ZContext, ZNode) {
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx.create_node(name).build().expect("node");
    (ctx, node)
}

/// Poll `condition` until it holds. The test fails with `what` after ten seconds.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Poll until the cache holds `len` messages. On a timeout the failure names what the cache holds.
fn wait_for_len(what: &str, cache: &ZCache<RosString>, len: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.len() != len {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: the cache holds {} of {len} messages, oldest stamp {:?}, newest stamp {:?}",
            cache.len(),
            cache.oldest_stamp(),
            cache.newest_stamp(),
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn message(text: &str) -> RosString {
    RosString {
        data: text.to_string(),
    }
}

/// A cache built with a transient-local QoS replays the history of a publisher that started
/// earlier. A cache with the default QoS does not.
///
/// Control in the same test: the volatile cache sees none of the three messages that the
/// transient-local cache received, so the replay comes from the durability and not from a late
/// delivery.
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
    assert_eq!(
        volatile.len(),
        0,
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

/// A cache counts the messages that a gap in the sequence numbers of one publisher skips, and
/// reports them as a `MessageLost` event on its own event manager.
///
/// The test learns the topic key and a valid payload from one real publication, then puts three
/// samples with a forged attachment: sequence numbers 0, 1 and 5. The first is the baseline, the
/// second is contiguous, and the third skips 2, 3 and 4. Control in the same test: the baseline
/// and the contiguous sample add nothing, so the count of 3 is the gap and not a phantom loss.
#[test]
fn cache_reports_message_loss() {
    const TOPIC: &str = "cache_parity_loss";
    let (_ctx, node) = isolated_node("cache_loss");
    let cache = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .build()
        .expect("cache");
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
    wait_until("the cache to store the real message", || cache.len() == 1);

    let gid = [9u8; 16];
    for sn in [0i64, 1, 5] {
        node.session()
            .put(sample.key_expr().clone(), sample.payload().clone())
            .attachment(Attachment::new(sn, gid))
            .wait()
            .expect("forged put");
    }
    wait_until("the cache to store the forged messages", || {
        cache.len() == 4
    });

    let status = cache
        .events_mgr()
        .lock()
        .unwrap()
        .take_event_status(ZenohEventType::MessageLost);
    assert_eq!(
        status.total_count, 3,
        "expected the gap 2, 3, 4 and no other loss"
    );
}

/// A cache keeps its message count and its loss baselines within bounds under publisher churn.
///
/// The test puts one sample for each of 1100 forged publisher GIDs into a cache of capacity 2. That
/// is more than the loss tracker keeps. The first GID has lost its baseline by then, so its next
/// sample, far ahead of the first, is a new baseline and raises no loss. Control in the same
/// test: a sample that skips 3 and 4 from the most recent GID, whose baseline is kept, raises a
/// loss of 2.
#[test]
fn cache_loss_tracking_survives_publisher_churn() {
    const TOPIC: &str = "cache_parity_loss_churn";
    const PUBLISHERS: u32 = 1100;
    let (_ctx, node) = isolated_node("cache_loss_churn");
    let cache = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 2)
        .build()
        .expect("cache");
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
    wait_until("the cache to store the real message", || cache.len() == 1);

    let gid_for = |n: u32| {
        let mut gid = [7u8; 16];
        gid[..4].copy_from_slice(&n.to_le_bytes());
        gid
    };
    let put = |sn: i64, gid: [u8; 16]| {
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
    for n in 0..PUBLISHERS {
        put(0, gid_for(n));
    }
    // Puts on one session arrive in order, so the last one is the last to be stored.
    put(1, gid_for(PUBLISHERS - 1));
    std::thread::sleep(Duration::from_secs(1));
    assert!(cache.len() <= 2, "the cache holds {} messages", cache.len());
    assert_eq!(lost(), 0, "churn alone is not loss");

    put(1_000, gid_for(0));
    put(4, gid_for(PUBLISHERS - 1));
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        lost(),
        2,
        "the evicted GID starts a new baseline, and the kept GID reports its gap"
    );
}

/// Two peer sessions on one machine: the second connects to the first.
fn connected_nodes(name: &str) -> (ZContext, ZNode, ZContext, ZNode) {
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port();
    let endpoint = format!("tcp/127.0.0.1:{port}");
    let ctx_a = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints([endpoint.clone()])
        .build()
        .expect("context a");
    let node_a = ctx_a
        .create_node(format!("{name}_a"))
        .build()
        .expect("node a");
    let ctx_b = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context b");
    let node_b = ctx_b
        .create_node(format!("{name}_b"))
        .build()
        .expect("node b");
    (ctx_a, node_a, ctx_b, node_b)
}

/// A volatile cache stores a sample whose timestamp is older than one it already stored.
///
/// Two plain puts carry timestamps from one clock, the newer one first. A cache that filters
/// samples by timestamp would drop the second put. Control in the same test: the first message,
/// from a real publisher, is stored before the puts, so a missing sample is a drop and not a
/// cache that never received anything. The cache is cleared after the control, so the two puts
/// are counted alone.
#[test]
fn volatile_cache_keeps_reordered_timestamps() {
    const TOPIC: &str = "cache_parity_reorder";
    let (_ctx, node) = isolated_node("cache_reorder");
    let cache = node
        .create_cache::<RosString>(&format!("/{TOPIC}"), 10)
        .build()
        .expect("cache");
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
    wait_until("the cache to store the real message", || cache.len() == 1);

    // The control only shows that the cache receives. Clearing it leaves the injected pair alone,
    // so the count cannot depend on whether a key of the pair equals the key of the control.
    cache.clear();

    // The cache indexes a message by its timestamp as a `SystemTime`, which has no room for the
    // logical counter of the clock. Two timestamps from one clock tick differ only in that counter
    // and index the same entry. A coarse system clock produces such a pair, so the test draws
    // `newer` until the two timestamps differ as a `SystemTime`.
    let older = node.session().new_timestamp();
    let tick_deadline = Instant::now() + Duration::from_secs(10);
    let newer = loop {
        let candidate = node.session().new_timestamp();
        if candidate.get_time().to_system_time() > older.get_time().to_system_time() {
            break candidate;
        }
        assert!(
            Instant::now() < tick_deadline,
            "the clock never produced a timestamp after {older}"
        );
    };
    for timestamp in [newer, older] {
        node.session()
            .put(sample.key_expr().clone(), sample.payload().clone())
            .timestamp(timestamp)
            .wait()
            .expect("timestamped put");
    }
    wait_for_len("the cache to store both timestamped samples", &cache, 2);
}

/// The locality of a cache restricts the buffer-group subscription as well as the topic one.
///
/// Both caches join the CPU buffer group. A plain put on the group key of the topic reaches the
/// cache with the default locality and must not reach the cache that accepts remote publishers
/// only. Control in the same test: the default cache stores the put, so the empty cache is the
/// locality and not a group key that nobody reads.
#[test]
fn cache_locality_covers_the_buffer_group_key() {
    const TOPIC: &str = "cache_parity_group_locality";
    let (_ctx, node) = isolated_node("cache_group_locality");
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
        .put(group_key, sample.payload().clone())
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
}

/// A cache with `Locality::Remote` stores what a publisher on another session sends, and nothing
/// that a publisher on its own session sends.
///
/// Control in the same test: the cache with the default locality stores the local message, so the
/// absence of that message in the restricted cache is the locality. The restricted cache must
/// store a remote message, so it is not a cache that never receives anything.
#[test]
fn cache_with_remote_locality_stores_remote_messages_only() {
    const TOPIC: &str = "/cache_parity_remote_locality";
    let (_ctx_a, node_a, _ctx_b, node_b) = connected_nodes("cache_remote_locality");
    let remote_only = node_a
        .create_cache::<RosString>(TOPIC, 100)
        .with_locality(Locality::Remote)
        .build()
        .expect("remote-only cache");
    let any = node_a
        .create_cache::<RosString>(TOPIC, 100)
        .build()
        .expect("default cache");
    let local_publisher = node_a
        .create_pub::<RosString>(TOPIC)
        .build()
        .expect("local publisher");
    let remote_publisher = node_b
        .create_pub::<RosString>(TOPIC)
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
    let kept = everything(&remote_only);
    assert!(
        kept.iter().all(|m| m.data == "remote"),
        "a cache restricted to remote publishers stored a local message"
    );
    wait_until("the default cache to store a local message", || {
        everything(&any).iter().any(|m| m.data == "local")
    });
}
