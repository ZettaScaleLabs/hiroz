//! Re-entrancy audit for endpoint event-status updates.
//!
//! `EventsManager` is shared as `Arc<Mutex<..>>`, so `&mut self` proves the
//! caller holds that mutex — and `update_event_status` fired the callback from
//! there. The callback is user code the rmw layer hands to an rclcpp executor,
//! and the first thing it usually does is ask the handle that fired for its
//! status (`rmw_take_event` → `RmEventHandle::take_event`), locking the same
//! mutex on the same thread. Non-reentrant, no race needed.
//!
//! Fix: record under the lock, drop the guard, invoke.
//! `record_event_status_with_policy` returns the callback rather than calling
//! it, and `update_shared_event_status[_with_policy]` is what holders use.
//!
//! Each scenario runs on its own thread behind a deadline, so a deadlock fails
//! the test instead of wedging the suite.
//!
//! # What these detect, and what they do not
//!
//! Both call `update_shared_event_status`, which this change *introduces* — so
//! a wholesale revert does not turn them red, it stops this file compiling.
//! They are not evidence the old shape deadlocked; `reentrant_graph_event.rs`
//! carries that.
//!
//! They are still detectors, for the property rather than the history:
//! reinstate the callout inside `update_shared_event_status_with_policy` and
//! both fail on their deadline. That is the regression worth guarding, since
//! the old entry point is gone.

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
