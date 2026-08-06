//! `RMW_EVENT_MESSAGE_LOST` must actually be raised by a live subscriber.
//!
//! `event.rs`'s unit tests pin `MessageLossTracker`'s arithmetic. They say
//! nothing about whether anything *calls* it: delete the `observe_loss(..)` line
//! from the subscriber's receive path and every one of them still passes. This
//! file is the detector for that wiring.
//!
//! Loss is induced deterministically rather than by trying to drop a packet.
//! The subscriber's own key expression is published to directly, through the
//! node's zenoh session, with a hand-built [`Attachment`] carrying a chosen
//! sequence number — so the gap is exact and there is no timing to lose.

mod common;

use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{
    Builder, GidArray, TypeHash,
    attachment::Attachment,
    event::ZenohEventType,
    ros_msg::MessageTypeInfo,
};
use serde::{Deserialize, Serialize};
use serial_test::serial;
use zenoh::Wait;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Tick {
    counter: u64,
}

impl MessageTypeInfo for Tick {
    fn type_name() -> &'static str {
        "test_msgs::msg::dds_::Tick_"
    }
    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl hiroz::ros_msg::WithTypeInfo for Tick {}

impl hiroz::msg::ZMessage for Tick {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Tick>;
}

fn gid(n: u8) -> GidArray {
    let mut g = [0u8; 16];
    g[0] = n;
    g
}

/// A CDR-encoded `Tick`, ready to hand to `Session::put`.
fn payload(counter: u64) -> zenoh::bytes::ZBytes {
    use hiroz::msg::ZSerializer;
    let zbuf = <hiroz::msg::SerdeCdrSerdes<Tick>>::serialize_to_zbuf(&Tick { counter });
    zenoh::bytes::ZBytes::from(zbuf)
}

/// Wait until `total_count` for `MessageLost` stops changing, then return it.
fn settled_loss_count(sub_events: &Arc<Mutex<hiroz::event::EventsManager>>) -> i32 {
    let mut last = -1;
    let mut stable_since = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = sub_events
            .lock()
            .unwrap()
            .take_event_status(ZenohEventType::MessageLost)
            .total_count;
        if now != last {
            last = now;
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= Duration::from_millis(400) {
            return now;
        }
        assert!(Instant::now() < deadline, "loss count never settled");
        thread::sleep(Duration::from_millis(25));
    }
}

/// A gap in a publisher's sequence numbers raises `MessageLost` on the
/// subscriber that saw it, with the count of samples that never arrived.
#[test]
#[serial]
fn a_sequence_gap_raises_message_lost() {
    const TOPIC: &str = "/message_lost_gap";

    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("context");
    let node = ctx.create_node("message_lost_node").build().expect("node");

    let received = Arc::new(Mutex::new(Vec::<u64>::new()));
    let cb_received = received.clone();
    let sub = node
        .create_sub::<Tick>(TOPIC)
        .build_with_callback(move |msg: Tick| {
            cb_received.lock().unwrap().push(msg.counter);
        })
        .expect("subscriber");

    // Publish straight onto the subscriber's own key expression, so the
    // sequence numbers are ours to choose.
    let ke = node
        .keyexpr_format()
        .topic_key_expr(sub.entity())
        .expect("topic key expr");
    let session = node.session();
    let publisher_gid = gid(42);

    let put = |sn: i64, counter: u64| {
        session
            .put((*ke).clone(), payload(counter))
            .attachment(Attachment::new(sn, publisher_gid))
            .wait()
            .expect("put");
    };

    thread::sleep(Duration::from_millis(300));

    put(0, 0); // baseline — first from this publisher, never counted
    put(1, 1); // contiguous
    put(5, 5); // 2, 3 and 4 never arrived

    let lost = settled_loss_count(sub.events_mgr());

    assert_eq!(
        lost, 3,
        "expected the three skipped sequence numbers to be reported as lost; \
         got {lost}. Zero means the receive path never fed the loss tracker"
    );
    assert_eq!(
        received.lock().unwrap().len(),
        3,
        "all three published samples should still have been delivered — \
         detecting loss must not drop anything"
    );
}

/// A subscriber that joins late has not "lost" the history it was never sent.
///
/// Without the first-sample exemption this reports the publisher's sequence
/// number as the loss count, so every late joiner looks catastrophically lossy.
#[test]
#[serial]
fn joining_late_reports_no_loss() {
    const TOPIC: &str = "/message_lost_late_join";

    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("context");
    let node = ctx
        .create_node("message_lost_late_node")
        .build()
        .expect("node");

    let sub = node
        .create_sub::<Tick>(TOPIC)
        .build_with_callback(|_msg: Tick| {})
        .expect("subscriber");

    let ke = node
        .keyexpr_format()
        .topic_key_expr(sub.entity())
        .expect("topic key expr");
    let session = node.session();

    thread::sleep(Duration::from_millis(300));

    // First sample this subscriber ever sees from this publisher, and it is
    // already well into the publisher's stream.
    session
        .put((*ke).clone(), payload(9000))
        .attachment(Attachment::new(9000, gid(7)))
        .wait()
        .expect("put");

    let lost = settled_loss_count(sub.events_mgr());

    assert_eq!(
        lost, 0,
        "a late joiner must not be charged for history it was never sent"
    );
}
