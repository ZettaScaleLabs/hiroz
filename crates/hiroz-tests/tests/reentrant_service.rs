//! Re-entrancy audit for the subsystems the pub/sub deadlock fix did *not* touch.
//!
//! The pub/sub bug was specific: zenoh-ext's `AdvancedSubscriber::sub_callback`
//! takes a non-reentrant `std::sync::Mutex` and invokes the user callback under
//! that guard, so a callback that published back into its own session re-entered
//! a mutex its own thread already held.
//!
//! Services, actions and parameters use plain zenoh (`declare_queryable`), and
//! zenoh core deliberately clones the queryable callbacks and `drop(state)`s
//! before invoking them (`zenoh/src/api/session.rs`, `handle_query`) — exactly as
//! it does for subscribers in `resolve_put`. That makes them *likely* safe, but
//! "likely, by analogy" is not evidence. These tests are the evidence.
//!
//! Every scenario runs on a dedicated thread behind a hard deadline, so a
//! re-entrancy deadlock fails the test instead of wedging the suite — the same
//! shape as `reentrant_publish.rs`.
//!
//! # What each test is evidence *of*
//!
//! Only one of the four detects the defect this change fixes. Measured by
//! reverting `parameter/service.rs` and re-running:
//!
//! * `parameter_on_set_callback_reregistering_does_not_deadlock` — **detector**.
//!   Fails on its deadline without the fix.
//! * `parameter_on_set_callback_setting_another_parameter_does_not_deadlock` —
//!   guard, not detector: recursive `read()` usually succeeds unless a writer is
//!   queued, so against unfixed source it is a coin flip. See its own note.
//! * the two service scenarios — the audit's **negative result**. They show
//!   services are clean; they pass with or without the fix, because the fix does
//!   not touch them. Their value is that "services came back clean" is a claim
//!   this file substantiates rather than asserts.
//!
//! Note on which API each test exercises. hiroz's *ergonomic* service server
//! (`create_service(..).build()`) is queue-mode: the query is pushed onto a
//! `BoundedQueue` and the user drains it with `take_request()` from their own
//! thread, so ordinary service handlers never run on a zenoh RX thread at all.
//! `build_with_callback` is the raw escape hatch that does put user code on the
//! RX thread (it is what the parameter service and the action server use
//! internally), so that is the path these tests target — it is the only place a
//! service-side re-entrancy deadlock could exist.

#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::{
    Builder,
    msg::{SerdeCdrSerdes, ZSerializer},
    parameter::{Parameter, ParameterValue, SetParametersResult},
};
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts};
use serial_test::serial;
use zenoh::{Wait, query::Query};

/// Budget for one scenario. Generous relative to the work done — anything slower
/// than this is a hang, not slowness.
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

/// Reply to a query, echoing the attachment so the hiroz client can match it.
fn reply_sum(query: &Query, sum: i64) {
    let bytes = SerdeCdrSerdes::<AddTwoIntsResponse>::serialize(&AddTwoIntsResponse { sum });
    let mut reply = query.reply(query.key_expr().clone(), bytes);
    if let Some(att) = query.attachment() {
        reply = reply.attachment(att.clone());
    }
    let _ = reply.wait();
}

/// A service handler that publishes on the same session.
///
/// The handler runs on the zenoh RX thread, inside the queryable callback. If
/// hiroz held any lock across `handler.handle(query)` — as the pub/sub path used
/// to — this publish would re-enter it.
#[test]
#[serial]
fn service_handler_publishing_does_not_deadlock() {
    with_deadline("service_handler_publishing", || {
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("ctx");
        let node = ctx.create_node("svc_pub").build().expect("node");

        let published = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(AtomicUsize::new(0));

        let sub_seen = seen.clone();
        let _sub = node
            .create_sub::<hiroz_msgs::std_msgs::String>("/svc_side_effect")
            .build_with_callback(move |_m| {
                sub_seen.fetch_add(1, Ordering::SeqCst);
            })
            .expect("sub");

        let side_pub = Arc::new(
            node.create_pub::<hiroz_msgs::std_msgs::String>("/svc_side_effect")
                .build()
                .expect("pub"),
        );

        let pub_for_handler = side_pub.clone();
        let published_c = published.clone();
        let _server = node
            .create_service::<AddTwoInts>("add_two_ints")
            .build_with_callback(move |query: Query| {
                // The re-entrant side effect: publish from inside the queryable
                // callback, on the same session.
                pub_for_handler
                    .publish(&hiroz_msgs::std_msgs::String {
                        data: "from-service-handler".into(),
                    })
                    .expect("publish from service handler");
                published_c.fetch_add(1, Ordering::SeqCst);
                reply_sum(&query, 42);
            })
            .expect("server");

        let client = node
            .create_client::<AddTwoInts>("add_two_ints")
            .build()
            .expect("client");

        thread::sleep(Duration::from_millis(1000));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt.block_on(async {
            client
                .call_with_timeout(&AddTwoIntsRequest { a: 1, b: 2 }, Duration::from_secs(10))
                .await
        });
        assert!(resp.is_ok(), "service call failed: {:?}", resp.err());

        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            published.load(Ordering::SeqCst),
            1,
            "handler did not complete its publish"
        );
        assert!(
            seen.load(Ordering::SeqCst) >= 1,
            "the publish issued from the service handler was never delivered"
        );
    });
}

/// A service handler that calls a *second* service on the same session.
///
/// The nested call is issued from the zenoh RX thread. This is the "service
/// handler calls another service" hazard: if the queryable dispatch path held a
/// lock, or if the inner query could only be answered by the very thread that is
/// blocked, this never returns.
#[test]
#[serial]
fn service_handler_calling_another_service_does_not_deadlock() {
    with_deadline("service_handler_nested_call", || {
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("ctx");
        let node = ctx.create_node("svc_nested").build().expect("node");

        let _inner = node
            .create_service::<AddTwoInts>("inner")
            .build_with_callback(move |query: Query| reply_sum(&query, 7))
            .expect("inner server");

        let inner_client = Arc::new(
            node.create_client::<AddTwoInts>("inner")
                .build()
                .expect("inner client"),
        );

        let nested_ok = Arc::new(AtomicBool::new(false));
        let nested_ok_c = nested_ok.clone();
        let inner_for_handler = inner_client.clone();

        let _outer = node
            .create_service::<AddTwoInts>("outer")
            .build_with_callback(move |query: Query| {
                // Nested service call from inside a queryable callback.
                //
                // The callback runs on one of zenoh's own tokio worker threads,
                // so `Runtime::block_on` here panics with "Cannot start a
                // runtime from within a runtime". The nested call therefore runs
                // on a plain std thread with its own runtime, and this callback
                // *joins* it — which is the hazard being tested: the zenoh RX
                // thread is blocked for the whole duration of the inner query.
                // If serving that inner query required this very thread, the
                // join never returns.
                let client = inner_for_handler.clone();
                let worker = thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async {
                        client
                            .call_with_timeout(
                                &AddTwoIntsRequest { a: 3, b: 4 },
                                Duration::from_secs(10),
                            )
                            .await
                    })
                });
                let inner = worker.join().expect("nested-call worker panicked");
                if inner.is_ok() {
                    nested_ok_c.store(true, Ordering::SeqCst);
                }
                reply_sum(&query, inner.map(|r| r.sum).unwrap_or(-1));
            })
            .expect("outer server");

        let outer_client = node
            .create_client::<AddTwoInts>("outer")
            .build()
            .expect("outer client");

        thread::sleep(Duration::from_millis(1000));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt.block_on(async {
            outer_client
                .call_with_timeout(&AddTwoIntsRequest { a: 1, b: 2 }, Duration::from_secs(20))
                .await
        });
        assert!(resp.is_ok(), "outer service call failed: {:?}", resp.err());
        assert!(
            nested_ok.load(Ordering::SeqCst),
            "the nested service call from inside the handler did not complete"
        );
        assert_eq!(resp.unwrap().sum, 7, "nested result not propagated");
    });
}

/// A parameter `on_set` callback that sets another parameter.
///
/// `ParameterState::validate_and_apply` invokes the user callback while holding
/// `on_set_callback.read()` (an `std::sync::RwLock` read guard). A callback that
/// calls `set_parameter` re-entered `validate_and_apply` on the same thread and
/// therefore took that same read lock recursively. Recursive read acquisition on
/// `std::sync::RwLock` is explicitly not guaranteed by the standard library — it
/// deadlocks if a writer is queued between the two acquisitions — so this was the
/// closest analogue to the pub/sub bug outside pub/sub.
///
/// Note what this test does and does not detect. Recursive `read()` on one
/// thread usually *succeeds* unless a writer is queued in between, and nothing
/// here queues one — so against unfixed source it is a coin flip, not a
/// detector. The re-registering case below is the deterministic one. This is a
/// regression test for the fixed behaviour, not proof the defect existed.
#[test]
#[serial]
fn parameter_on_set_callback_setting_another_parameter_does_not_deadlock() {
    with_deadline("parameter_on_set_reentrant", || {
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("ctx");
        let node = Arc::new(ctx.create_node("param_reentrant").build().expect("node"));

        node.declare_parameter("a", ParameterValue::Integer(0), Default::default())
            .expect("declare a");
        node.declare_parameter("b", ParameterValue::Integer(0), Default::default())
            .expect("declare b");

        let reentered = Arc::new(AtomicBool::new(false));
        let reentered_c = reentered.clone();
        // Weak, so the callback (owned by the node) does not keep the node alive.
        let node_weak = Arc::downgrade(&node);

        node.on_set_parameters(move |changed: &[Parameter]| {
            // Only re-enter for "a", or this recurses forever.
            if changed.iter().any(|p| p.name == "a")
                && !reentered_c.swap(true, Ordering::SeqCst)
                && let Some(n) = node_weak.upgrade()
            {
                // Re-entrant set from inside the on_set callback.
                let _ = n.set_parameter(Parameter::new("b", ParameterValue::Integer(99)));
            }
            SetParametersResult::success()
        });

        node.set_parameter(Parameter::new("a", ParameterValue::Integer(1)))
            .expect("set a");

        assert!(
            reentered.load(Ordering::SeqCst),
            "the on_set callback never ran"
        );
        assert_eq!(
            node.get_parameter("b"),
            Some(ParameterValue::Integer(99)),
            "the re-entrant set_parameter did not take effect"
        );
    });
}

/// A parameter `on_set` callback that replaces the callback registration.
///
/// This is the deterministic form of the same defect — stated in the past tense
/// because this branch is what removes it. `validate_and_apply` *used to* hold
/// `on_set_callback.read()` across the user callback, while `on_set_parameters`
/// takes `on_set_callback.write()`. A callback that re-registered therefore
/// asked the same thread for a write lock while it still held a read lock on
/// the same `std::sync::RwLock` — a guaranteed self-deadlock, no race required.
/// The fix clones the callback `Arc` out and drops the guard before invoking,
/// so no lock is held when the callback runs; this test holds that line.
///
/// "Swap out the validator once the node is configured" is an ordinary thing to
/// want, and rclcpp supports it (`remove_on_set_parameters_callback` /
/// `add_on_set_parameters_callback` are callable from within a callback), so
/// this is a reachable API shape rather than a contrived one.
#[test]
#[serial]
fn parameter_on_set_callback_reregistering_does_not_deadlock() {
    with_deadline("parameter_on_set_reregister", || {
        let router = TestRouter::new();
        let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("ctx");
        let node = Arc::new(ctx.create_node("param_rereg").build().expect("node"));

        node.declare_parameter("a", ParameterValue::Integer(0), Default::default())
            .expect("declare a");

        let ran = Arc::new(AtomicBool::new(false));
        let ran_c = ran.clone();
        let node_weak = Arc::downgrade(&node);

        node.on_set_parameters(move |_changed: &[Parameter]| {
            if !ran_c.swap(true, Ordering::SeqCst)
                && let Some(n) = node_weak.upgrade()
            {
                // Re-register from inside the callback. Pre-fix this asked
                // for a write lock while the same thread still held the read
                // lock; post-fix no guard is live here at all.
                n.on_set_parameters(|_| SetParametersResult::success());
            }
            SetParametersResult::success()
        });

        node.set_parameter(Parameter::new("a", ParameterValue::Integer(1)))
            .expect("set a");

        assert!(ran.load(Ordering::SeqCst), "the on_set callback never ran");
    });
}
