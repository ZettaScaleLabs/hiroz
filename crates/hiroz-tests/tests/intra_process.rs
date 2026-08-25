//! Intra-process `Arc<T>` delivery — does it really skip the serialization?
//!
//! # The assertion that matters is `Arc::ptr_eq`
//!
//! "The message arrived and its contents match" is satisfied by a CDR round
//! trip just as well as by a pointer move, so it proves nothing about which
//! path ran. Pointer identity between the `Arc` that was published and the
//! `Arc` that was received cannot survive a serialization boundary: encoding
//! and decoding necessarily produces a new allocation. That is the property
//! under test.
//!
//! # What each test pins
//!
//! | test | property |
//! |---|---|
//! | `same_arc_reaches_a_same_session_subscriber` | pointer identity — no encode, no copy |
//! | `intra_process_only_publisher_does_not_reach_another_context` | nothing goes on the wire, with a control proving the wire works |
//! | `a_different_rust_type_on_the_same_topic_is_not_delivered` | the `TypeId` gate holds |
//! | `dropping_the_subscriber_unregisters_it` | no delivery into a dead subscriber |

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use common::*;
use hiroz::Builder;
use hiroz_msgs::std_msgs::{Int32, String as RosString};

const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

fn wait_until(f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < DELIVERY_DEADLINE {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    f()
}

#[test]
fn same_arc_reaches_a_same_session_subscriber() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx.create_node("zc_tx").build()?;
    let node_rx = ctx.create_node("zc_rx").build()?;

    let received: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let _sub = node_rx
        .create_sub::<RosString>("shared")
        .build_with_shared_callback(move |msg: Arc<RosString>| {
            sink.lock().expect("poisoned").push(msg);
        })?;

    let publisher = node_tx
        .create_pub::<RosString>("shared")
        .with_intra_process_only()
        .build()?;

    let sent = Arc::new(RosString {
        data: "no copies please".to_owned(),
    });
    let delivered = publisher.publish_shared(sent.clone())?;
    assert_eq!(delivered, 1, "expected exactly one local subscriber");

    // Delivery is synchronous on this thread, so no waiting is needed — but
    // assert on the count first so a failure says "nothing arrived" rather than
    // panicking on an empty vec.
    let got = received.lock().expect("poisoned");
    assert_eq!(got.len(), 1, "subscriber did not receive the message");
    assert!(
        Arc::ptr_eq(&sent, &got[0]),
        "received a different allocation — the message went through a copy or a \
         serialization round trip, which is exactly what this path exists to avoid"
    );
    Ok(())
}

#[test]
fn intra_process_only_publisher_does_not_reach_another_context() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_tx = create_hiroz_context_with_router(&router)?;
    let ctx_other = create_hiroz_context_with_router(&router)?;

    let node_tx = ctx_tx.create_node("zc_tx").build()?;
    let node_other = ctx_other.create_node("zc_other").build()?;

    let local_hits = Arc::new(AtomicUsize::new(0));
    let control_hits = Arc::new(AtomicUsize::new(0));
    let (c1, c2) = (local_hits.clone(), control_hits.clone());

    let _s1 = node_other
        .create_sub::<RosString>("zc_local")
        .build_with_callback(move |_m: RosString| {
            c1.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s2 = node_other
        .create_sub::<RosString>("zc_control")
        .build_with_callback(move |_m: RosString| {
            c2.fetch_add(1, Ordering::SeqCst);
        })?;

    let pub_local = node_tx
        .create_pub::<RosString>("zc_local")
        .with_intra_process_only()
        .build()?;
    let pub_control = node_tx.create_pub::<RosString>("zc_control").build()?;

    wait_for_ready(Duration::from_millis(500));

    let msg = Arc::new(RosString {
        data: "hello".to_owned(),
    });
    for _ in 0..5 {
        pub_local.publish_shared(msg.clone())?;
        pub_control.publish(&msg)?;
    }

    // The control must cross. Without it, a zero on `zc_local` would be
    // indistinguishable from a broken router or a wrong topic.
    assert!(
        wait_until(|| control_hits.load(Ordering::SeqCst) >= 5),
        "control publisher did not reach the other context ({} of 5) — this run \
         proves nothing about the intra-process path",
        control_hits.load(Ordering::SeqCst)
    );
    assert_eq!(
        local_hits.load(Ordering::SeqCst),
        0,
        "an intra-process-only publisher put something on the wire"
    );
    Ok(())
}

#[test]
fn a_different_rust_type_on_the_same_topic_is_not_delivered() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("zc_types").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<Int32>("typed")
        .build_with_shared_callback(move |_m: Arc<Int32>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    // Same topic, different concrete Rust type.
    let publisher = node
        .create_pub::<RosString>("typed")
        .with_intra_process_only()
        .build()?;
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "wrong type".to_owned(),
    }))?;

    assert_eq!(delivered, 0, "delivered across a type mismatch");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "an Int32 subscriber was handed a String — the TypeId gate is not holding"
    );
    Ok(())
}

#[test]
fn dropping_the_subscriber_unregisters_it() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("zc_drop").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let sub = node
        .create_sub::<RosString>("dropme")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("dropme")
        .with_intra_process_only()
        .build()?;
    let msg = Arc::new(RosString {
        data: "x".to_owned(),
    });

    // Positive control: it is registered right now. Without this the assertion
    // below would pass just as well against a subscriber that never worked.
    assert_eq!(publisher.publish_shared(msg.clone())?, 1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    drop(sub);

    assert_eq!(
        publisher.publish_shared(msg)?,
        0,
        "the bus still holds a registration for a dropped subscriber"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "a dropped subscriber was called");
    Ok(())
}
