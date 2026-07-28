//! Re-entrancy audit for lifecycle transition callbacks.
//!
//! `ZLifecycleNode::trigger_transition` used to invoke the user's transition
//! callback from inside `self.state_machine.lock().unwrap().trigger(..)`, i.e.
//! with the state-machine mutex held for the whole duration of the callback.
//!
//! That mutex is not private to the transition: it is shared with the node's
//! own `~/get_state`, `~/change_state` and `~/get_available_transitions` service
//! handlers, and with `get_current_state()`. So while a transition callback ran,
//! the node could not answer any question about its own state. A callback that
//! asks — directly, or by waiting on anything that asks — waits forever.
//!
//! The fix splits `StateMachine::trigger` into `begin` (enter the intermediate
//! "busy" state) and `finish` (apply the callback's verdict), so the node drops
//! the guard while the callback runs. Observers keep seeing exactly what they
//! saw before: the intermediate state (`configuring`, `activating`, …).
//!
//! Every scenario runs on a dedicated thread behind a hard deadline, so a
//! re-entrancy deadlock fails the test instead of wedging the suite — the same
//! shape as `reentrant_publish.rs` and `reentrant_service.rs`.

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{
    Builder,
    lifecycle::{CallbackReturn, LifecycleState, ZLifecycleClient},
};
use serial_test::serial;

/// Budget for one scenario. Generous relative to the work done — anything slower
/// than this is a hang, not slowness.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(45);

/// How long the in-callback service call is allowed to take. Shorter than the
/// scenario budget so a blocked handler surfaces as a failed assertion rather
/// than as an unhelpful whole-scenario timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);

fn with_deadline(name: &'static str, scenario: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        scenario();
        let _ = tx.send(());
    });
    match rx.recv_timeout(SCENARIO_TIMEOUT) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{name}: scenario panicked — see the worker thread's panic above")
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{name}: scenario did not finish within {SCENARIO_TIMEOUT:?} — deadlock")
        }
    }
}

/// Query `~/get_state` from a thread with its own runtime and return the answer.
///
/// The transition callback runs on the caller's thread, which may already be
/// inside a runtime, so the query gets a plain std thread of its own — the same
/// pattern `reentrant_service.rs` uses for a nested service call.
fn get_state_blocking(client: Arc<ZLifecycleClient>) -> zenoh::Result<LifecycleState> {
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async { client.get_state(CALL_TIMEOUT).await })
    })
    .join()
    .expect("get_state worker panicked")
}

/// A transition callback that asks its own node what state it is in.
///
/// The `~/get_state` handler runs on a zenoh RX thread and takes the same
/// state-machine mutex the transition holds. Under the old code it blocks until
/// the transition completes — but the transition cannot complete, because its
/// callback is waiting for that very reply. Deadlock, broken only by the client
/// timeout, which then fails the assertion.
///
/// Introspecting your own lifecycle state during a transition is ordinary
/// (rclcpp exposes `get_current_state()` for exactly this), and a lifecycle
/// manager polling `~/get_state` while a node configures is even more ordinary.
#[test]
#[serial]
fn transition_callback_querying_own_state_does_not_deadlock() {
    with_deadline("lifecycle_transition_get_state", || {
        const NODE_NAME: &str = "lc_reentrant_get_state";

        let router = TestRouter::new();

        let ctx_node = create_hiroz_context_with_endpoint(router.endpoint()).expect("node ctx");
        let mut lc_node = ctx_node
            .create_lifecycle_node(NODE_NAME)
            .build()
            .expect("lifecycle node");

        // The querying side lives in its own context, as a real lifecycle
        // manager would.
        let ctx_client = create_hiroz_context_with_endpoint(router.endpoint()).expect("client ctx");
        let mgr_node = ctx_client
            .create_node("lc_manager")
            .build()
            .expect("mgr node");

        thread::sleep(Duration::from_millis(1000));
        let client =
            Arc::new(ZLifecycleClient::new(&mgr_node, NODE_NAME).expect("lifecycle client"));
        thread::sleep(Duration::from_millis(1000));

        let ran = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Option<zenoh::Result<LifecycleState>>>> = Arc::new(Mutex::new(None));

        let ran_c = ran.clone();
        let seen_c = seen.clone();
        let client_c = client.clone();
        lc_node.on_configure = Box::new(move |_prev| {
            ran_c.store(true, Ordering::SeqCst);
            // Re-entrant query of this node's own state, from inside its own
            // transition callback.
            *seen_c.lock().unwrap() = Some(get_state_blocking(client_c.clone()));
            CallbackReturn::Success
        });

        let final_state = lc_node.configure().expect("configure");

        assert!(ran.load(Ordering::SeqCst), "on_configure never ran");

        let seen = seen.lock().unwrap().take().expect("no state recorded");
        let seen = seen.expect(
            "~/get_state did not answer while the transition callback was running — \
             the state-machine mutex was held across the callback",
        );
        assert_eq!(
            seen,
            LifecycleState::Configuring,
            "the callback must observe the intermediate transition state"
        );
        assert_eq!(final_state, LifecycleState::Inactive);
    });
}

/// The same hazard on the failure path: a callback that returns `Failure` must
/// still have been able to introspect, and the node must still land on the
/// pre-transition state.
///
/// This pins the ordering the split introduced: `begin` publishes the
/// intermediate state, `finish` applies the verdict *after* the callback
/// returns. If the two halves were reordered, the observed state here would be
/// `Unconfigured` rather than `Activating`.
#[test]
#[serial]
fn failing_transition_callback_still_observes_intermediate_state() {
    with_deadline("lifecycle_failing_transition_get_state", || {
        const NODE_NAME: &str = "lc_reentrant_fail";

        let router = TestRouter::new();

        let ctx_node = create_hiroz_context_with_endpoint(router.endpoint()).expect("node ctx");
        let mut lc_node = ctx_node
            .create_lifecycle_node(NODE_NAME)
            .build()
            .expect("lifecycle node");

        let ctx_client = create_hiroz_context_with_endpoint(router.endpoint()).expect("client ctx");
        let mgr_node = ctx_client
            .create_node("lc_manager_fail")
            .build()
            .expect("mgr node");

        thread::sleep(Duration::from_millis(1000));
        let client =
            Arc::new(ZLifecycleClient::new(&mgr_node, NODE_NAME).expect("lifecycle client"));
        thread::sleep(Duration::from_millis(1000));

        // Get to Inactive first, so `activate` is a legal transition.
        assert_eq!(
            lc_node.configure().expect("configure"),
            LifecycleState::Inactive
        );

        let seen: Arc<Mutex<Option<zenoh::Result<LifecycleState>>>> = Arc::new(Mutex::new(None));
        let seen_c = seen.clone();
        let client_c = client.clone();
        lc_node.on_activate = Box::new(move |_prev| {
            *seen_c.lock().unwrap() = Some(get_state_blocking(client_c.clone()));
            CallbackReturn::Failure
        });

        let final_state = lc_node.activate().expect("activate");

        let seen = seen.lock().unwrap().take().expect("no state recorded");
        let seen = seen.expect(
            "~/get_state did not answer while the transition callback was running — \
             the state-machine mutex was held across the callback",
        );
        assert_eq!(
            seen,
            LifecycleState::Activating,
            "the callback must observe the intermediate transition state"
        );
        // Failure reverts to the start state.
        assert_eq!(final_state, LifecycleState::Inactive);
        assert_eq!(lc_node.get_current_state(), LifecycleState::Inactive);
    });
}

/// A transition callback that panics must not leave the node wedged.
///
/// This is the hazard the `begin`/`finish` split introduced. `begin` moves the
/// state machine into the intermediate ("busy") state and releases the guard;
/// the callback then runs unlocked. If it panics, the unwind used to escape
/// `trigger_transition` before `finish` ran — so `current` stayed at
/// `Configuring` forever. Worse, because the guard had already been dropped,
/// the mutex was *not* poisoned, so nothing ever reported it: every later
/// `begin` returned `None` and `~/get_state` answered `configuring` for the
/// life of the process.
///
/// That is strictly worse than the behaviour it replaced. While the callback
/// ran under the guard, a panic unwound through a live `MutexGuard` and
/// poisoned the mutex, so the very next `lock().unwrap()` panicked loudly.
/// Fail-fast became a silent permanent wedge.
///
/// The fix catches the unwind, settles the state machine on `Failure` (revert
/// to the start state — "the transition did not happen"), and re-raises the
/// payload, so the panic is still as loud as before while the node stays
/// usable.
///
/// Reverting the `catch_unwind` in `ZLifecycleNode::trigger_transition` makes
/// both assertions below fail: the state reads `Configuring`, and the recovery
/// transition returns `Configuring` because `begin` refuses to start.
#[test]
#[serial]
fn panicking_transition_callback_does_not_wedge_the_node() {
    with_deadline("lifecycle_transition_panic", || {
        const NODE_NAME: &str = "lc_panicking_callback";

        let router = TestRouter::new();
        let ctx_node = create_hiroz_context_with_endpoint(router.endpoint()).expect("node ctx");
        let mut lc_node = ctx_node
            .create_lifecycle_node(NODE_NAME)
            .build()
            .expect("lifecycle node");

        lc_node.on_configure = Box::new(|_prev| panic!("deliberate panic from on_configure"));

        // The panic must still propagate — silently swallowing it would be its
        // own defect. Keep the default hook quiet for this one call so the
        // expected backtrace does not look like a test failure.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lc_node.configure()));
        std::panic::set_hook(prev_hook);

        assert!(
            outcome.is_err(),
            "the panic was swallowed; it must still reach the caller"
        );

        // The node must be back at its pre-transition primary state, not
        // stranded in the intermediate one.
        assert_eq!(
            lc_node.get_current_state(),
            LifecycleState::Unconfigured,
            "node wedged in an intermediate state after a panicking callback"
        );

        // And it must still be usable: a subsequent transition has to work.
        lc_node.on_configure = Box::new(|_prev| CallbackReturn::Success);
        let recovered = lc_node.configure().expect("configure after panic");
        assert_eq!(
            recovered,
            LifecycleState::Inactive,
            "node could not transition after a panicking callback"
        );
    });
}
