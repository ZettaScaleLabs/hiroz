//! Intra-process `Arc<T>` delivery: does it skip the serialization?
//!
//! The assertion that separates the bus from the wire is `Arc::ptr_eq`. A CDR
//! round trip satisfies "the message arrived and its contents match" just as
//! well as a pointer move. It cannot preserve the allocation, because decoding
//! produces a new one.
//!
//! Each test that asserts a negative (nothing arrived, nothing was served) has a
//! positive control in the same test, so a dead bus or router cannot pass it.

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

/// The erased republish hook the depth tests install. Naming the type avoids
/// `clippy::type_complexity` on the `ZPub` serializer parameter.
type EchoPublisher = Box<dyn Fn(RosString) -> hiroz::Result<Published> + Send + Sync>;

const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

/// Ceiling on the self-publishing echo, so a missing depth guard fails by
/// assertion instead of running forever. Far above the delivery-depth bound.
const ECHO_CAP: usize = 64;

/// Silences the default panic hook for readable output, and restores the
/// previous hook on drop, so a failing assertion cannot leave a silent hook.
/// Tests that hold one must carry `#[serial_test::serial(panic_hook)]`.
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

struct SilentPanicHook(Option<PanicHook>);

impl SilentPanicHook {
    fn install() -> Self {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self(Some(prev))
    }
}

impl Drop for SilentPanicHook {
    fn drop(&mut self) {
        if let Some(prev) = self.0.take() {
            std::panic::set_hook(prev);
        }
    }
}

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
        // publish_ref, not publish(&msg): `msg` is an Arc, and a generic
        // parameter does not deref-coerce the way `&T` did.
        pub_control.publish_ref(&msg)?;
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

/// An ordinary publisher reaches a shared-callback subscriber.
///
/// Nothing errors if the subscriber's wire half discards same-session messages,
/// and the graph shows both endpoints matched, so only a delivery assertion
/// catches it.
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

/// A `publish_shared` on a publisher with no locality restriction arrives once.
///
/// It reaches the subscriber over the wire and must not also arrive over the bus.
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
    // The graph cannot see a plain zenoh subscriber, so the bus is not inferred.
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

/// A callback that publishes onto its own topic does not recurse forever.
///
/// Delivery is inline, so this is direct recursion. The bus refuses past a fixed
/// depth, so the chain ends and the test returns instead of overflowing the
/// stack.
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
            // The cap makes a missing depth guard fail by assertion. A stack overflow
            // would abort the whole binary and name no property.
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

/// A plain publisher takes the wire for an ordinary (non-bus) subscriber.
///
/// The publisher asserts nothing about its audience, so `publish_shared` goes
/// over the wire and the subscriber receives a decoded message.
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

/// A plain publisher reaches an off-session subscriber over the wire, and the
/// bus stays out.
///
/// Using both routes would deliver twice to a same-session bus subscriber. The
/// local subscriber gets one decoded copy.
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

/// A sole receiver is handed the message itself, not a shared `Arc`.
///
/// The callback mutates what it receives. A shared `Arc<T>` cannot be mutated,
/// so this does not compile against the shared path.
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

    let took = publisher.publish_moved(RosString {
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

/// An owned subscriber receives from an off-session publisher.
///
/// The publisher is in a different context, so only the subscriber's wire half
/// can satisfy this test.
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

/// A self-publishing callback does not escape to the wire and loop.
///
/// Depth exhaustion and "no subscriber wanted it" must stay distinct. A caller
/// that falls back to the wire on depth exhaustion re-enters the callback on a
/// zenoh thread, where the depth counter restarts at zero, and never returns.
///
/// The publisher is `Locality::Remote`, the bus route that does not use
/// `with_intra_process_only()`. Its wire half cannot reach this session, so the
/// depth guard is the only bound.
///
/// A plain publisher is not tested: it takes the wire alone, so a callback that
/// republishes onto its own topic is an ordinary topic cycle that the user must
/// avoid.
#[test]
fn a_self_publishing_callback_does_not_escape_to_the_wire_and_loop() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("loop_node").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    // The publisher is stored erased to avoid naming `ZPub`'s serializer
    // parameter. `OnceLock`, not `Mutex`: delivery is synchronous, and the
    // callback would re-enter a lock this thread already holds.
    type Echo = Arc<OnceLock<Arc<dyn Fn(Arc<RosString>) + Send + Sync>>>;
    let echo: Echo = Arc::new(OnceLock::new());
    let echo_cb = echo.clone();
    let _sub = node
        .create_sub::<RosString>("cycle")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            let seen = h.fetch_add(1, Ordering::SeqCst);
            // Stop echoing well above the guard's bound. Without a cap the unbounded
            // case never terminates, and a hang does not name the broken property.
            if seen < ECHO_CAP
                && let Some(publish) = echo_cb.get()
            {
                publish(m);
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
    // not only loop: the wire callback publishes from a zenoh runtime thread,
    // which deadlocks on that runtime. A count assertion cannot see a deadlock,
    // so the timeout is explicit and names the property.
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

/// A `Locality::Remote` publisher still reaches a same-session subscriber.
///
/// The wire half cannot reach this session, so the bus must serve it. The far
/// subscriber is the positive control for the near one.
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

/// `TRANSIENT_LOCAL` with an intra-process-only publisher is refused.
///
/// The durability cache lives in the wire publisher, and this publisher has no
/// wire, so the bus cannot serve a late joiner the full history.
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
    // Refused, and not delivered anyway: a refusal that still delivers is worse.
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the call was refused but the message was delivered regardless"
    );
    Ok(())
}

/// Every shared subscriber receives the same allocation.
///
/// The fan-out branch of `Channel::publish` is covered by no other test, which
/// uses one subscriber. `Arc::ptr_eq` on every receiver checks that none gets a
/// clone.
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

/// The `TRANSIENT_LOCAL` refusal also holds on the `publish_moved` route.
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

    let result = publisher.publish_moved(RosString {
        data: "durable, allegedly".to_owned(),
    });
    assert!(
        result.is_err(),
        "publish_moved served the bus for a transient-local publisher"
    );
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "refused and delivered anyway"
    );
    Ok(())
}

/// A panicking subscriber does not stop its siblings or the publisher.
///
/// Delivery is synchronous, so without isolation a panic unwinds out of
/// `publish_shared` and skips every later subscriber in the snapshot.
#[test]
#[serial_test::serial(panic_hook)]
fn a_panicking_subscriber_does_not_stop_delivery_to_the_others() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("panicky").build()?;

    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let (b, a) = (before.clone(), after.clone());

    // The panicking subscriber is registered first: the fan-out branch calls
    // the first entry on its own line, and a later panic would leave that line
    // untested.
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

    // Silence the default panic hook for readable output, and restore it after.
    let hook = SilentPanicHook::install();
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "one of you will panic".to_owned(),
    }));
    drop(hook);

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

/// A panicking sole subscriber does not reach the publisher.
///
/// The sole-subscriber branch of `Channel::publish` is a separate call site from
/// the fan-out branch.
#[test]
#[serial_test::serial(panic_hook)]
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

    let hook = SilentPanicHook::install();
    let delivered = publisher.publish_shared(Arc::new(RosString {
        data: "boom".to_owned(),
    }));
    drop(hook);

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

/// `publish_moved` on a `Locality::Remote` publisher reaches the wire.
///
/// The far assertion carries the property. The near one also passes when the
/// publisher takes the bus alone.
#[test]
fn publish_moved_on_a_remote_publisher_still_reaches_the_wire() -> hiroz::Result<()> {
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

    let outcome = publisher.publish_moved(RosString {
        data: "must reach both".to_owned(),
    })?;

    // Shared, not moved: the wire needs the value to serialize it.
    assert!(
        matches!(outcome, Published::BusAndWire(_)),
        "a Remote publisher must run both routes, got {outcome:?}"
    );

    assert!(
        wait_until(|| far.load(Ordering::SeqCst) >= 1),
        "the off-session subscriber never received it"
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

/// `TRANSIENT_LOCAL` with `Locality::Remote` is allowed on the owned path,
/// because the wire fills the durability cache.
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
    // above proves the wire runs.
    let outcome = publisher.publish_moved(RosString {
        data: "durable".to_owned(),
    })?;
    assert!(
        matches!(outcome, Published::BusAndWire(_) | Published::Wire),
        "a durable Remote publisher must still reach the wire, got {outcome:?}"
    );
    Ok(())
}

/// `Published` distinguishes the three ways nothing was delivered locally.
///
/// A caller may fall back to the wire on one of them and must not on another.
/// A bare count cannot carry the difference.
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

/// A message refused at `MAX_DELIVERY_DEPTH` is reported as dropped, not
/// delivered.
///
/// Retrying on a low count would re-enter the same callback.
#[test]
fn a_depth_refusal_is_reported_as_dropped_not_delivered() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("depth_outcome").build()?;

    let outcomes: Arc<Mutex<Vec<Published>>> = Arc::new(Mutex::new(Vec::new()));
    let publisher: Arc<OnceLock<EchoPublisher>> = Arc::new(OnceLock::new());

    let p = publisher.clone();
    let o = outcomes.clone();
    let _sub = node
        .create_sub::<RosString>("depth_outcome")
        .build_with_owned_callback(move |m: RosString| {
            // Republish onto the same topic: each delivery nests one deeper.
            if let Some(send) = p.get()
                && let Ok(out) = send(m)
            {
                o.lock().expect("outcome lock").push(out);
            }
        })?;

    let pubr = node
        .create_pub::<RosString>("depth_outcome")
        .with_intra_process_only()
        .build()?;
    let pubr = Arc::new(pubr);
    let p2 = pubr.clone();
    publisher
        .set(Box::new(move |m: RosString| p2.publish_moved(m)))
        .map_err(|_| "publisher already set")
        .expect("set once");

    pubr.publish_moved(RosString {
        data: "recurse".to_owned(),
    })?;

    let seen = outcomes.lock().expect("outcome lock");
    assert!(
        seen.iter()
            .any(|o| matches!(o, Published::Bus(Delivery::DepthExceeded))),
        "the depth refusal was never reported; outcomes were {seen:?}"
    );
    // A global bound on `seen.len()`. A per-element predicate cannot fail,
    // because the guard's own bound makes it false for every element.
    assert!(
        seen.len() <= hiroz::local_bus::MAX_DELIVERY_DEPTH as usize + 2,
        "delivery went {} deep; the depth bound is {}",
        seen.len(),
        hiroz::local_bus::MAX_DELIVERY_DEPTH
    );
    Ok(())
}

/// `publish_shared` and `publish_moved` agree on the route when the assertions
/// conflict.
///
/// Both read one routing decision. `with_intra_process_only()` wins over a
/// locality, because it asserts that the publisher has no wire.
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
    let moved = publisher.publish_moved(RosString {
        data: "moved".to_owned(),
    })?;

    // Nobody is subscribed, so both report no taker. Neither may reach the wire:
    // `BusAndWire` from either means the routes disagree.
    assert_eq!(
        shared,
        Published::Bus(Delivery::NoTaker),
        "publish_shared took the wrong route"
    );
    assert_eq!(
        moved,
        Published::Bus(Delivery::NoTaker),
        "publish_moved disagreed with publish_shared"
    );
    Ok(())
}

/// An owned subscriber is not starved in silence.
///
/// The shared path skips an owned subscriber, and an `intra_process_only`
/// publisher has no wire behind the bus. The publisher refuses instead of
/// returning `Ok(Bus(NoTaker))`, as `refuse_durable_bus` refuses a durable
/// publisher. A clone cannot serve the subscriber: `ZMessage` is not `Clone`.
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
    // The control: the subscriber was unserviceable, not merely refused.
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the owned subscriber cannot be served by a shared publish"
    );
    Ok(())
}

/// Two owning subscribers cannot be served by `publish_moved`: nothing can be
/// given away, so it falls back to the shared path above.
#[test]
fn publish_moved_refuses_when_two_owned_subscribers_cannot_be_served() -> hiroz::Result<()> {
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

    let outcome = publisher.publish_moved(RosString {
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

/// A wire failure returns before any local subscriber has run.
///
/// Bus delivery is synchronous. If the bus ran first, a caller that retries
/// after an `Err` would deliver to every local subscriber twice. The property is
/// observable only when the wire fails, so the test closes the session.
#[test]
fn a_wire_failure_returns_before_any_local_subscriber_runs() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("order_fail").build()?;

    let near = Arc::new(AtomicUsize::new(0));
    let n = near.clone();
    let _sub = node
        .create_sub::<RosString>("order_fail_topic")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            n.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("order_fail_topic")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    // The control: while the session is alive, both routes run.
    let ok = publisher.publish_shared(Arc::new(RosString {
        data: "healthy".to_owned(),
    }))?;
    assert!(
        matches!(ok, Published::BusAndWire(Delivery::Sent(1))),
        "control: a healthy publish must run both routes, got {ok:?}"
    );
    assert_eq!(
        near.load(Ordering::SeqCst),
        1,
        "control: the bus must deliver"
    );

    ctx.shutdown()?;
    let after_control = near.load(Ordering::SeqCst);

    let failed = publisher.publish_shared(Arc::new(RosString {
        data: "the wire is gone".to_owned(),
    }));
    assert!(
        failed.is_err(),
        "the session is closed, so the wire half must fail; got {failed:?}"
    );
    assert_eq!(
        near.load(Ordering::SeqCst),
        after_control,
        "the wire failed, so no local subscriber may have run. Running the bus \
         first and failing afterwards leaves the caller unable to retry safely."
    );
    Ok(())
}

/// A channel that no endpoint holds is reclaimed.
///
/// `channel()` runs in every `ZPubBuilder::build()`, so a process that publishes
/// on dynamically named topics would otherwise grow the registry for its
/// lifetime. Reclamation is safe because a strong count of one proves that no
/// `ZPub` or `ZSub` can reach the channel.
#[test]
#[serial_test::serial(registry)]
fn a_channel_no_endpoint_holds_is_reclaimed() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("reclaim").build()?;

    // Counted per session: other tests in the process move the global total.
    let zid = node.session().zid();
    let before = hiroz::local_bus::session_channels(zid);
    for i in 0..24 {
        let p = node
            .create_pub::<RosString>(&format!("ephemeral_{i}"))
            .build()?;
        drop(p);
    }
    let after_ephemeral = hiroz::local_bus::session_channels(zid);
    assert!(
        after_ephemeral.saturating_sub(before) <= 4,
        "24 publishers were created and dropped; the registry grew by {} channels. \
         It should reclaim the ones no endpoint holds.",
        after_ephemeral.saturating_sub(before)
    );

    // The control is delivery, not a count: a registry that reclaims
    // indiscriminately would split a live publisher from a later subscriber, and
    // a refilled count could hide a deleted channel.
    let held = node
        .create_pub::<RosString>("held_topic")
        .with_intra_process_only()
        .build()?;
    let _forces_a_reclaim_pass = node.create_pub::<RosString>("spacer_topic").build()?;
    let got = Arc::new(AtomicUsize::new(0));
    let g = got.clone();
    let _sub = node
        .create_sub::<RosString>("held_topic")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            g.fetch_add(1, Ordering::SeqCst);
        })?;
    let outcome = held.publish_shared(Arc::new(RosString {
        data: "after a reclaim pass".to_owned(),
    }))?;
    assert_eq!(
        outcome,
        Published::Bus(Delivery::Sent(1)),
        "a channel a live publisher still holds must survive a reclamation pass, \
         or a later subscriber is split from it"
    );
    assert_eq!(got.load(Ordering::SeqCst), 1);
    drop(held);
    Ok(())
}

/// A shared and an owned subscriber of one type cannot both be served by an
/// intra-process-only publisher, so it refuses and delivers to neither.
///
/// With two takers the value cannot be given away, and the shared path skips the
/// owned one.
#[test]
fn publish_moved_refuses_when_an_owned_and_a_shared_subscriber_coexist() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("owned_mixed").build()?;

    let shared = Arc::new(AtomicUsize::new(0));
    let owned = Arc::new(AtomicUsize::new(0));
    let (cs, co) = (shared.clone(), owned.clone());
    let _s1 = node
        .create_sub::<RosString>("owned_mixed")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            cs.fetch_add(1, Ordering::SeqCst);
        })?;
    let _s2 = node
        .create_sub::<RosString>("owned_mixed")
        .build_with_owned_callback(move |_m: RosString| {
            co.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("owned_mixed")
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    let outcome = publisher.publish_moved(RosString {
        data: "one shared taker, one owned taker".to_owned(),
    });

    assert!(
        outcome.is_err(),
        "the owned subscriber cannot be served and there is no wire behind the bus: \
         this must refuse rather than report success. Got {outcome:?}"
    );
    assert_eq!(
        (shared.load(Ordering::SeqCst), owned.load(Ordering::SeqCst)),
        (0, 0),
        "a refused publish delivers to nobody"
    );
    Ok(())
}

/// A panic payload whose destructor panics does not escape the isolation.
///
/// Dropping the payload is user code running outside the guard. The first
/// subscriber panics with such a payload, and the second is still called.
#[test]
#[serial_test::serial(panic_hook)]
fn a_panic_payload_that_panics_on_drop_does_not_stop_delivery() -> hiroz::Result<()> {
    struct Bomb;
    impl Drop for Bomb {
        fn drop(&mut self) {
            panic!("the payload's destructor panics");
        }
    }

    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("bomb").build()?;

    let after = Arc::new(AtomicUsize::new(0));
    let a = after.clone();
    let _s1 = node
        .create_sub::<RosString>("bomb")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            std::panic::panic_any(Bomb);
        })?;
    let _s2 = node
        .create_sub::<RosString>("bomb")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            a.fetch_add(1, Ordering::SeqCst);
        })?;
    let publisher = node
        .create_pub::<RosString>("bomb")
        .with_intra_process_only()
        .build()?;

    let hook = SilentPanicHook::install();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publisher.publish_shared(Arc::new(RosString {
            data: "armed".to_owned(),
        }))
    }));
    drop(hook);

    assert!(
        result.is_ok(),
        "the payload's destructor panicked into the publishing thread"
    );
    assert_eq!(
        after.load(Ordering::SeqCst),
        1,
        "the sibling after the bomb must still be called"
    );
    Ok(())
}

/// `NoTaker` is not proof that no callback ran.
///
/// A callback that keeps the `Arc` and then panics has had an effect, yet the
/// bus reports `NoTaker`. A caller that retried on `NoTaker` would repeat the
/// effect. The test pins the documented outcome on both call sites of
/// `Channel::publish`, and the recorded effect is the positive control.
#[test]
#[serial_test::serial(panic_hook)]
fn no_taker_is_reported_although_a_panicking_callback_had_an_effect() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("effect_then_panic").build()?;

    let kept: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));
    let kept_more: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));

    let sink = kept.clone();
    let _solo = node
        .create_sub::<RosString>("effect_solo")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            sink.lock().expect("poisoned").push(m);
            panic!("the callback records its effect, then panics");
        })?;
    let (first, second) = (kept_more.clone(), kept_more.clone());
    let _fan1 = node
        .create_sub::<RosString>("effect_fan")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            first.lock().expect("poisoned").push(m);
            panic!("the first callback records its effect, then panics");
        })?;
    let _fan2 = node
        .create_sub::<RosString>("effect_fan")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            second.lock().expect("poisoned").push(m);
            panic!("the second callback records its effect, then panics");
        })?;

    let solo = node
        .create_pub::<RosString>("effect_solo")
        .with_intra_process_only()
        .build()?;
    let fan = node
        .create_pub::<RosString>("effect_fan")
        .with_intra_process_only()
        .build()?;

    let hook = SilentPanicHook::install();
    let solo_result = solo.publish_shared(Arc::new(RosString {
        data: "sole".to_owned(),
    }));
    let fan_result = fan.publish_shared(Arc::new(RosString {
        data: "fan-out".to_owned(),
    }));
    drop(hook);

    assert_eq!(
        solo_result.expect("the panic escaped into the publisher"),
        Published::Bus(Delivery::NoTaker),
        "a sole callback that panicked is reported as NoTaker"
    );
    assert_eq!(
        kept.lock().expect("poisoned").len(),
        1,
        "the effect was not recorded, so NoTaker here would be accurate"
    );
    assert_eq!(
        fan_result.expect("the panic escaped into the publisher"),
        Published::Bus(Delivery::NoTaker),
        "when every callback panicked the fan-out reports NoTaker too"
    );
    assert_eq!(
        kept_more.lock().expect("poisoned").len(),
        2,
        "both callbacks ran and recorded an effect"
    );
    Ok(())
}

/// Known limit, pinned as it is: on a `Locality::Remote` publisher
/// `publish_moved` does not refuse a mix of shared and owned receivers.
///
/// The shared receiver gets the message, the owned receiver gets nothing, and
/// the result carries no error. `publish_shared` on the same publisher refuses,
/// which shows that the owned receiver is registered and counted. If the
/// behaviour changes, update the documented limit on
/// `ZSubBuilder::build_with_owned_callback` with it.
#[test]
fn remote_publisher_owned_publish_serves_only_the_shared_receiver_of_a_mix() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("remote_mix").build()?;

    let shared = Arc::new(AtomicUsize::new(0));
    let owned = Arc::new(AtomicUsize::new(0));
    let (s, o) = (shared.clone(), owned.clone());
    let _shared_sub = node
        .create_sub::<RosString>("remote_mix")
        .build_with_shared_callback(move |_m: Arc<RosString>| {
            s.fetch_add(1, Ordering::SeqCst);
        })?;
    let _owned_sub = node
        .create_sub::<RosString>("remote_mix")
        .build_with_owned_callback(move |_m: RosString| {
            o.fetch_add(1, Ordering::SeqCst);
        })?;
    let publisher = node
        .create_pub::<RosString>("remote_mix")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;

    let outcome = publisher.publish_moved(RosString {
        data: "mixed receivers".to_owned(),
    })?;
    assert_eq!(
        outcome,
        Published::BusAndWire(Delivery::Sent(1)),
        "the shared receiver is the only one served, and nothing reports the omission"
    );
    assert_eq!(
        shared.load(Ordering::SeqCst),
        1,
        "the shared receiver got it"
    );
    assert_eq!(
        owned.load(Ordering::SeqCst),
        0,
        "known limit: the owned receiver gets nothing on a Remote publisher"
    );

    assert!(
        publisher
            .publish_shared(Arc::new(RosString {
                data: "control".to_owned(),
            }))
            .is_err(),
        "control: publish_shared refuses this mix, so the owned receiver is registered"
    );
    Ok(())
}

/// Known limit, pinned as it is: two owned receivers on a `Locality::Remote`
/// publisher receive nothing from `publish_moved`, and the result carries no
/// error. See the companion test for the mixed case.
#[test]
fn remote_publisher_owned_publish_reaches_no_one_of_two_owned_receivers() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("remote_two_owned").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let (a, b) = (hits.clone(), hits.clone());
    let _o1 = node
        .create_sub::<RosString>("remote_two_owned")
        .build_with_owned_callback(move |_m: RosString| {
            a.fetch_add(1, Ordering::SeqCst);
        })?;
    let _o2 = node
        .create_sub::<RosString>("remote_two_owned")
        .build_with_owned_callback(move |_m: RosString| {
            b.fetch_add(1, Ordering::SeqCst);
        })?;
    let publisher = node
        .create_pub::<RosString>("remote_two_owned")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()?;

    let outcome = publisher.publish_moved(RosString {
        data: "two owners".to_owned(),
    })?;
    assert_eq!(
        outcome,
        Published::BusAndWire(Delivery::NoTaker),
        "known limit: no owned receiver is served, and nothing reports it"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no owned receiver got it");

    assert!(
        publisher
            .publish_shared(Arc::new(RosString {
                data: "control".to_owned(),
            }))
            .is_err(),
        "control: publish_shared refuses, so the owned receivers are registered"
    );
    Ok(())
}

/// `publish` dispatches on ownership: `&T` takes the wire and `Arc<T>` the
/// bus, and each reports the route it took. The `Arc` arrives unchanged.
#[test]
fn ownership_selects_the_route_through_the_generic_publish() -> hiroz::Result<()> {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router)?;
    let node = ctx.create_node("dispatch").build()?;

    let hits = Arc::new(AtomicUsize::new(0));
    let received: Arc<Mutex<Vec<Arc<RosString>>>> = Arc::new(Mutex::new(Vec::new()));
    let h = hits.clone();
    let r = received.clone();
    let _sub = node
        .create_sub::<RosString>("dispatch_topic")
        .build_with_shared_callback(move |m: Arc<RosString>| {
            r.lock().expect("lock").push(m);
            h.fetch_add(1, Ordering::SeqCst);
        })?;

    let publisher = node
        .create_pub::<RosString>("dispatch_topic")
        .with_intra_process_only()
        .build()?;
    wait_for_ready(Duration::from_millis(300));

    // Arc<T> -> the bus, through the trait rather than publish_shared.
    let sent = Arc::new(RosString {
        data: "by arc".to_owned(),
    });
    let shared = publisher.publish(Arc::clone(&sent))?;
    assert_eq!(
        shared,
        Published::Bus(Delivery::Sent(1)),
        "an Arc argument must select the bus, got {shared:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the bus subscriber must be served"
    );
    assert!(
        Arc::ptr_eq(&sent, &received.lock().expect("lock")[0]),
        "the subscriber got a copy, not the published allocation"
    );

    // &T -> the wire, even on this intra-process-only publisher. The wire copy
    // reaches the same subscriber through deserialization, so it is a
    // different allocation.
    let borrowed = publisher.publish(&*sent)?;
    assert_eq!(
        borrowed,
        Published::Wire,
        "a &T argument must select the wire, got {borrowed:?}"
    );
    assert!(
        wait_until(|| hits.load(Ordering::SeqCst) == 2),
        "the wire copy of the borrowed message never arrived"
    );
    let got = received.lock().expect("lock");
    assert!(
        !Arc::ptr_eq(&sent, &got[1]),
        "the borrowed publish reached the subscriber as the same allocation, so it took the bus"
    );
    assert_eq!(got[1].data, "by arc");
    Ok(())
}
