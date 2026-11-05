//! A publisher honours its locality option like a subscriber does.

use std::time::Duration;

use hiroz::{Builder, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;
use zenoh::sample::Locality;

fn message(text: &str) -> RosString {
    RosString {
        data: text.to_string(),
    }
}

/// A publisher with `Locality::Remote` does not deliver to a subscriber on its own session. A
/// publisher with the default locality does.
///
/// Control in the same test: the restricted publisher sends first and the default publisher sends
/// second. The subscriber must receive the second message first, and nothing after it. A
/// subscriber that never received anything would fail the first receive.
#[test]
fn publisher_honours_locality() {
    const TOPIC: &str = "/pubsub_locality";
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx.create_node("pubsub_locality").build().expect("node");
    let subscriber = node
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

    restricted.publish(&message("restricted")).expect("publish");
    control.publish(&message("control")).expect("publish");

    let first = subscriber
        .recv_timeout(Duration::from_secs(10))
        .expect("the default publisher delivers");
    assert_eq!(
        first.data, "control",
        "a publisher restricted to remote subscribers delivered to a local one"
    );
    assert!(
        subscriber.recv_timeout(Duration::from_millis(500)).is_err(),
        "the restricted publisher delivered after the control message"
    );
}

/// A publisher with `Locality::Remote` delivers to a subscriber on another session and not to one
/// on its own session.
///
/// Control in the same test: the remote subscriber must receive the restricted publisher's
/// message, so a publisher that suppresses every message once a locality is set fails. The local
/// subscriber first receives a message from an unrestricted publisher, so it is live, and then
/// must receive nothing.
#[test]
fn remote_publisher_reaches_a_remote_subscriber_only() {
    const TOPIC: &str = "/pubsub_locality_remote";
    let port = std::net::TcpListener::bind("127.0.0.1:0")
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
    let node_a = ctx_a.create_node("locality_a").build().expect("node a");
    let ctx_b = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context b");
    let node_b = ctx_b.create_node("locality_b").build().expect("node b");

    let local_subscriber = node_a
        .create_sub::<RosString>(TOPIC)
        .build()
        .expect("local subscriber");
    let remote_subscriber = node_b
        .create_sub::<RosString>(TOPIC)
        .build()
        .expect("remote subscriber");
    let restricted = node_a
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::Remote)
        .build()
        .expect("restricted publisher");
    let control = node_a
        .create_pub::<RosString>(TOPIC)
        .with_locality(Locality::SessionLocal)
        .build()
        .expect("session-local publisher");

    // The sessions need a moment to learn each other's declarations, so the restricted publisher
    // sends until the remote subscriber has received one message.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let received = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the remote subscriber never received the restricted publisher's message"
        );
        restricted.publish(&message("restricted")).expect("publish");
        if let Ok(received) = remote_subscriber.recv_timeout(Duration::from_millis(200)) {
            break received;
        }
    };
    assert_eq!(received.data, "restricted");

    control.publish(&message("control")).expect("publish");
    let first = local_subscriber
        .recv_timeout(Duration::from_secs(10))
        .expect("the session-local publisher delivers");
    assert_eq!(
        first.data, "control",
        "a publisher restricted to remote subscribers delivered to a local one"
    );
    assert!(
        local_subscriber
            .recv_timeout(Duration::from_millis(500))
            .is_err(),
        "the restricted publisher delivered to a subscriber on its own session"
    );
}

/// `wait_for_subscription` counts only the subscribers the publisher's locality admits.
///
/// One session holds a subscriber and a `Locality::Remote` publisher. The publisher delivers to
/// nobody, so it must not report a subscriber as ready. Control in the same test: an unrestricted
/// publisher on the same topic reports the subscriber ready, so a graph that never shows the
/// subscriber fails the control.
#[test]
fn wait_for_subscription_ignores_subscribers_the_locality_excludes() {
    const TOPIC: &str = "/pubsub_locality_wait";
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx
        .create_node("pubsub_locality_wait")
        .build()
        .expect("node");
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
    let port = std::net::TcpListener::bind("127.0.0.1:0")
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
        .create_node("locality_wait_a")
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
        .create_node("locality_wait_b")
        .build()
        .expect("node b");

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
