//! The plain-path callback dispatcher must honour `KEEP_LAST(depth)`.
//!
//! A callback subscriber cannot run user code on the publishing thread — that is
//! what made re-entrant publishes recurse — so locally published samples are
//! handed to `CallbackDispatcher`'s queue and delivered on its own thread. That
//! queue is the subscriber's history buffer, and it must behave like the one the
//! queue-mode path uses (`BoundedQueue`): retain the last `depth` undelivered
//! samples and drop the *oldest* on overflow.
//!
//! Two properties are asserted, one per direction of the bound:
//!
//! * `KeepLast(n)` drops, and drops the oldest — an unbounded queue delivers the
//!   *first* `n` after the burst instead of the last, so the assertion on
//!   contents (not merely on count) fails if the bound is removed.
//! * `KeepAll` does not drop — the lossless branch is still reachable.
//!
//! Both are made deterministic by blocking the drain thread inside the first
//! callback until the whole burst has been published, so the race between
//! producer and drain thread is removed rather than slept on.

mod common;

use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{
    Builder, TypeHash,
    qos::{QosDurability, QosHistory, QosProfile},
    ros_msg::MessageTypeInfo,
};
use serde::{Deserialize, Serialize};
use serial_test::serial;

/// Budget for one scenario. Generous relative to the work done.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(30);

/// History depth under test. Small enough that a 50-message burst overflows it
/// many times over, large enough that an off-by-one would be visible.
const DEPTH: usize = 4;

/// Messages published after the drain thread is parked. Counter values are
/// `0..BURST`.
const BURST: u64 = 50;

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

/// Run `scenario` on its own thread and fail (rather than hang) if it does not
/// finish within [`SCENARIO_TIMEOUT`].
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

/// A latch the first callback parks on until the publisher releases it.
#[derive(Default)]
struct Latch {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    fn wait(&self) {
        let mut open = self.open.lock().expect("latch poisoned");
        while !*open {
            open = self.changed.wait(open).expect("latch poisoned");
        }
    }

    fn release(&self) {
        *self.open.lock().expect("latch poisoned") = true;
        self.changed.notify_all();
    }
}

/// Publish `0..BURST` into a callback subscriber whose drain thread is parked on
/// the first message, then release it and return everything the callback saw.
///
/// Returns once the delivered sequence has been quiet for 500 ms, so the caller
/// asserts on a settled result rather than on a snapshot mid-drain.
fn burst_through_dispatcher(
    endpoint: &str,
    node: &str,
    topic: &str,
    history: QosHistory,
    durability: QosDurability,
) -> Vec<u64> {
    let qos = QosProfile {
        history,
        durability,
        ..Default::default()
    };

    let ctx = create_hiroz_context_with_endpoint(endpoint).expect("failed to create context");
    let node = ctx
        .create_node(node)
        .build()
        .expect("failed to create node");

    let publisher = node
        .create_pub::<Seq>(topic)
        .with_qos(qos)
        .build()
        .expect("failed to create publisher");

    let seen = Arc::new(Mutex::new(Vec::<u64>::new()));
    let latch = Arc::new(Latch::default());
    let parked = Arc::new(AtomicBool::new(false));

    let cb_seen = seen.clone();
    let cb_latch = latch.clone();
    let cb_parked = parked.clone();
    let _sub = node
        .create_sub::<Seq>(topic)
        .with_qos(qos)
        .build_with_callback(move |msg: Seq| {
            cb_seen.lock().expect("seen poisoned").push(msg.counter);
            if msg.counter == 0 {
                // The drain thread is now provably inside the callback, so every
                // subsequent publish lands in the queue rather than racing it.
                cb_parked.store(true, Ordering::SeqCst);
                cb_latch.wait();
            }
        })
        .expect("failed to create subscriber");

    thread::sleep(Duration::from_millis(300));

    publisher
        .publish(&Seq { counter: 0 })
        .expect("seed publish failed");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !parked.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the drain thread never entered the callback"
        );
        thread::sleep(Duration::from_millis(10));
    }

    for counter in 1..BURST {
        publisher
            .publish(&Seq { counter })
            .expect("burst publish failed");
    }

    latch.release();

    // Settle: stop once the delivered sequence has not changed for 500 ms.
    let mut last = 0usize;
    let mut stable_since = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let len = seen.lock().expect("seen poisoned").len();
        if len != last {
            last = len;
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= Duration::from_millis(500) {
            break;
        }
        assert!(Instant::now() < deadline, "delivery never settled");
        thread::sleep(Duration::from_millis(25));
    }

    seen.lock().expect("seen poisoned").clone()
}

/// `KeepLast(DEPTH)` must retain the newest `DEPTH` undelivered samples.
///
/// The first message is already out of the queue (the drain thread is parked
/// holding it), so the settled sequence is `[0]` followed by the last `DEPTH` of
/// the burst. Asserting the *values* rather than the count is what makes this a
/// drop-**oldest** detector: an unbounded queue yields `0,1,2,…` and a
/// drop-newest queue yields `0,1,2,3,4`.
#[test]
#[serial]
fn keep_last_drops_the_oldest_local_samples() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("keep_last_drops_oldest", move || {
        let delivered = burst_through_dispatcher(
            &endpoint,
            "dispatch_keep_last",
            "/dispatch_keep_last",
            QosHistory::KeepLast(NonZeroUsize::new(DEPTH).unwrap()),
            QosDurability::Volatile,
        );

        let mut expected = vec![0];
        expected.extend((BURST - DEPTH as u64)..BURST);

        assert_eq!(
            delivered, expected,
            "a KeepLast({DEPTH}) callback subscriber must deliver the seed plus the \
             last {DEPTH} of the burst, dropping the oldest in between"
        );
    });
}

/// `KeepAll` must not drop: the queue is unbounded on that profile, matching
/// `BoundedQueue::new(usize::MAX)` on the queue-mode path.
#[test]
#[serial]
fn keep_all_delivers_every_local_sample() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("keep_all_lossless", move || {
        let delivered = burst_through_dispatcher(
            &endpoint,
            "dispatch_keep_all",
            "/dispatch_keep_all",
            QosHistory::KeepAll,
            QosDurability::Volatile,
        );

        let expected: Vec<u64> = (0..BURST).collect();
        assert_eq!(
            delivered, expected,
            "a KeepAll callback subscriber must not drop"
        );
    });
}

/// The **advanced** path must honour `KeepLast(DEPTH)` too.
///
/// `TransientLocal` routes through `AdvancedSubscriber`, a different construction
/// site with its own `CallbackDispatcher::spawn` call. That site passed
/// `DISPATCH_UNBOUNDED` unconditionally, so a `KeepLast` subscriber got an
/// unbounded queue — and because the advanced path's shim enqueues *remote*
/// samples too, that replaced zenoh's transport backpressure with unbounded
/// growth. Nothing detected it: every other test in this file takes the plain
/// path.
///
/// Asserting values rather than a count makes this a drop-**oldest** detector in
/// both directions, exactly as on the plain path: unbounded yields `0,1,2,…`,
/// drop-newest yields the first `DEPTH + 1`.
#[test]
#[serial]
fn transient_local_keep_last_drops_the_oldest_local_samples() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("transient_local_keep_last_drops_oldest", move || {
        let delivered = burst_through_dispatcher(
            &endpoint,
            "dispatch_tl_keep_last",
            "/dispatch_tl_keep_last",
            QosHistory::KeepLast(NonZeroUsize::new(DEPTH).unwrap()),
            QosDurability::TransientLocal,
        );

        let mut expected = vec![0];
        expected.extend((BURST - DEPTH as u64)..BURST);

        assert_eq!(
            delivered, expected,
            "a TransientLocal KeepLast({DEPTH}) callback subscriber must deliver the \
             seed plus the last {DEPTH} of the burst; an unbounded advanced queue \
             delivers all {BURST}"
        );
    });
}

/// `KeepAll` on the advanced path stays lossless.
///
/// This is the half of the previous test's argument that survives: replaying
/// history and recovering missed samples is what `TransientLocal` is for, so a
/// profile that asks to keep everything must keep everything. Bounding by the
/// declared depth must not have collapsed this case too.
#[test]
#[serial]
fn transient_local_keep_all_delivers_every_local_sample() {
    let router = TestRouter::new();
    let endpoint = router.endpoint().to_string();

    run_with_deadline("transient_local_keep_all_lossless", move || {
        let delivered = burst_through_dispatcher(
            &endpoint,
            "dispatch_tl_keep_all",
            "/dispatch_tl_keep_all",
            QosHistory::KeepAll,
            QosDurability::TransientLocal,
        );

        let expected: Vec<u64> = (0..BURST).collect();
        assert_eq!(
            delivered, expected,
            "a TransientLocal KeepAll callback subscriber must not drop"
        );
    });
}
