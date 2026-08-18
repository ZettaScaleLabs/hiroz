//! The plain path's INLINE branch must survive a panicking callback.
//!
//! `CallbackDispatcher`'s drain loop wraps the user callback in `catch_unwind`.
//! The plain path does not always use that loop: `local_only_shim` enqueues only
//! the samples the delivering thread published itself, and calls the handler
//! inline for every other one — which is every sample that arrived over a
//! transport.
//!
//! That inline branch is the ROS 2 default. `qos_needs_advanced` is true only
//! for `TransientLocal` durability and the default is `Volatile`, so a default
//! subscriber takes the plain path and its inter-process traffic is exactly the
//! inline case. Before the guard was added there, a panicking callback unwound
//! out of hiroz and into a zenoh receive worker.
//!
//! # How this test avoids passing for the wrong reason
//!
//! Three properties, each of which would otherwise let it pass vacuously.
//!
//! 1. **The sample must be remote.** The shim's selector is
//!    [`local_publish_active`], a THREAD-LOCAL depth counter — not session and
//!    not process locality. A publish on the same thread takes the `if` arm and
//!    exercises the already-guarded queue path, proving nothing about the
//!    branch under test. Publisher and subscriber therefore use separate
//!    contexts through a router, and the test asserts the delivering thread was
//!    a zenoh receive worker rather than `hiroz-sub-drain`.
//!
//! 2. **Panics must unwind.** `catch_unwind` can only return `Err` where they
//!    do, and this workspace's `[profile.opt]` sets `panic = "abort"`. The file
//!    is gated so it SKIPS visibly on such a profile instead of passing.
//!
//! 3. **A positive control.** `delivery_continues_without_a_panic` runs the
//!    identical shape with the panic removed. Without it, "the later samples
//!    never arrived" would be indistinguishable from "delivery never worked
//!    here at all".
//!
//! # Revert direction
//!
//! Remove the `catch_unwind` from `local_only_shim`'s `else` arm in
//! `crates/hiroz/src/pubsub.rs` — leaving a bare `handler(sample)` — and
//! `a_panicking_callback_on_a_remote_sample_does_not_stop_delivery` must fail.
//!
//! Part of #296 (tag G7).

#![cfg(panic = "unwind")]

mod common;

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{Builder, TypeHash, ros_msg::MessageTypeInfo};
use serde::{Deserialize, Serialize};
use serial_test::serial;

/// Budget for one scenario. Generous relative to the work done.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(30);

/// Samples published. The callback panics on exactly one of them.
const COUNT: u64 = 12;

/// Which sample panics. Chosen so that several arrive before it and several
/// after, making "delivery stopped" distinguishable from "delivery never
/// started".
const PANIC_AT: u64 = 4;

/// How long to wait for the delivered set to settle before asserting.
const SETTLE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Seq {
    counter: u64,
}

impl MessageTypeInfo for Seq {
    fn type_name() -> &'static str {
        "test_msgs::msg::dds_::Seq_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl hiroz::ros_msg::WithTypeInfo for Seq {}

impl hiroz::msg::ZMessage for Seq {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Seq>;
}

/// Run `scenario` on its own thread and fail rather than hang.
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
        panic!("`{name}` did not finish within {SCENARIO_TIMEOUT:?}");
    }
}

/// What one run observed.
struct Observed {
    /// Counter values the callback received.
    seen: BTreeSet<u64>,
    /// Names of the threads the callback ran on.
    threads: BTreeSet<String>,
}

/// Publish `0..COUNT` from a SEPARATE context through `router`, into a
/// default-QoS callback subscriber. `panic_at` makes the callback panic on that
/// counter value; `None` is the positive control.
fn deliver_remote(endpoint: &str, topic: &str, panic_at: Option<u64>) -> Observed {
    let sub_ctx = create_hiroz_context_with_endpoint(endpoint).expect("subscriber context");
    let pub_ctx = create_hiroz_context_with_endpoint(endpoint).expect("publisher context");

    let sub_node = sub_ctx
        .create_node("panic_guard_sub")
        .build()
        .expect("sub node");
    let pub_node = pub_ctx
        .create_node("panic_guard_pub")
        .build()
        .expect("pub node");

    let seen: Arc<Mutex<BTreeSet<u64>>> = Arc::new(Mutex::new(BTreeSet::new()));
    let threads: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
    let received = Arc::new(AtomicU64::new(0));

    let c_seen = seen.clone();
    let c_threads = threads.clone();
    let c_recv = received.clone();

    // Default QoS: Volatile, so this is the PLAIN path.
    let _sub = sub_node
        .create_sub::<Seq>(topic)
        .build_with_callback(move |msg: Seq| {
            c_threads
                .lock()
                .expect("threads poisoned")
                .insert(thread::current().name().unwrap_or("<unnamed>").to_string());
            c_seen.lock().expect("seen poisoned").insert(msg.counter);
            c_recv.fetch_add(1, Ordering::Relaxed);
            if panic_at == Some(msg.counter) {
                panic!(
                    "deliberate panic in a subscriber callback, counter={}",
                    msg.counter
                );
            }
        })
        .expect("subscriber");

    let zpub = pub_node
        .create_pub::<Seq>(topic)
        .build()
        .expect("publisher");

    // Let discovery settle so the first samples are not lost to a race. A lost
    // early sample would weaken the "delivery started" half of the assertion.
    thread::sleep(Duration::from_millis(500));

    for counter in 0..COUNT {
        zpub.publish(&Seq { counter }).expect("publish");
        thread::sleep(Duration::from_millis(50));
    }

    // Wait for the delivered set to go quiet rather than sleeping a fixed time.
    let mut last = u64::MAX;
    for _ in 0..(SETTLE.as_millis() / 100) {
        let now = received.load(Ordering::Relaxed);
        if now == last {
            break;
        }
        last = now;
        thread::sleep(Duration::from_millis(100));
    }

    let seen = seen.lock().expect("seen poisoned").clone();
    let threads = threads.lock().expect("threads poisoned").clone();
    Observed { seen, threads }
}

/// GUARD 1: the samples must have arrived over a transport, on a zenoh receive
/// worker. If they came in on `hiroz-sub-drain` the enqueue arm was taken and
/// the already-guarded path was tested instead.
fn assert_took_the_inline_branch(o: &Observed, what: &str) {
    assert!(
        !o.threads.is_empty(),
        "{what}: the callback never ran, so nothing was tested"
    );
    assert!(
        !o.threads.contains("hiroz-sub-drain"),
        "{what}: delivery ran on `hiroz-sub-drain`, so the sample took \
         local_only_shim's enqueue arm. That is the queue path, which the drain \
         loop already guards — this test measured the wrong branch. \
         Threads seen: {:?}",
        o.threads
    );
}

#[test]
#[serial]
fn a_panicking_callback_on_a_remote_sample_does_not_stop_delivery() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("panic_guard_inline", move || {
        let o = deliver_remote(&endpoint, "/panic_guard_inline", Some(PANIC_AT));

        assert_took_the_inline_branch(&o, "panic run");

        assert!(
            o.seen.contains(&PANIC_AT),
            "the panicking sample itself never arrived, so the panic never \
             happened and nothing was tested. Seen: {:?}",
            o.seen
        );

        let after: Vec<u64> = o.seen.iter().copied().filter(|c| *c > PANIC_AT).collect();
        assert!(
            !after.is_empty(),
            "no sample after the panic was delivered: the panicking callback \
             stopped the subscriber. The inline branch of `local_only_shim` \
             needs the same `catch_unwind` the drain loop has. Seen: {:?}",
            o.seen
        );
    });
}

/// GUARD 3: the positive control. Without it, a failure above could mean
/// "delivery never worked in this configuration" rather than "the panic
/// stopped it".
#[test]
#[serial]
fn delivery_continues_without_a_panic() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("panic_guard_control", move || {
        let o = deliver_remote(&endpoint, "/panic_guard_control", None);

        assert_took_the_inline_branch(&o, "control run");

        let after: Vec<u64> = o.seen.iter().copied().filter(|c| *c > PANIC_AT).collect();
        assert!(
            !after.is_empty(),
            "the control run delivered nothing after counter {PANIC_AT}, so this \
             configuration cannot deliver at all and the panic test above proves \
             nothing either way. Seen: {:?}",
            o.seen
        );
    });
}
