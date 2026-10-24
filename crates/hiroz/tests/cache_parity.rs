//! A cache applies the same subscriber checks as a plain subscriber.
//!
//! Each test builds its entities on one session with no router and no peer. A control in the same
//! test shows that the checked behaviour differs from the default, so a green result cannot come
//! from a cache that never receives anything.

use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use hiroz::{
    Builder,
    attachment::Attachment,
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
