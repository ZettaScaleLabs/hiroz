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
//! | `a_pooled_payload_buffer_is_written_in_place_and_reused` | one buffer serves many sends |

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
use hiroz::{Builder, ZBuf};
use hiroz_msgs::std_msgs::{ByteMultiArray, Int32, String as RosString};
use zenoh_buffers::buffer::SplitBuffer;

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

/// One payload buffer, reused across sends, written in place.
///
/// Delivering an `Arc<T>` without serializing still leaves the publisher
/// allocating a payload buffer per message. This is the test for reusing one
/// instead. Two things must both hold, and neither implies the other:
///
/// - the **payload allocation** must not move between sends, or the buffer was
///   silently reallocated and nothing was reused;
/// - the subscriber must see the value written for **that** send, or the
///   in-place write did not reach the receiver.
///
/// The `Arc::get_mut` on each iteration is itself load bearing: it succeeds only
/// because no one still holds the previous message. That is the invariant a
/// pool has to respect, so a change that leaks a reference fails here.
#[test]
fn a_pooled_payload_buffer_is_written_in_place_and_reused() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("zc_pool").build()?;

    // (address of the received Arc, address of its payload bytes, the stamp)
    let seen: Arc<Mutex<Vec<(usize, usize, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = node
        .create_sub::<ByteMultiArray>("pooled")
        .build_with_shared_callback(move |msg: Arc<ByteMultiArray>| {
            let bytes = msg.data.contiguous();
            let stamp = u64::from_le_bytes(bytes[0..8].try_into().expect("8 bytes"));
            sink.lock()
                .expect("poisoned")
                .push((Arc::as_ptr(&msg) as usize, bytes.as_ptr() as usize, stamp));
        })?;

    let publisher = node
        .create_pub::<ByteMultiArray>("pooled")
        .with_intra_process_only()
        .build()?;

    let mut slot = Arc::new(ByteMultiArray {
        data: ZBuf::from(vec![0xAAu8; 64]),
        ..Default::default()
    });

    let mut sent_payload_addrs = Vec::new();
    for stamp in [1u64, 2, 3] {
        let payload = Arc::get_mut(&mut slot)
            .expect("nobody still holds the previous message")
            .data
            .as_mut_slice()
            .expect("a solely owned single-slice buffer must be writable in place");
        sent_payload_addrs.push(payload.as_ptr() as usize);
        payload[0..8].copy_from_slice(&stamp.to_le_bytes());

        assert_eq!(publisher.publish_shared(slot.clone())?, 1);
    }

    let got = seen.lock().expect("poisoned");
    assert_eq!(got.len(), 3, "not every send arrived");

    assert!(
        sent_payload_addrs.windows(2).all(|w| w[0] == w[1]),
        "the payload buffer moved between sends ({sent_payload_addrs:?}) — it was \
         reallocated, so nothing was reused"
    );
    let slot_addr = Arc::as_ptr(&slot) as usize;
    for (i, (arc_addr, payload_addr, stamp)) in got.iter().enumerate() {
        assert_eq!(*arc_addr, slot_addr, "send {i} delivered a different allocation");
        assert_eq!(
            *payload_addr, sent_payload_addrs[i],
            "send {i} delivered a different payload buffer"
        );
        assert_eq!(*stamp, i as u64 + 1, "send {i} carried a stale value");
    }
    Ok(())
}
