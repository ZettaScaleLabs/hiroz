//! A cache warns about an encoding mismatch like a plain subscriber.
//!
//! The check only logs, so the test installs a global `tracing` subscriber that records the
//! messages of every event. The file holds one test because the subscriber is global to the
//! process, and zenoh runs the sample callbacks on its own threads.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hiroz::{Builder, context::ZContextBuilder, encoding::Encoding};
use hiroz_msgs::std_msgs::String as RosString;

/// Records the message text of every event.
struct Capture(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0.lock().unwrap().push(visitor.0);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

fn mismatches(log: &Arc<Mutex<Vec<String>>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|line| line.contains("Encoding mismatch"))
        .count()
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A CDR publisher and a consumer that expects Protobuf produce one warning per received message.
///
/// Control in the same test: the first phase uses a plain subscriber, which already checks the
/// encoding. It shows that the capture sees the warning. The second phase replaces the subscriber
/// with a cache, and the count must rise by one more.
#[test]
fn cache_warns_on_encoding_mismatch() {
    const TOPIC: &str = "/cache_parity_encoding";
    let log = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(Capture(log.clone())).expect("global subscriber");

    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx.create_node("cache_encoding").build().expect("node");
    let publisher = node
        .create_pub::<RosString>(TOPIC)
        .with_encoding(Encoding::cdr())
        .build()
        .expect("publisher");
    let message = RosString {
        data: "payload".to_string(),
    };

    let subscriber = node
        .create_sub::<RosString>(TOPIC)
        .with_encoding(Encoding::protobuf())
        .build()
        .expect("subscriber");
    publisher.publish(&message).expect("publish");
    wait_until("the subscriber warning", || mismatches(&log) == 1);
    drop(subscriber);

    let cache = node
        .create_cache::<RosString>(TOPIC, 10)
        .with_encoding(Encoding::protobuf())
        .build()
        .expect("cache");
    publisher.publish(&message).expect("publish");
    wait_until("the cache warning", || mismatches(&log) == 2);
    wait_until("the cache to keep the message", || cache.len() == 1);
}
