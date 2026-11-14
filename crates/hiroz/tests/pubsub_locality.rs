//! `wait_for_subscription` counts only the subscribers the publisher's locality admits.

use std::time::Duration;

mod common;

use common::{connected_nodes, isolated_node};
use hiroz::Builder;
use hiroz_msgs::std_msgs::String as RosString;
use zenoh::sample::Locality;

/// `wait_for_subscription` counts only the subscribers the publisher's locality admits.
///
/// One session holds a subscriber and a `Locality::Remote` publisher. The publisher delivers to
/// nobody, so it must not report a subscriber as ready. Control in the same test: an unrestricted
/// publisher on the same topic reports the subscriber ready, so a graph that never shows the
/// subscriber fails the control.
#[test]
fn wait_for_subscription_ignores_subscribers_the_locality_excludes() {
    const TOPIC: &str = "/pubsub_locality_wait";
    let (_ctx, node) = isolated_node("pubsub_locality_wait");
    let _subscriber = node
        .create_sub::<RosString>(TOPIC)
        .build()
        .expect("subscriber");
    let restricted = node
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::Remote)
        .build()
        .expect("restricted publisher");
    let control = node
        .create_pub::<RosString>(TOPIC)
        .build()
        .expect("publisher");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert!(
        rt.block_on(control.wait_for_subscription(1, Duration::from_secs(10))),
        "the unrestricted publisher never saw the local subscriber"
    );
    assert!(
        !rt.block_on(restricted.wait_for_subscription(1, Duration::from_millis(500))),
        "a publisher restricted to remote subscribers reported a local subscriber as ready"
    );
}

/// With a subscriber on another session only, a `Locality::Remote` publisher reports ready and a
/// `Locality::SessionLocal` publisher does not.
///
/// The remote publisher's wait comes first and must succeed. That proves the remote subscriber is
/// in the graph before the session-local publisher's negative wait.
#[test]
fn wait_for_subscription_follows_the_locality_across_sessions() {
    const TOPIC: &str = "/pubsub_locality_wait_remote";
    let (_ctx_a, node_a, _ctx_b, node_b) = connected_nodes("locality_wait");

    let _remote_subscriber = node_b
        .create_sub::<RosString>(TOPIC)
        .build()
        .expect("remote subscriber");
    let remote = node_a
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::Remote)
        .build()
        .expect("remote publisher");
    let session_local = node_a
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::SessionLocal)
        .build()
        .expect("session-local publisher");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert!(
        rt.block_on(remote.wait_for_subscription(1, Duration::from_secs(10))),
        "the remote publisher never saw the subscriber on the other session"
    );
    assert!(
        !rt.block_on(session_local.wait_for_subscription(1, Duration::from_millis(500))),
        "a session-local publisher reported a subscriber on another session as ready"
    );
}

/// With a subscriber on the publisher's own session only, a `Locality::SessionLocal` publisher
/// reports ready.
///
/// This is the positive case of the session-local arm. The negative waits elsewhere in this file
/// cannot tell that arm from one that always counts nothing.
#[test]
fn wait_for_subscription_counts_a_same_session_subscriber_for_session_local() {
    const TOPIC: &str = "/pubsub_locality_wait_session_local";
    let (_ctx, node) = isolated_node("pubsub_locality_wait_sl");
    let _subscriber = node
        .create_sub::<RosString>(TOPIC)
        .build()
        .expect("subscriber");
    let session_local = node
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::SessionLocal)
        .build()
        .expect("session-local publisher");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert!(
        rt.block_on(session_local.wait_for_subscription(1, Duration::from_secs(10))),
        "a session-local publisher never saw the subscriber on its own session"
    );
}
