//! Re-entrancy audit for endpoint event-status updates.
//!
//! An `EventsManager` lives behind an `Arc<Mutex<..>>` that its owners share
//! with `RmEventHandle`. `update_event_status` takes `&mut self`, so every
//! caller necessarily holds that outer mutex — and the method fires the
//! registered callback. The callback is user code: the rmw layer hands it
//! straight to an rclcpp executor, and the first thing such a callback does is
//! usually ask the handle that fired for the status behind it
//! (`rmw_take_event` → `RmEventHandle::take_event`), which locks the very same
//! mutex. On a non-reentrant `std::sync::Mutex` that is a self-deadlock on the
//! thread already holding the guard, with no race needed.
//!
//! The fix is the shape used throughout this module and by zenoh core in
//! `resolve_put`: record under the lock, drop the guard, then invoke.
//! `EventsManager::record_event_status_with_policy` returns the callback
//! instead of calling it, and `update_shared_event_status[_with_policy]` is the
//! entry point every `Arc<Mutex<EventsManager>>` holder uses.
//!
//! Each scenario runs on a dedicated thread behind a hard deadline, so a
//! re-entrancy deadlock fails the test instead of wedging the suite — the same
//! shape as `reentrant_graph_event.rs`.
//!
//! # These are not A/B detectors
//!
//! Both tests call `update_shared_event_status`, which this change introduces.
//! Reverting the production code does not make them fail — it makes this file
//! stop compiling. They show the new entry point is re-entrancy-safe; they are
//! not evidence that the old shape deadlocked. The detectors for that live in
//! `reentrant_graph_event.rs`, which does go red on a revert.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use hiroz::{
    GidArray,
    event::{
        EventsManager, RmEventHandle, ZenohEventType, update_shared_event_status,
        update_shared_event_status_with_policy,
    },
};

/// Budget for one scenario. Generous relative to the work done — anything
/// slower than this is a hang, not slowness.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `scenario` on its own thread; fail (rather than hang) past the deadline.
///
/// On timeout the worker is deliberately left running: it is blocked on a lock
/// that will never be released, and there is no sound way to unwind it.
fn with_deadline(name: &'static str, scenario: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        scenario();
        let _ = tx.send(());
    });
    match rx.recv_timeout(SCENARIO_TIMEOUT) {
        Ok(()) => {}
        // The worker panicked and dropped the sender. That is an assertion
        // failure inside the scenario, NOT a deadlock — reporting it as one
        // would turn every ordinary test failure into a false deadlock report.
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{name}: scenario panicked — see the worker thread's panic above")
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{name}: scenario did not finish within {SCENARIO_TIMEOUT:?} — deadlock")
        }
    }
}

fn gid(n: u8) -> GidArray {
    let mut g = [0u8; 16];
    g[0] = n;
    g
}

/// The canonical rmw shape: the matched-event callback immediately takes the
/// status that triggered it.
///
/// `take_event` locks the same `Arc<Mutex<EventsManager>>` the update path
/// holds, so with the callback invoked under that guard this never returns.
#[test]
fn event_callback_taking_its_own_status_does_not_deadlock() {
    with_deadline("event_callback_take_event", || {
        let mgr = Arc::new(Mutex::new(EventsManager::new(gid(1))));
        let handle = Arc::new(RmEventHandle::new(
            mgr.clone(),
            ZenohEventType::SubscriptionMatched,
        ));

        let observed = Arc::new(AtomicI32::new(-1));
        {
            let handle_in_cb = handle.clone();
            let observed = observed.clone();
            handle.set_callback(move |_change| {
                let status = handle_in_cb.take_event();
                observed.store(status.total_count, Ordering::SeqCst);
            });
        }

        update_shared_event_status(&mgr, ZenohEventType::SubscriptionMatched, 1);

        assert_eq!(
            observed.load(Ordering::SeqCst),
            1,
            "the callback did not observe the status change that triggered it"
        );
        // The callback consumed the change counters via `take_event`.
        assert!(
            !handle.is_ready(),
            "take_event inside the callback should have cleared the changed flag"
        );
    });
}

/// A QoS-incompatibility callback that re-arms itself.
///
/// `set_callback` locks the manager to install, so a callback that replaces
/// itself re-enters the outer mutex exactly like `take_event` does. This also
/// covers the `_with_policy` entry point, which carries the encoded policy kind.
#[test]
fn event_callback_reinstalling_itself_does_not_deadlock() {
    with_deadline("event_callback_reinstall", || {
        let mgr = Arc::new(Mutex::new(EventsManager::new(gid(2))));
        let handle = Arc::new(RmEventHandle::new(
            mgr.clone(),
            ZenohEventType::RequestedQosIncompatible,
        ));

        let fired = Arc::new(AtomicI32::new(0));
        {
            let handle_in_cb = handle.clone();
            let fired = fired.clone();
            handle.set_callback(move |_change| {
                fired.fetch_add(1, Ordering::SeqCst);
                // Re-arm with a no-op. Installing takes the manager lock.
                handle_in_cb.set_callback(|_| {});
            });
        }

        update_shared_event_status_with_policy(
            &mgr,
            ZenohEventType::RequestedQosIncompatible,
            1,
            42,
        );

        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the re-arming callback did not run"
        );
        let status = handle.take_event();
        assert_eq!(status.total_count, 1);
        assert_eq!(status.last_policy_kind, 42);
    });
}
