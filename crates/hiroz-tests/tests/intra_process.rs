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
//! | `a_plain_publish_reaches_a_shared_callback_subscriber` | #39 — no silent same-session loss |
//! | `publish_shared_without_a_locality_restriction_arrives_once` | #39 — and no duplicate either |
//! | `a_self_publishing_callback_does_not_recurse_without_bound` | #40 — a cycle is refused, not fatal |
//! | `a_publisher_asks_the_graph_instead_of_being_told` | #36 — no drop when nobody is on the bus |
//! | `the_wire_is_used_when_a_subscriber_is_off_session` | #36 — and the bus is not, so no duplicate |
//! | `a_sole_receiver_is_given_the_message_to_own` | #36 — the move, not a shared Arc |

mod common;

use std::{
    sync::{
        Arc, Mutex, OnceLock,
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

/// Ceiling on the self-publishing echo, so neither direction of the
/// recursion test can run forever. Far above the delivery-depth bound.
const ECHO_CAP: usize = 64;

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

/// #39 — an ordinary publisher must reach a shared-callback subscriber.
///
/// The bus subscriber's wire half used to be forced to `Locality::Remote`,
/// which silently discarded every same-session `publish`. Nothing errored and
/// the graph showed both endpoints matched, so only an assertion on delivery
/// can catch it.
#[test]
fn a_plain_publish_reaches_a_shared_callback_subscriber() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx.create_node("plain_tx").build()?;
    let node_rx = ctx.create_node("plain_rx").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node_rx
        .create_sub::<RosString>("plain_to_shared")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    // An ordinary publisher: no locality, no bus involvement, nothing special.
    let publisher = node_tx.create_pub::<RosString>("plain_to_shared").build()?;
    wait_for_ready(Duration::from_millis(500));

    publisher.publish(&RosString {
        data: "over the wire".to_owned(),
    })?;

    assert!(
        wait_until(|| hits.load(Ordering::SeqCst) >= 1),
        "a same-session plain publish never reached the shared-callback subscriber"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "delivered more than once — the wire and the bus both fired"
    );
    Ok(())
}

/// #39 — and the duplicate the old filter existed to prevent must stay prevented.
///
/// A `publish_shared` on a publisher with no locality restriction reaches the
/// subscriber over the wire. It must not *also* arrive over the bus.
#[test]
fn publish_shared_without_a_locality_restriction_arrives_once() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx.create_node("once_tx").build()?;
    let node_rx = ctx.create_node("once_rx").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let seen: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));
    let h = hits.clone();
    let sink = seen.clone();
    let _sub = node_rx
        .create_sub::<RosString>("exactly_once")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
            sink.lock().expect("poisoned").push(m);
        })?;

    // A plain publisher: no locality, no flag, nothing telling it what to do.
    let publisher = node_tx.create_pub::<RosString>("exactly_once").build()?;
    wait_for_ready(Duration::from_millis(500));

    let sent = Arc::new(RosString {
        data: "once please".to_owned(),
    });
    let delivered = publisher.publish_shared(sent.clone())?;
    assert_eq!(
        delivered, 1,
        "a plain publisher took the wire even though every subscriber is on the bus"
    );

    assert!(
        wait_until(|| hits.load(Ordering::SeqCst) >= 1),
        "the message did not arrive at all"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "delivered twice — once over the bus and once over the wire"
    );
    let got = seen.lock().expect("poisoned");
    assert!(
        Arc::ptr_eq(&sent, &got[0]),
        "a different allocation arrived: the wire carried this, not the bus"
    );
    Ok(())
}

/// #40 — a callback that publishes onto its own topic must not recurse forever.
///
/// Delivery is inline on the publishing thread, so this is direct recursion.
/// On the wire the same shape is an endless stream of messages, which is
/// observable and survivable; here it used to end in a stack overflow. The bus
/// now refuses past a fixed depth, so the chain terminates and the test
/// returns rather than dying with SIGSEGV.
#[test]
fn a_self_publishing_callback_does_not_recurse_without_bound() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("echo_node").build()?;

    let publisher = Arc::new(
        node.create_pub::<RosString>("echo")
            .with_intra_process_only()
            .build()?,
    );
    let echo = publisher.clone();

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("echo")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
            // Republish onto the very topic this callback serves.
            let _ = echo.publish_shared(m);
        })?;

    publisher.publish_shared(Arc::new(RosString {
        data: "round and round".to_owned(),
    }))?;

    let seen = hits.load(Ordering::SeqCst);
    assert!(seen >= 1, "the callback never ran");
    assert!(
        seen <= 16,
        "delivery recursed {seen} deep — the depth guard did not hold"
    );
    Ok(())
}

/// #36 — a publisher without the flag must read its audience off the graph.
///
/// The prototype was *told* whether to use the wire, so `publish_shared` on a
/// publisher whose subscriber was an ordinary one delivered nothing at all.
/// Now the publisher asks: no bus subscriber means the wire carries it.
#[test]
fn a_publisher_asks_the_graph_instead_of_being_told() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx.create_node("ask_tx").build()?;
    let node_rx = ctx.create_node("ask_rx").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    // An ORDINARY subscriber: not on the bus, expects decoded bytes.
    let _sub = node_rx
        .create_sub::<RosString>("ask_graph")
        .build_with_callback(move |_m: RosString| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    // No locality, no flag. The publisher has to work out the audience.
    let publisher = node_tx.create_pub::<RosString>("ask_graph").build()?;
    wait_for_ready(Duration::from_millis(500));

    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "who is listening".to_owned(),
    }))?;
    assert_eq!(delivered, 0, "the bus took a message its subscriber cannot decode");

    assert!(
        wait_until(|| hits.load(Ordering::SeqCst) >= 1),
        "publish_shared reached nobody: the publisher neither used the bus nor the wire"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "delivered more than once");
    Ok(())
}

/// #36 — an off-session subscriber forces the wire, and then the bus stays out.
///
/// Both paths at once would deliver twice to a same-session bus subscriber.
/// The publisher takes the wire alone, so the local subscriber gets exactly one
/// copy — decoded rather than shared, which is the price of a remote audience.
#[test]
fn the_wire_is_used_when_a_subscriber_is_off_session() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_tx = create_hiroz_context_with_router(&router)?;
    let ctx_far = create_hiroz_context_with_router(&router)?;

    let node_tx = ctx_tx.create_node("mix_tx").build()?;
    let node_near = ctx_tx.create_node("mix_near").build()?;
    let node_far = ctx_far.create_node("mix_far").build()?;

    let near = Arc::new(AtomicUsize::new(0));
    let far = Arc::new(AtomicUsize::new(0));
    let (n, f) = (near.clone(), far.clone());

    let _s_near = node_near
        .create_sub::<RosString>("mixed")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            n.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s_far = node_far
        .create_sub::<RosString>("mixed")
        .build_with_callback(move |_m: RosString| {
            f.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node_tx.create_pub::<RosString>("mixed").build()?;
    wait_for_ready(Duration::from_millis(800));

    publisher.publish_shared(Arc::new(RosString {
        data: "both audiences".to_owned(),
    }))?;

    // The far one is the control: without it crossing, a zero on the near side
    // would be indistinguishable from a broken router.
    assert!(
        wait_until(|| far.load(Ordering::SeqCst) >= 1),
        "the off-session subscriber never received it; this run proves nothing"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        near.load(Ordering::SeqCst),
        1,
        "the near subscriber received {} copies — the bus and the wire both fired",
        near.load(Ordering::SeqCst)
    );
    Ok(())
}

/// #36 — a sole receiver is handed the message itself, not a shared `Arc`.
///
/// The proof is that the callback can **mutate** what it receives. A shared
/// `Arc<T>` cannot be mutated at all, so this does not compile against the
/// shared path — which is the point of having a separate one.
#[test]
fn a_sole_receiver_is_given_the_message_to_own() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("owned_node").build()?;

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = node
        .create_sub::<RosString>("owned")
        .build_with_owned_callback(move |mut msg: RosString| {
            // Mutating the received message is the whole property under test.
            msg.data.push_str(" — mutated by its owner");
            sink.lock().expect("poisoned").push(msg.data);
        })?;

    let publisher = node
        .create_pub::<RosString>("owned")
        .with_intra_process_only()
        .build()?;

    let took = publisher.publish_owned(RosString {
        data: "mine".to_owned(),
    })?;
    assert_eq!(took, 1, "the sole owning receiver did not take the message");

    let got = seen.lock().expect("poisoned");
    assert_eq!(got.len(), 1, "the owned callback did not run");
    assert_eq!(got[0], "mine — mutated by its owner");
    Ok(())
}

/// #36 defect 1. An owned subscriber's wire half was `|_m| {}`, so every
/// message from off-session — that is, from the entire rest of the ROS graph —
/// was discarded in silence while the subscription still advertised itself.
///
/// The publisher here is in a *different* context, so the bus cannot carry it
/// and only the wire half can satisfy this test.
#[test]
fn an_owned_subscriber_receives_from_an_off_session_publisher() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_rx = create_hiroz_context_with_router(&router)?;
    let ctx_tx = create_hiroz_context_with_router(&router)?;
    let node_rx = ctx_rx.create_node("own_wire_rx").build()?;
    let node_tx = ctx_tx.create_node("own_wire_tx").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node_rx
        .create_sub::<RosString>("owned_wire")
        .build_with_owned_callback(move |m: RosString| {
            assert_eq!(m.data, "from far away");
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node_tx.create_pub::<RosString>("owned_wire").build()?;
    wait_for_ready(Duration::from_millis(800));
    publisher.publish(&RosString {
        data: "from far away".to_owned(),
    })?;

    assert!(
        wait_until(|| hits.load(Ordering::SeqCst) >= 1),
        "an owned subscriber discarded a message from another session"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "delivered more than once");
    Ok(())
}

/// #36 defect 2. Depth exhaustion and "no subscriber wanted it" both reported
/// zero, and the caller read zero as "fall back to the wire". The wire re-enters
/// the same callback on a zenoh thread, where the depth counter is a fresh
/// thread-local zero — so a bounded drop became an unbounded loop at wire rate.
///
/// The publisher is plain: no flag, no locality. That is the path the existing
/// recursion test does not take.
#[test]
fn a_self_publishing_callback_does_not_escape_to_the_wire_and_loop() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("loop_node").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    // The publisher is stored erased, so this test does not have to name
    // `ZPub`'s serializer parameter. `OnceLock` rather than `Mutex` because
    // delivery is synchronous: the callback runs on the publishing thread and
    // would re-enter a lock that thread already holds.
    type Echo = Arc<OnceLock<Arc<dyn Fn(Arc<RosString>) + Send + Sync>>>;
    let echo: Echo = Arc::new(OnceLock::new());
    let echo_cb = echo.clone();
    let _sub = node
        .create_sub::<RosString>("cycle")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            let seen = h.fetch_add(1, Ordering::SeqCst);
            // Stop echoing well above the depth guard's bound but well below
            // forever. Without this the unbounded case never terminates, and a
            // detector that hangs tells you less than one that fails: the
            // timeout does not say which property broke.
            if seen < ECHO_CAP {
                if let Some(publish) = echo_cb.get() {
                    publish(m);
                }
            }
        })?;

    let publisher = node.create_pub::<RosString>("cycle").build()?;
    let _ = echo.set(Arc::new(move |m: Arc<RosString>| {
        let _ = publisher.publish_shared(m);
    }));
    wait_for_ready(Duration::from_millis(500));

    // Publish on a worker thread behind a watchdog. Escaping to the wire does
    // not merely loop: the wire callback runs on a zenoh runtime thread and
    // publishes from inside it, which blocks on that runtime — a re-entrancy
    // deadlock. A count assertion cannot see that, because the count never gets
    // to climb. Only a timeout can, so the timeout is made explicit here rather
    // than left to the CI runner, where it would report as "the suite hung"
    // without naming the property.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let counter = hits.clone();
    let publish = echo.get().expect("just set").clone();
    thread::spawn(move || {
        publish(Arc::new(RosString {
            data: "round and round".to_owned(),
        }));
        thread::sleep(Duration::from_millis(400));
        let settled = counter.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(600));
        let _ = done_tx.send((settled, counter.load(Ordering::SeqCst)));
    });

    let (settled, later) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("publish never returned: the depth guard escaped to the wire, \
                 which re-enters on a zenoh thread and deadlocks on its runtime");
    assert!(
        later <= 32,
        "{later} deliveries from one publish (settled at {settled}): the guard \
         escaped to the wire and re-entered at depth zero"
    );
    assert_eq!(
        settled, later,
        "delivery is still growing ({settled} -> {later}) after it should have stopped"
    );
    Ok(())
}

/// #36 defect 3. `bus_can_serve_everyone` never consulted the publisher's own
/// locality. With `Locality::Remote` the wire half cannot reach this session, so
/// taking the wire alone left the same-session subscriber with nothing.
///
/// The far subscriber is the positive control: without it crossing, a zero on
/// the near side would be indistinguishable from a broken router.
#[test]
fn a_remote_locality_publisher_still_reaches_a_same_session_subscriber() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_tx = create_hiroz_context_with_router(&router)?;
    let ctx_far = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx_tx.create_node("rem_tx").build()?;
    let node_near = ctx_tx.create_node("rem_near").build()?;
    let node_far = ctx_far.create_node("rem_far").build()?;

    let near = Arc::new(AtomicUsize::new(0));
    let far = Arc::new(AtomicUsize::new(0));
    let (n, f) = (near.clone(), far.clone());

    let _s_near = node_near
        .create_sub::<RosString>("split")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            n.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s_far = node_far
        .create_sub::<RosString>("split")
        .build_with_callback(move |_m: RosString| {
            f.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node_tx
        .create_pub::<RosString>("split")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(800));

    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "both, disjointly".to_owned(),
    }))?;
    assert_eq!(delivered, 1, "the bus did not carry it to the near subscriber");

    assert!(
        wait_until(|| far.load(Ordering::SeqCst) >= 1),
        "the off-session subscriber never received it; this run proves nothing"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(near.load(Ordering::SeqCst), 1, "near subscriber count wrong");
    assert_eq!(far.load(Ordering::SeqCst), 1, "far subscriber count wrong");
    Ok(())
}
