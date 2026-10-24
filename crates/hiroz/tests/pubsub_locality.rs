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
