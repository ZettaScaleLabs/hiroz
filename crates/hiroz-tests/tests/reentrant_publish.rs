//! Re-entrant publish from inside a subscriber callback must not deadlock.
//!
//! Every hiroz subscriber used to be declared as a zenoh-ext `AdvancedSubscriber`,
//! whose sample callback runs the user closure while a non-reentrant
//! `std::sync::Mutex` guard is alive (`sub_callback` takes `zlock!(statesref)`,
//! then `handle_sample` calls the callback under it). Zenoh core dispatches
//! samples published on the same session synchronously on the publishing thread,
//! so publishing from inside a callback re-enters that mutex on the very thread
//! that already holds it — a deterministic self-deadlock.
//!
//! Each scenario runs on a dedicated thread and reports through a channel, so a
//! hang fails the test instead of wedging the harness.
//!
//! Two families of scenario run here, because hiroz uses two different subscriber
//! implementations depending on QoS:
//!
//! * **Volatile** (the ROS 2 default) declares a plain zenoh subscriber, whose
//!   callback runs inline with no lock held.
//! * **TransientLocal** must keep the `AdvancedSubscriber` — history replay and
//!   sample-miss recovery live there — so its user callback is moved onto a
//!   dedicated delivery thread and zenoh-ext only ever gets an enqueue-only shim.
//!
//! Both must survive the same re-entrancy scenarios, and with the same
//! observable semantics, so each scenario is written twice.

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{
    Builder, TypeHash,
    qos::{QosDurability, QosProfile},
    ros_msg::MessageTypeInfo,
};
use serde::{Deserialize, Serialize};
use serial_test::serial;

/// QoS that forces hiroz down the zenoh-ext `AdvancedSubscriber` path.
///
/// This is the case the `qos_needs_advanced` gate deliberately does *not* cover:
/// the advanced entity is genuinely needed here, so the deadlock has to be solved
/// rather than side-stepped.
fn transient_local() -> QosProfile {
    QosProfile {
        durability: QosDurability::TransientLocal,
        ..Default::default()
    }
}

/// Budget for one scenario. Generous relative to the work done (a handful of
/// intra-process publishes) — anything slower than this is a hang, not slowness.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for the expected number of deliveries once seeded.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Run `scenario` on its own thread and fail (rather than hang) if it does not
/// finish within [`SCENARIO_TIMEOUT`].
///
/// On timeout the worker thread is deliberately left running: it is blocked on a
/// mutex it can never acquire and cannot be unwound. Each scenario owns its own
/// context and router, and the test binary exits shortly after.
fn run_with_deadline(name: &'static str, scenario: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            scenario();
            let _ = tx.send(());
        })
        .expect("failed to spawn scenario thread");

    if rx.recv_timeout(SCENARIO_TIMEOUT).is_err() {
        panic!(
            "`{name}` did not finish within {SCENARIO_TIMEOUT:?} — \
             re-entrant publish from a subscriber callback deadlocked"
        );
    }
}

/// Block until `seen` reaches `expected`, or fail with what actually arrived.
fn await_deliveries(seen: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while seen.load(Ordering::SeqCst) < expected {
        assert!(
            Instant::now() < deadline,
            "only {} of {expected} messages delivered",
            seen.load(Ordering::SeqCst)
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// A callback that publishes back onto its own topic must make progress.
///
/// The minimal shape of the bug: one subscriber, one publisher, one session. The
/// callback re-publishes for a bounded number of hops so the scenario terminates
/// by construction rather than relying on the fix to bound it.
#[test]
#[serial]
fn callback_republishing_on_same_topic_does_not_deadlock() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("same_topic", move || {
        const HOPS: u64 = 4;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_same_topic")
            .build()
            .expect("failed to create node");

        let publisher = Arc::new(
            node.create_pub::<Tick>("/reentrant_same")
                .build()
                .expect("failed to create publisher"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub = publisher.clone();
        let cb_seen = seen.clone();
        let _sub = node
            .create_sub::<Tick>("/reentrant_same")
            .build_with_callback(move |msg: Tick| {
                cb_seen.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    // Re-entrant publish on the same session, from inside the
                    // subscriber callback. This is what used to deadlock.
                    cb_pub
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("re-entrant publish failed");
                }
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, HOPS as usize + 1);
    });
}

/// Two callbacks publishing to each other's topic must make progress.
///
/// Distinct from the same-topic case: two *different* subscribers, and so two
/// different zenoh-ext state mutexes, form a cycle — the shape a real ping-pong
/// node pair has. The deadlock still occurs, one frame later.
#[test]
#[serial]
fn callback_cycle_across_two_topics_does_not_deadlock() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("two_topics", move || {
        const HOPS: u64 = 4;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_cycle")
            .build()
            .expect("failed to create node");

        let pub_a = Arc::new(
            node.create_pub::<Tick>("/reentrant_a")
                .build()
                .expect("failed to create publisher a"),
        );
        let pub_b = Arc::new(
            node.create_pub::<Tick>("/reentrant_b")
                .build()
                .expect("failed to create publisher b"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub_b = pub_b.clone();
        let cb_seen_a = seen.clone();
        let _sub_a = node
            .create_sub::<Tick>("/reentrant_a")
            .build_with_callback(move |msg: Tick| {
                cb_seen_a.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    cb_pub_b
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("publish a->b failed");
                }
            })
            .expect("failed to create subscriber a");

        let cb_pub_a = pub_a.clone();
        let cb_seen_b = seen.clone();
        let _sub_b = node
            .create_sub::<Tick>("/reentrant_b")
            .build_with_callback(move |msg: Tick| {
                cb_seen_b.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    cb_pub_a
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("publish b->a failed");
                }
            })
            .expect("failed to create subscriber b");

        thread::sleep(Duration::from_millis(300));
        pub_a
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, HOPS as usize + 1);
    });
}

/// A self-feeding callback loop must *iterate*, not recurse.
///
/// This is the detector for the whole point of routing session-local delivery
/// through the dispatcher. A callback that republishes to its own topic used to
/// be reached inline from inside `publish()`, so the loop was recursion: it grew
/// the stack, so before this fix it could not be expressed as a loop at all and
/// drop samples past the cap to avoid a `SIGSEGV`.
///
/// Now the sample is enqueued and the callback runs on the dispatcher thread, so
/// each iteration returns to a flat stack before the next begins. The loop runs
/// indefinitely at constant stack depth and constant queue depth.
///
/// The assertion is deliberately the *inverse* of the old one: it requires the
/// loop to exceed the old cap by orders of magnitude. Against the pre-fix
/// sources this fails at 16.
#[test]
#[serial]
fn self_feeding_callback_loop_iterates_without_a_depth_cap() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("unbounded_loop", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_unbounded")
            .build()
            .expect("failed to create node");

        let publisher = Arc::new(
            node.create_pub::<Tick>("/reentrant_unbounded")
                .build()
                .expect("failed to create publisher"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub = publisher.clone();
        let cb_seen = seen.clone();
        let _sub = node
            .create_sub::<Tick>("/reentrant_unbounded")
            .build_with_callback(move |msg: Tick| {
                // Stop feeding well before the deadline so the test ends on its
                // own; the loop's *ability* to keep going is what is under test.
                if cb_seen.fetch_add(1, Ordering::SeqCst) >= ITERATION_TARGET {
                    return;
                }
                let _ = cb_pub.publish(&Tick {
                    counter: msg.counter + 1,
                });
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, ITERATION_TARGET);

        let delivered = seen.load(Ordering::SeqCst);
        assert!(
            delivered >= ITERATION_TARGET,
            "self-feeding loop stalled at {delivered} deliveries (target {ITERATION_TARGET}). \
             Under the old inline dispatch this deadlocks on the first hop."
        );
    });
}

/// How far the self-feeding loops must run to prove they are iterative.
///
/// Two orders of magnitude above the old depth cap of 16, and well past the
/// stack depth a recursive implementation survives: with the fix reverted these
/// same tests die with `fatal runtime error: stack overflow` rather than merely
/// falling short of the target.
///
/// Deliberately not larger. At 20_000 these loops saturate a core for long
/// enough to perturb the *next* test binary in a sequential run — `parameter_tests`
/// began failing three service calls on timeout purely from the load, with no
/// code path in common (verified: the dispatcher branch is never taken in that
/// suite). The property under test is "does it iterate at all", which 2_000
/// settles just as conclusively as 20_000.
const ITERATION_TARGET: usize = 2_000;

// ---------------------------------------------------------------------------
// TransientLocal variants
//
// These exercise the `AdvancedSubscriber` path, which cannot avoid holding its
// state mutex across the user callback: `handle_sample` interleaves
// `callback.call(sample)` with mutation of `last_delivered`/`pending_samples`.
// Instead the callback zenoh-ext receives only enqueues, and hiroz runs the real
// callback on a dedicated delivery thread with no lock held.
// ---------------------------------------------------------------------------

/// TransientLocal counterpart of
/// [`callback_republishing_on_same_topic_does_not_deadlock`].
#[test]
#[serial]
fn transient_local_callback_republishing_on_same_topic_does_not_deadlock() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_same_topic", move || {
        const HOPS: u64 = 4;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_same_topic")
            .build()
            .expect("failed to create node");

        let publisher = Arc::new(
            node.create_pub::<Tick>("/reentrant_tl_same")
                .with_qos(transient_local())
                .build()
                .expect("failed to create publisher"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub = publisher.clone();
        let cb_seen = seen.clone();
        let _sub = node
            .create_sub::<Tick>("/reentrant_tl_same")
            .with_qos(transient_local())
            .build_with_callback(move |msg: Tick| {
                cb_seen.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    cb_pub
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("re-entrant publish failed");
                }
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, HOPS as usize + 1);
    });
}

/// TransientLocal counterpart of
/// [`callback_cycle_across_two_topics_does_not_deadlock`].
///
/// Each subscriber owns its own delivery thread, so this also covers a callback
/// on one delivery thread publishing into a *different* advanced subscriber.
#[test]
#[serial]
fn transient_local_callback_cycle_across_two_topics_does_not_deadlock() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_two_topics", move || {
        const HOPS: u64 = 4;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_cycle")
            .build()
            .expect("failed to create node");

        let pub_a = Arc::new(
            node.create_pub::<Tick>("/reentrant_tl_a")
                .with_qos(transient_local())
                .build()
                .expect("failed to create publisher a"),
        );
        let pub_b = Arc::new(
            node.create_pub::<Tick>("/reentrant_tl_b")
                .with_qos(transient_local())
                .build()
                .expect("failed to create publisher b"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub_b = pub_b.clone();
        let cb_seen_a = seen.clone();
        let _sub_a = node
            .create_sub::<Tick>("/reentrant_tl_a")
            .with_qos(transient_local())
            .build_with_callback(move |msg: Tick| {
                cb_seen_a.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    cb_pub_b
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("publish a->b failed");
                }
            })
            .expect("failed to create subscriber a");

        let cb_pub_a = pub_a.clone();
        let cb_seen_b = seen.clone();
        let _sub_b = node
            .create_sub::<Tick>("/reentrant_tl_b")
            .with_qos(transient_local())
            .build_with_callback(move |msg: Tick| {
                cb_seen_b.fetch_add(1, Ordering::SeqCst);
                if msg.counter < HOPS {
                    cb_pub_a
                        .publish(&Tick {
                            counter: msg.counter + 1,
                        })
                        .expect("publish b->a failed");
                }
            })
            .expect("failed to create subscriber b");

        thread::sleep(Duration::from_millis(300));
        pub_a
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, HOPS as usize + 1);
    });
}

/// TransientLocal counterpart of
/// [`self_feeding_callback_loop_iterates_without_a_depth_cap`].
///
/// This path always ran the callback on the dispatcher thread, so it was already
/// iterative — the depth cap was carried across the queue only to keep its
/// behaviour identical to the inline path's. Now that the inline path is gone
/// there is nothing to stay identical to, and both paths iterate. Asserting it
/// here keeps the two in step.
#[test]
#[serial]
fn transient_local_self_feeding_callback_loop_iterates() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_unbounded_loop", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_unbounded")
            .build()
            .expect("failed to create node");

        let publisher = Arc::new(
            node.create_pub::<Tick>("/reentrant_tl_unbounded")
                .with_qos(transient_local())
                .build()
                .expect("failed to create publisher"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_pub = publisher.clone();
        let cb_seen = seen.clone();
        let _sub = node
            .create_sub::<Tick>("/reentrant_tl_unbounded")
            .with_qos(transient_local())
            .build_with_callback(move |msg: Tick| {
                if cb_seen.fetch_add(1, Ordering::SeqCst) >= ITERATION_TARGET {
                    return;
                }
                let _ = cb_pub.publish(&Tick {
                    counter: msg.counter + 1,
                });
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, ITERATION_TARGET);

        let delivered = seen.load(Ordering::SeqCst);
        assert!(
            delivered >= ITERATION_TARGET,
            "self-feeding loop stalled at {delivered} deliveries (target {ITERATION_TARGET})"
        );
    });
}

/// The `intra` closed loop: two topics, two callbacks, one session, each
/// callback feeding the other. This is the shape the `afor` benchmark's `intra`
/// cell drives, and the reason that cell could not produce a number.
///
/// Under inline session-local dispatch this is not a loop at all — it is mutual
/// recursion on one thread, so it either overflows the stack or, with the depth
/// cap, stops dead at 16. Under dispatcher delivery each hop returns to a flat
/// stack, so the loop runs as long as it is fed.
#[test]
#[serial]
fn intra_closed_loop_runs_iteratively() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("intra_closed_loop", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("intra_closed_loop")
            .build()
            .expect("failed to create node");

        let hello_pub = Arc::new(
            node.create_pub::<Tick>("/intra_hello")
                .build()
                .expect("failed to create hello publisher"),
        );
        let world_pub = Arc::new(
            node.create_pub::<Tick>("/intra_world")
                .build()
                .expect("failed to create world publisher"),
        );
        let seen = Arc::new(AtomicUsize::new(0));

        // hello -> world
        let to_world = world_pub.clone();
        let hello_seen = seen.clone();
        let _hello_sub = node
            .create_sub::<Tick>("/intra_hello")
            .build_with_callback(move |msg: Tick| {
                if hello_seen.fetch_add(1, Ordering::SeqCst) >= ITERATION_TARGET {
                    return;
                }
                let _ = to_world.publish(&Tick {
                    counter: msg.counter + 1,
                });
            })
            .expect("failed to create hello subscriber");

        // world -> hello
        let to_hello = hello_pub.clone();
        let world_seen = seen.clone();
        let _world_sub = node
            .create_sub::<Tick>("/intra_world")
            .build_with_callback(move |msg: Tick| {
                if world_seen.fetch_add(1, Ordering::SeqCst) >= ITERATION_TARGET {
                    return;
                }
                let _ = to_hello.publish(&Tick {
                    counter: msg.counter + 1,
                });
            })
            .expect("failed to create world subscriber");

        thread::sleep(Duration::from_millis(300));
        hello_pub
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        await_deliveries(&seen, ITERATION_TARGET);

        let delivered = seen.load(Ordering::SeqCst);
        assert!(
            delivered >= ITERATION_TARGET,
            "intra closed loop stalled at {delivered} hops (target {ITERATION_TARGET}) — \
             this is the `afor` intra cell's failure mode"
        );
    });
}

/// The delivery thread must not reorder samples.
///
/// `AdvancedSubscriber` exists to deliver samples in source order and to recover
/// missed ones; a handoff that reordered them would defeat its entire purpose.
/// The shim enqueues from inside `handle_sample` — i.e. under zenoh-ext's state
/// mutex, in exactly the order zenoh-ext chose to deliver — and a single thread
/// pops FIFO, so the observed order must be the publish order.
///
/// **Ordering, not completeness.** The burst is far deeper than the declared
/// `KeepLast` depth, so the queue drops the oldest — exactly as
/// `rmw_zenoh_cpp`'s `add_new_message` does (`rmw_subscription_data.cpp`:
/// `size() >= adapted_qos_profile.depth` → `pop_front()`, with no
/// `TransientLocal` exemption). Asserting the delivered values were *all*
/// published would therefore assert a promise no RMW makes.
///
/// Asserting a strictly increasing subsequence is the stronger test anyway: it
/// catches reordering whether or not anything was dropped, whereas an equality
/// check conflates the two failures. `keep_all_delivers_every_local_sample` in
/// `dispatch_backpressure.rs` covers losslessness on the profile that promises
/// it.
#[test]
#[serial]
fn transient_local_delivery_preserves_order() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_ordering", move || {
        const COUNT: u64 = 500;

        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_order")
            .build()
            .expect("failed to create node");

        let publisher = node
            .create_pub::<Tick>("/reentrant_tl_order")
            .with_qos(transient_local())
            .build()
            .expect("failed to create publisher");

        let received: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(AtomicUsize::new(0));

        let cb_received = received.clone();
        let cb_seen = seen.clone();
        let _sub = node
            .create_sub::<Tick>("/reentrant_tl_order")
            .with_qos(transient_local())
            .build_with_callback(move |msg: Tick| {
                cb_received.lock().unwrap().push(msg.counter);
                cb_seen.fetch_add(1, Ordering::SeqCst);
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        for counter in 0..COUNT {
            publisher
                .publish(&Tick { counter })
                .expect("publish failed");
        }

        // Settle rather than wait for a fixed count: with a bounded queue the
        // delivered total is a property of scheduling, not of the publish count.
        let mut last = 0usize;
        let mut stable_since = Instant::now();
        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        loop {
            let len = seen.load(Ordering::SeqCst);
            if len != last {
                last = len;
                stable_since = Instant::now();
            } else if stable_since.elapsed() >= Duration::from_millis(300) {
                break;
            }
            assert!(Instant::now() < deadline, "delivery never settled");
            thread::sleep(Duration::from_millis(25));
        }

        let received = received.lock().unwrap();
        assert!(
            !received.is_empty(),
            "nothing was delivered — the scenario proved nothing"
        );
        assert!(
            received.windows(2).all(|w| w[0] < w[1]),
            "delivery thread reordered samples: {received:?}"
        );
        assert!(
            received.iter().all(|&c| c < COUNT),
            "delivered a counter that was never published: {received:?}"
        );
    });
}

/// Dropping the subscriber must shut the delivery thread down — no leak, no hang.
///
/// The sentinel is owned by the user callback, which the delivery thread owns in
/// turn, so its `Drop` firing proves the thread actually exited and released the
/// closure rather than being detached and left running.
#[test]
#[serial]
fn transient_local_subscriber_drop_shuts_down_delivery_thread() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_drop", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_drop")
            .build()
            .expect("failed to create node");

        let publisher = node
            .create_pub::<Tick>("/reentrant_tl_drop")
            .with_qos(transient_local())
            .build()
            .expect("failed to create publisher");

        /// Fires when the delivery thread drops the user callback.
        struct Sentinel(Arc<AtomicBool>);
        impl Drop for Sentinel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let callback_dropped = Arc::new(AtomicBool::new(false));
        let sentinel = Sentinel(callback_dropped.clone());
        let seen = Arc::new(AtomicUsize::new(0));
        let cb_seen = seen.clone();

        let sub = node
            .create_sub::<Tick>("/reentrant_tl_drop")
            .with_qos(transient_local())
            .build_with_callback(move |_msg: Tick| {
                // Keep the sentinel owned by the callback.
                let _ = &sentinel;
                cb_seen.fetch_add(1, Ordering::SeqCst);
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");
        await_deliveries(&seen, 1);

        assert!(
            !callback_dropped.load(Ordering::SeqCst),
            "callback was dropped while the subscriber was still alive"
        );

        // If `Drop` deadlocked or the join hung, the deadline guard fails the test.
        drop(sub);

        assert!(
            callback_dropped.load(Ordering::SeqCst),
            "subscriber dropped without shutting down its delivery thread — \
             the thread is leaked"
        );
    });
}

/// Dropping the subscriber *from inside its own callback* must not deadlock.
///
/// That drop runs on the delivery thread itself, so joining the thread would be
/// a self-join. The dispatcher detects this and lets the thread wind itself down
/// instead.
#[test]
#[serial]
fn transient_local_subscriber_dropped_inside_its_own_callback_does_not_deadlock() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("tl_self_drop", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_tl_self_drop")
            .build()
            .expect("failed to create node");

        let publisher = node
            .create_pub::<Tick>("/reentrant_tl_self_drop")
            .with_qos(transient_local())
            .build()
            .expect("failed to create publisher");

        // The subscriber hands itself to its own callback, which drops it.
        let slot: Arc<Mutex<Option<Box<dyn std::any::Any + Send>>>> = Arc::new(Mutex::new(None));
        let dropped = Arc::new(AtomicBool::new(false));

        let cb_slot = slot.clone();
        let cb_dropped = dropped.clone();
        let sub = node
            .create_sub::<Tick>("/reentrant_tl_self_drop")
            .with_qos(transient_local())
            .build_with_callback(move |_msg: Tick| {
                // Runs on the delivery thread; this drop is the self-join case.
                let taken = cb_slot.lock().unwrap().take();
                drop(taken);
                cb_dropped.store(true, Ordering::SeqCst);
            })
            .expect("failed to create subscriber");

        *slot.lock().unwrap() = Some(Box::new(sub));

        thread::sleep(Duration::from_millis(300));
        publisher
            .publish(&Tick { counter: 0 })
            .expect("seed publish failed");

        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        while !dropped.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "subscriber was never dropped from its own callback"
            );
            thread::sleep(Duration::from_millis(20));
        }
    });
}

/// `async_publish` must also hand session-local delivery to the drain thread.
///
/// Every other scenario in this file drives the *synchronous* `publish`, so the
/// async path's guard was unpinned. It is guarded differently, and the
/// difference is load-bearing: a thread-local held across an `.await` would be
/// observed on whatever thread resumed the task, so `async_publish` scopes
/// `LocalPublishGuard` to the `into_future()` call rather than the await. That
/// is only sufficient because zenoh resolves a put eagerly there --
/// `IntoFuture for PublicationBuilder<_, PublicationBuilderPut>` is
/// `std::future::ready(self.wait())` (zenoh 1.9.0, `api/builders/publisher.rs`).
///
/// That is an upstream implementation detail, not a documented contract. Should
/// zenoh ever make the put lazy, it would move outside the guard, session-local
/// samples would again be dispatched inline on the publishing thread, and every
/// deadlock this file exists to prevent would return on the async path with no
/// other test noticing.
///
/// The detector is thread identity rather than a hang, so it fails immediately
/// and for a legible reason instead of timing out: with the guard covering the
/// put, the callback runs on `hiroz-sub-drain`; without it, on the publishing
/// thread.
#[test]
#[serial]
fn async_publish_delivers_off_the_publishing_thread() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("async_publish_off_thread", move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("failed to create context");
        let node = ctx
            .create_node("reentrant_async_publish")
            .build()
            .expect("failed to create node");

        let publisher = node
            .create_pub::<Tick>("/reentrant_async")
            .build()
            .expect("failed to create publisher");

        let (tx, rx) = mpsc::channel();
        let _sub = node
            .create_sub::<Tick>("/reentrant_async")
            .build_with_callback(move |_msg: Tick| {
                let _ = tx.send(thread::current().id());
            })
            .expect("failed to create subscriber");

        thread::sleep(Duration::from_millis(300));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");
        let publishing_thread = runtime.block_on(async {
            publisher
                .async_publish(&Tick { counter: 0 })
                .await
                .expect("async publish failed");
            thread::current().id()
        });

        let callback_thread = rx
            .recv_timeout(DELIVERY_TIMEOUT)
            .expect("async_publish produced no delivery");

        assert_ne!(
            callback_thread, publishing_thread,
            "async_publish delivered the sample inline on the publishing thread: \
             the local-publish guard did not cover the put, so a callback that \
             publishes would recurse instead of iterate"
        );
    });
}
