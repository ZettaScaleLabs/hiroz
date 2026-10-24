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
    context::{ZContext, ZContextBuilder},
    node::ZNode,
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};
use hiroz_msgs::std_msgs::String as RosString;

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
