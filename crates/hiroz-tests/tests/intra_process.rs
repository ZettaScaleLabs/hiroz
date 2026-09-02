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
use hiroz::{
    Builder,
    local_bus::{Delivery, Published},
};
use hiroz_msgs::std_msgs::{Int32, String as RosString};

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
    assert_eq!(
        delivered,
        Published::Bus(Delivery::Sent(1)),
        "expected exactly one local subscriber"
    );

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

    // Positive control: without it, this test passes against a bus that
    // delivers nothing at all, which is the failure mode it exists to detect.
    let ok_hits = Arc::new(AtomicUsize::new(0));
    let ok = ok_hits.clone();
    let _sub_ok = node
        .create_sub::<Int32>("typed_control")
        .build_with_shared_callback(move |_m: Arc<Int32>| {
            ok.fetch_add(1, Ordering::SeqCst);
        })?;
    let control_pub = node
        .create_pub::<Int32>("typed_control")
        .with_intra_process_only()
        .build()?;
    assert_eq!(
        control_pub.publish_shared(Arc::new(Int32 { data: 7 }))?,
        Published::Bus(Delivery::Sent(1)),
        "the control did not deliver: the bus is dead, so the assertion below proves nothing"
    );
    assert_eq!(
        ok_hits.load(Ordering::SeqCst),
        1,
        "control subscriber not called"
    );

    // Same topic, different concrete Rust type.
    let publisher = node
        .create_pub::<RosString>("typed")
        .with_intra_process_only()
        .build()?;
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "wrong type".to_owned(),
    }))?;

    assert_eq!(
        delivered,
        Published::Bus(Delivery::NoTaker),
        "delivered across a type mismatch"
    );
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
    assert_eq!(
        publisher.publish_shared(msg.clone())?,
        Published::Bus(Delivery::Sent(1))
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    drop(sub);

    assert_eq!(
        publisher.publish_shared(msg)?,
        Published::Bus(Delivery::NoTaker),
        "the bus still holds a registration for a dropped subscriber"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a dropped subscriber was called"
    );
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
    // The wire, because this publisher asserted nothing about its audience.
    // Inferring "everyone is local" from the ROS graph is unsound: it cannot
    // see a plain zenoh subscriber. See #134.
    assert_eq!(
        delivered,
        Published::Wire,
        "the bus was taken without the caller asserting the audience"
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
    // Deliberately NOT asserting pointer identity: it arrived over the wire, so
    // it is a decoded copy. The property here is exactly-once, not zero-copy.
    let got = seen.lock().expect("poisoned");
    assert_eq!(got.len(), 1, "subscriber did not receive the message");
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
            let seen = h.fetch_add(1, Ordering::SeqCst);
            // ECHO_CAP so that reverting the depth guard fails this test by
            // assertion rather than by stack overflow, which aborts the whole
            // binary and names no property at all.
            if seen < ECHO_CAP {
                // Republish onto the very topic this callback serves.
                let _ = echo.publish_shared(m);
            }
        })?;

    publisher.publish_shared(Arc::new(RosString {
        data: "round and round".to_owned(),
    }))?;

    let seen = hits.load(Ordering::SeqCst);
    assert!(seen >= 1, "the callback never ran");
    assert_eq!(
        seen,
        hiroz::local_bus::MAX_DELIVERY_DEPTH as usize,
        "expected exactly the depth bound; a change to MAX_DELIVERY_DEPTH must \
         update this test rather than slip past a loose ceiling"
    );
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
fn a_plain_publisher_takes_the_wire_for_an_ordinary_subscriber() -> hiroz::Result<()> {
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
    assert_eq!(
        delivered,
        Published::Wire,
        "the bus took a message its subscriber cannot decode"
    );

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
    assert_eq!(
        took,
        Published::Bus(Delivery::Sent(1)),
        "the sole owning receiver did not take the message"
    );

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
/// thread-local zero, and the publish never returns.
///
/// The publisher is `Locality::Remote`: the bus-taking path that does NOT use
/// `with_intra_process_only()`, which is what the existing recursion test
/// covers. Its wire half cannot reach this session, so the wire cannot echo and
/// the depth guard is the only thing bounding this.
///
/// Deliberately not tested with a *plain* publisher: since #134 that takes the
/// wire alone, so a callback republishing onto its own topic is an ordinary
/// topic cycle — the same unbounded echo any ROS 2 node produces by subscribing
/// and publishing to one topic. That is the user's cycle to avoid, not a
/// defect, and no client library prevents it.
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

    let publisher = node
        .create_pub::<RosString>("cycle")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
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

    let (settled, later) = done_rx.recv_timeout(Duration::from_secs(10)).expect(
        "publish never returned: the depth guard escaped to the wire, \
                 which re-enters on a zenoh thread and deadlocks on its runtime",
    );
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
    assert_eq!(
        delivered,
        Published::BusAndWire(Delivery::Sent(1)),
        "the bus did not carry it to the near subscriber, or skipped the wire"
    );

    assert!(
        wait_until(|| far.load(Ordering::SeqCst) >= 1),
        "the off-session subscriber never received it; this run proves nothing"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        near.load(Ordering::SeqCst),
        1,
        "near subscriber count wrong"
    );
    assert_eq!(far.load(Ordering::SeqCst), 1, "far subscriber count wrong");
    Ok(())
}

/// #133. TRANSIENT_LOCAL durability lives in the wire publisher's cache, and an
/// intra-process-only publisher has no wire. Serving the bus would let a
/// late-joining subscriber be handed a history this message is missing from —
/// wrong rather than absent — so the call is refused instead.
#[test]
fn transient_local_plus_intra_process_only_is_refused() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("tl_node").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("tl_topic")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("tl_topic")
        .with_qos(hiroz::qos::QosProfile {
            durability: hiroz::qos::QosDurability::TransientLocal,
            ..Default::default()
        })
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(500));

    let result = publisher.publish_shared(Arc::new(RosString {
        data: "durable, allegedly".to_owned(),
    }));
    assert!(
        result.is_err(),
        "a transient-local publisher served the bus, which has no durability cache"
    );
    // And it did not deliver it anyway: a refusal that still delivers is worse
    // than either outcome on its own.
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the call was refused but the message was delivered regardless"
    );
    Ok(())
}

/// The `Some(second)` branch of `Channel::publish` has no other coverage: every
/// other test in this file uses exactly one shared subscriber, so a stub that
/// called only the first and returned `Sent(1)` would pass the whole suite.
///
/// `Arc::ptr_eq` on *every* receiver is the point. Delivering a clone to the
/// second and third is what the branch exists to avoid.
#[test]
fn every_shared_subscriber_receives_the_same_allocation() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("fanout").build()?;

    let seen: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut subs = Vec::new();
    for _ in 0..3 {
        let sink = seen.clone();
        subs.push(
            node.create_sub::<RosString>("fanout")
                .build_with_shared_callback(move |m: Arc<RosString>| {
                    sink.lock().expect("poisoned").push(m);
                })?,
        );
    }

    let publisher = node
        .create_pub::<RosString>("fanout")
        .with_intra_process_only()
        .build()?;
    let sent = Arc::new(RosString {
        data: "one allocation, three readers".to_owned(),
    });
    let delivered = publisher.publish_shared(sent.clone())?;
    assert_eq!(
        delivered,
        Published::Bus(Delivery::Sent(3)),
        "not every subscriber was served"
    );

    let got = seen.lock().expect("poisoned");
    assert_eq!(got.len(), 3, "expected three deliveries");
    for (i, g) in got.iter().enumerate() {
        assert!(
            Arc::ptr_eq(&sent, g),
            "receiver {i} was handed a different allocation: the fan-out branch copies"
        );
    }
    Ok(())
}

/// #133 reached through the sibling method. `publish_shared` refuses a
/// transient-local intra-process-only publisher; `publish_owned` took the bus
/// around that check and returned `Ok(1)`.
#[test]
fn transient_local_plus_intra_process_only_is_refused_for_owned_too() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("tl_owned").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("tl_owned_topic")
        .build_with_owned_callback(move |_m: RosString| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("tl_owned_topic")
        .with_qos(hiroz::qos::QosProfile {
            durability: hiroz::qos::QosDurability::TransientLocal,
            ..Default::default()
        })
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(500));

    let result = publisher.publish_owned(RosString {
        data: "durable, allegedly".to_owned(),
    });
    assert!(
        result.is_err(),
        "publish_owned served the bus for a transient-local publisher"
    );
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "refused and delivered anyway"
    );
    Ok(())
}

/// A panicking subscriber must not censor its siblings or kill the publisher.
///
/// Bus delivery is synchronous on the publishing thread, so without isolation a
/// panic unwinds out of `publish_shared` into the application and every
/// subscriber later in the snapshot is skipped. Delivery order is snapshot
/// order, so which siblings are lost varies between runs.
///
/// The wire path gives this isolation for free — one panicking callback kills
/// one zenoh task. This test exists so the bus does not regress against it.
#[test]
fn a_panicking_subscriber_does_not_stop_delivery_to_the_others() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("panicky").build()?;

    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let (b, a) = (before.clone(), after.clone());

    // The panicking subscriber is registered FIRST, deliberately. Delivery
    // walks the snapshot in registration order and the fan-out branch invokes
    // the first entry on its own line, so a panic anywhere later leaves that
    // line untested — which is how an earlier version of this test passed
    // against the isolation being removed.
    let _s1 = node
        .create_sub::<RosString>("panicky")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            panic!("this subscriber is deliberately broken");
        })?;
    let _s2 = node
        .create_sub::<RosString>("panicky")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            b.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s3 = node
        .create_sub::<RosString>("panicky")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            a.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("panicky")
        .with_intra_process_only()
        .build()?;

    // The panic is reported by the default hook; silence it so the test output
    // is readable, and restore the hook afterwards.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "one of you will panic".to_owned(),
    }));
    std::panic::set_hook(prev);

    // The publisher returned at all: the panic did not unwind into this thread.
    let delivered = delivered.expect("the panic escaped into the publishing thread");
    assert_eq!(
        delivered,
        Published::Bus(Delivery::Sent(2)),
        "three subscribers were called and two returned. The count is deliveries, \
         not invocations — the assertions on the two counters below are what \
         prove the panicking one did not stop its siblings."
    );
    assert_eq!(
        before.load(Ordering::SeqCst),
        1,
        "the subscriber after the panicking one was skipped"
    );
    assert_eq!(
        after.load(Ordering::SeqCst),
        1,
        "the subscriber after the panicking one was skipped: one bad callback \
         censored its siblings"
    );
    Ok(())
}

/// The sole-subscriber branch of `Channel::publish` is a different call site
/// from the fan-out branch, and a panic there has nowhere else to go: it would
/// unwind straight out of `publish_shared` into the caller.
#[test]
fn a_panicking_sole_subscriber_does_not_reach_the_publisher() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("panicky_solo").build()?;

    let _sub = node
        .create_sub::<RosString>("panicky_solo")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            panic!("the only subscriber, and it is broken");
        })?;
    let publisher = node
        .create_pub::<RosString>("panicky_solo")
        .with_intra_process_only()
        .build()?;

    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "boom".to_owned(),
    }));
    std::panic::set_hook(prev);

    let delivered = delivered.expect("the panic escaped into the publishing thread");
    assert_eq!(
        delivered,
        Published::Bus(Delivery::NoTaker),
        "the sole subscriber panicked, so nothing was delivered. Sent(1) would \
         report a message as landed when none was: Delivery::Sent counts \
         subscribers that returned, not subscribers that were called."
    );
    Ok(())
}

/// B1. `publish_owned` on a `Locality::Remote` publisher must reach the wire.
///
/// This is the detector that did not exist. Every other `publish_owned` test
/// uses `with_intra_process_only()`, so none of them could see that the Remote
/// arm took the bus and returned — losing the message to every off-session
/// subscriber, silently, while `publish_shared` in the identical configuration
/// served both.
///
/// The far assertion is the point. The near one passes against the defect.
#[test]
fn publish_owned_on_a_remote_publisher_still_reaches_the_wire() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx_tx = create_hiroz_context_with_router(&router)?;
    let ctx_far = create_hiroz_context_with_router(&router)?;
    let node_tx = ctx_tx.create_node("owned_rem_tx").build()?;
    let node_near = ctx_tx.create_node("owned_rem_near").build()?;
    let node_far = ctx_far.create_node("owned_rem_far").build()?;

    let near = Arc::new(AtomicUsize::new(0));
    let far = Arc::new(AtomicUsize::new(0));
    let (n, f) = (near.clone(), far.clone());

    let _s_near = node_near
        .create_sub::<RosString>("owned_split")
        .build_with_owned_callback(move |_m: RosString| {
            n.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s_far = node_far
        .create_sub::<RosString>("owned_split")
        .build_with_callback(move |_m: RosString| {
            f.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node_tx
        .create_pub::<RosString>("owned_split")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(800));

    let outcome = publisher.publish_owned(RosString {
        data: "must reach both".to_owned(),
    })?;

    // Shared, not moved: the wire half needs the value to serialize it, so a
    // Remote publisher cannot give it away.
    assert!(
        matches!(outcome, Published::BusAndWire(_)),
        "a Remote publisher must run both routes, got {outcome:?}"
    );

    assert!(
        wait_until(|| far.load(Ordering::SeqCst) >= 1),
        "the off-session subscriber never received it — this is B1"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        near.load(Ordering::SeqCst),
        1,
        "near subscriber count wrong"
    );
    assert_eq!(far.load(Ordering::SeqCst), 1, "far subscriber count wrong");
    Ok(())
}

/// B2. The durability refusal permits a `Locality::Remote` publisher because
/// the wire still populates its cache. That justification has to hold on the
/// owned path too, or a late joiner is served a history this message is
/// missing from.
#[test]
fn transient_local_plus_remote_locality_is_allowed_on_the_owned_path() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("tl_rem_node").build()?;

    let publisher = node
        .create_pub::<RosString>("tl_rem_topic")
        .with_qos(hiroz::qos::QosProfile {
            durability: hiroz::qos::QosDurability::TransientLocal,
            ..Default::default()
        })
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    // Permitted, because the wire runs and fills the cache. The companion test
    // above is what proves the wire actually runs; without it this assertion
    // would be satisfied by a publisher that silently dropped the message.
    let outcome = publisher.publish_owned(RosString {
        data: "durable".to_owned(),
    })?;
    assert!(
        matches!(outcome, Published::BusAndWire(_) | Published::Wire),
        "a durable Remote publisher must still reach the wire, got {outcome:?}"
    );
    Ok(())
}

/// B3. `Published` must distinguish the three ways nothing was delivered
/// locally, because a caller may fall back to the wire on one of them and must
/// not on another.
///
/// A count cannot carry this: `NoTaker`, `DepthExceeded` and "there is no bus
/// on this publisher" were all zero.
#[test]
fn published_says_which_routes_ran_and_why_nothing_was_taken() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("outcome_node").build()?;

    // 1. No assertion from the caller: the wire alone, and the bus is not
    //    consulted at all. Not `Bus(NoTaker)` — nothing asked the bus.
    let plain = node.create_pub::<RosString>("outcome_plain").build()?;
    let out = plain.publish_shared(Arc::new(RosString {
        data: "wire".to_owned(),
    }))?;
    assert_eq!(out, Published::Wire, "a plain publisher must report Wire");

    // 2. Bus asserted, nobody listening. Distinct from the case above: here the
    //    bus WAS asked and had no taker.
    let local = node
        .create_pub::<RosString>("outcome_local")
        .with_intra_process_only()
        .build()?;
    let out = local.publish_shared(Arc::new(RosString {
        data: "nobody".to_owned(),
    }))?;
    assert_eq!(
        out,
        Published::Bus(Delivery::NoTaker),
        "an empty bus must report NoTaker, not Wire"
    );

    // 3. Bus asserted, one subscriber.
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("outcome_taken")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;
    let taken = node
        .create_pub::<RosString>("outcome_taken")
        .with_intra_process_only()
        .build()?;
    let out = taken.publish_shared(Arc::new(RosString {
        data: "taken".to_owned(),
    }))?;
    assert_eq!(out, Published::Bus(Delivery::Sent(1)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    Ok(())
}

/// B3, the half a count could never express: a message refused at
/// `MAX_DELIVERY_DEPTH` is dropped, and must not be reported as delivered.
///
/// Before this, the bus returned `Ok(())` for both outcomes and `ZPub` mapped
/// it to `Ok(1)` — a dropped message counted as one receiver taking it. A
/// caller that retries on a low count would have retried forever on the one
/// outcome where retrying re-enters the same callback.
#[test]
fn a_depth_refusal_is_reported_as_dropped_not_delivered() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("depth_outcome").build()?;

    let outcomes: Arc<Mutex<Vec<Published>>> = Arc::new(Mutex::new(Vec::new()));
    let publisher: Arc<OnceLock<Box<dyn Fn(RosString) -> hiroz::Result<Published> + Send + Sync>>> =
        Arc::new(OnceLock::new());

    let p = publisher.clone();
    let o = outcomes.clone();
    let _sub = node
        .create_sub::<RosString>("depth_outcome")
        .build_with_owned_callback(move |m: RosString| {
            // Republish onto the same topic: each delivery nests one deeper.
            if let Some(send) = p.get() {
                if let Ok(out) = send(m) {
                    o.lock().expect("outcome lock").push(out);
                }
            }
        })?;

    let pubr = node
        .create_pub::<RosString>("depth_outcome")
        .with_intra_process_only()
        .build()?;
    let pubr = Arc::new(pubr);
    let p2 = pubr.clone();
    publisher
        .set(Box::new(move |m: RosString| p2.publish_owned(m)))
        .map_err(|_| "publisher already set")
        .expect("set once");

    pubr.publish_owned(RosString {
        data: "recurse".to_owned(),
    })?;

    let seen = outcomes.lock().expect("outcome lock");
    assert!(
        seen.iter()
            .any(|o| matches!(o, Published::Bus(Delivery::DepthExceeded))),
        "the depth refusal was never reported; outcomes were {seen:?}"
    );
    // A plain global bound, not a per-element predicate. The previous form put
    // the loop-invariant `seen.len()` inside `any(..)`, where the guard's own
    // bound made it false for every element — so the assertion was `!false` for
    // all inputs and could never fail. Without this, "refused once at depth 8"
    // and "refused once at depth 800" are indistinguishable.
    assert!(
        seen.len() <= hiroz::local_bus::MAX_DELIVERY_DEPTH as usize + 2,
        "delivery went {} deep; the depth bound is {}",
        seen.len(),
        hiroz::local_bus::MAX_DELIVERY_DEPTH
    );
    Ok(())
}

/// S9. `publish_shared` and `publish_owned` must agree about where a message
/// goes. They cannot, unless they read the same decision.
///
/// They did not. Each tested the conditions itself, in the opposite order, so a
/// publisher carrying both assertions routed one way through one method and the
/// other way through the other — the same drift that produced B1, in a
/// configuration nothing exercised. `with_intra_process_only()` wins, because it
/// is the stronger claim: this publisher has no wire.
#[test]
fn both_publish_methods_agree_when_the_assertions_conflict() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("conflict_node").build()?;

    let publisher = node
        .create_pub::<RosString>("conflict_topic")
        .with_intra_process_only()
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    let shared = publisher.publish_shared(Arc::new(RosString {
        data: "shared".to_owned(),
    }))?;
    let moved = publisher.publish_owned(RosString {
        data: "moved".to_owned(),
    })?;

    // Nobody is subscribed, so both report the bus having no taker. The point is
    // that neither reached the wire: a `BusAndWire` from either is the drift.
    assert_eq!(
        shared,
        Published::Bus(Delivery::NoTaker),
        "publish_shared took the wrong route"
    );
    assert_eq!(
        moved,
        Published::Bus(Delivery::NoTaker),
        "publish_owned disagreed with publish_shared"
    );
    Ok(())
}

/// An owned subscriber must not be starved in silence.
///
/// `Channel::publish` filters on `is_shared()`, so an owned subscriber is
/// invisible to the shared path — and an `intra_process_only` publisher has no
/// wire behind the bus. Before this, the message vanished and the caller was
/// told `Ok(Bus(NoTaker))`: a registered receiver of the right type existed and
/// got nothing, with one `debug!` line as the only trace.
///
/// It cannot be repaired by serving a clone, because `ZMessage` is not `Clone`.
/// So the publisher refuses instead, the way `refuse_durable_bus` refuses a
/// durable publisher rather than quietly violating its contract.
#[test]
fn publish_shared_refuses_when_only_owned_subscribers_are_listening() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("owned_starve").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let _sub = node
        .create_sub::<RosString>("owned_starve")
        .build_with_owned_callback(move |_m: RosString| {
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("owned_starve")
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    let outcome = publisher.publish_shared(Arc::new(RosString {
        data: "nobody shared is listening".to_owned(),
    }));

    assert!(
        outcome.is_err(),
        "an owned subscriber was registered and could not be served, and there is \
         no wire: this must refuse, not report success. Got {outcome:?}"
    );
    // The control: it really was unserviceable, not merely refused.
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the owned subscriber cannot be served by a shared publish"
    );
    Ok(())
}

/// The same starvation reached through `publish_owned`, which is the likelier
/// route to it: two owning subscribers means nothing can be given away, so it
/// falls back to sharing — into the case above.
#[test]
fn publish_owned_refuses_when_two_owned_subscribers_cannot_be_served() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("owned_two").build()?;

    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let (ca, cb) = (a.clone(), b.clone());
    let _s1 = node
        .create_sub::<RosString>("owned_two")
        .build_with_owned_callback(move |_m: RosString| {
            ca.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s2 = node
        .create_sub::<RosString>("owned_two")
        .build_with_owned_callback(move |_m: RosString| {
            cb.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("owned_two")
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    let outcome = publisher.publish_owned(RosString {
        data: "two takers, nothing to give".to_owned(),
    });

    assert!(
        outcome.is_err(),
        "two owning subscribers cannot both be given the value, and neither can be \
         served by the shared fallback: this must refuse. Got {outcome:?}"
    );
    assert_eq!(
        (a.load(Ordering::SeqCst), b.load(Ordering::SeqCst)),
        (0, 0),
        "neither owned subscriber is serviceable here"
    );
    Ok(())
}

/// Reordering `publish_shared`'s `BusAndWire` arm to publish on the wire first
/// must not stop it serving the bus. The ordering itself exists so that a wire
/// failure returns `Err` before any local subscriber has run — otherwise a
/// caller that retries delivers twice locally, and `Result<Published>` has no
/// partial-success value with which to warn them.
#[test]
fn a_remote_publisher_serves_the_bus_after_the_wire() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("order_near").build()?;

    let near = Arc::new(AtomicUsize::new(0));
    let n = near.clone();
    let _sub = node
        .create_sub::<RosString>("order_topic")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            n.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("order_topic")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    let outcome = publisher.publish_shared(Arc::new(RosString {
        data: "both routes".to_owned(),
    }))?;

    assert!(
        matches!(outcome, Published::BusAndWire(Delivery::Sent(1))),
        "the bus half must still run, and after the wire. Got {outcome:?}"
    );
    assert_eq!(
        near.load(Ordering::SeqCst),
        1,
        "the same-session subscriber must still be served exactly once"
    );
    Ok(())
}

/// A channel nobody holds any more is reclaimed, so the registry does not grow
/// for the life of the process.
///
/// `channel()` runs in **every** `ZPubBuilder::build()`, so this is not one
/// entry per `ZContext` — a process publishing on dynamically-named topics
/// accumulated one entry per topic, permanently, long after every publisher was
/// dropped. Reclamation is safe only because an `Arc::strong_count` of one
/// proves no `ZPub` or `ZSub` can still reach the channel, so a later builder
/// cannot be split from a live endpoint.
#[test]
fn a_channel_no_endpoint_holds_is_reclaimed() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("reclaim").build()?;

    let before = hiroz::local_bus::total_channels();
    for i in 0..24 {
        let p = node
            .create_pub::<RosString>(&format!("ephemeral_{i}"))
            .build()?;
        drop(p);
    }
    let after_ephemeral = hiroz::local_bus::total_channels();
    assert!(
        after_ephemeral - before <= 4,
        "24 publishers were created and dropped; the registry grew by {} channels. \
         It should reclaim the ones no endpoint holds.",
        after_ephemeral - before
    );

    // The control. Without it this test passes equally well against a registry
    // that reclaims indiscriminately — which would split a live publisher from
    // a subscriber created afterwards, turning a leak into lost messages.
    let held = node.create_pub::<RosString>("held_topic").build()?;
    let with_held = hiroz::local_bus::total_channels();
    let _forces_a_reclaim_pass = node.create_pub::<RosString>("spacer_topic").build()?;
    assert!(
        hiroz::local_bus::total_channels() >= with_held,
        "a channel a live publisher still holds must survive a reclamation pass"
    );
    drop(held);
    Ok(())
}
