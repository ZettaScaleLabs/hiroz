//! Re-entrancy audit for graph-change and endpoint event callbacks.
//!
//! `GraphEventManager` invoked its registered callbacks *while holding the
//! registries they are registered in*: `trigger_event_with_policy` called the
//! callback under the `event_callbacks` guard, and `trigger_graph_change` held
//! both `event_callbacks` and `entity_topics` for the whole notification loop.
//!
//! Worse, the hot path into `trigger_graph_change` is the liveliness subscriber
//! installed by `Graph::new_with_pattern`, which used to hold the `GraphData`
//! mutex across the call. So on every liveliness token the callback ran with
//! three or four non-reentrant locks held — and these callbacks are user code:
//! the rmw layer hands them straight to an rclcpp executor. Any callback that
//! re-entered hiroz (counting publishers, unregistering an entity, registering a
//! new one) self-deadlocked on the thread that already held the guard.
//!
//! The fix is the same shape zenoh core itself uses in `resolve_put`: collect
//! the callbacks under the lock, drop every guard, then invoke. Callbacks are
//! `Arc` rather than `Box` so they can be cloned out cheaply.
//!
//! Every scenario runs on a dedicated thread behind a hard deadline, so a
//! re-entrancy deadlock fails the test instead of wedging the suite — the same
//! shape as `reentrant_publish.rs` and `reentrant_service.rs`.

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
    Builder, GidArray, TypeHash,
    entity::EndpointKind,
    event::{GraphEventManager, ZenohEventType},
    ros_msg::MessageTypeInfo,
};
use serde::{Deserialize, Serialize};
use serial_test::serial;

/// A self-contained message type, so this file does not depend on the
/// `ros-msgs` feature. Same shape as `reentrant_publish.rs`.
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

/// A fixed, valid `ZenohId`. `trigger_graph_change` does not read it.
fn test_zid() -> zenoh::session::ZenohId {
    use std::str::FromStr;
    zenoh::session::ZenohId::from_str("221b72df20924c15b8794c6bdb471150").expect("zid")
}

fn gid(n: u8) -> GidArray {
    let mut g = [0u8; 16];
    g[0] = n;
    g
}

// ---------------------------------------------------------------------------
// Registry-level scenarios (deterministic, no router needed)
// ---------------------------------------------------------------------------

/// An endpoint event callback that unregisters an entity.
///
/// `trigger_event_with_policy` used to hold `event_callbacks` across the call and
/// `unregister_entity` takes the same `std::sync::Mutex`, so this re-entered a
/// mutex the calling thread already held — a self-deadlock with no race needed.
///
/// Tearing down an endpoint from inside its own matched-event callback is exactly
/// what an rmw/rclcpp user does when a "publisher went away" event triggers
/// cleanup, so this is a reachable shape rather than a contrived one.
#[test]
fn event_callback_unregistering_does_not_deadlock() {
    with_deadline("event_callback_unregister", || {
        let mgr = Arc::new(GraphEventManager::new());
        let ran = Arc::new(AtomicBool::new(false));

        let ran_c = ran.clone();
        // Weak, so the callback (owned by the manager) does not keep it alive.
        let mgr_weak = Arc::downgrade(&mgr);
        mgr.register_event_callback(
            gid(1),
            "/reentrant".to_string(),
            ZenohEventType::PublicationMatched,
            move |_change| {
                ran_c.store(true, Ordering::SeqCst);
                if let Some(m) = mgr_weak.upgrade() {
                    // Re-entrant registry access from inside the callback.
                    m.unregister_entity(&gid(2));
                }
            },
        )
        .expect("register");

        mgr.trigger_event(&gid(1), ZenohEventType::PublicationMatched, 1);

        assert!(ran.load(Ordering::SeqCst), "the event callback never ran");
    });
}

/// A graph-change callback that registers a *new* entity.
///
/// `trigger_graph_change` used to hold both `event_callbacks` and `entity_topics`
/// for the whole notification loop; `register_event_callback` takes both. Same
/// self-deadlock, on the graph-change path rather than the endpoint-event path.
#[test]
fn graph_change_callback_registering_does_not_deadlock() {
    with_deadline("graph_change_register", || {
        use hiroz::entity::{EndpointEntity, Entity};

        let mgr = Arc::new(GraphEventManager::new());
        let ran = Arc::new(AtomicUsize::new(0));

        let ran_c = ran.clone();
        let mgr_weak = Arc::downgrade(&mgr);
        mgr.register_event_callback(
            gid(1),
            "/reentrant".to_string(),
            // A Publisher appearing notifies subscriptions.
            ZenohEventType::SubscriptionMatched,
            move |_change| {
                // Only re-enter once, or this registers forever.
                if ran_c.fetch_add(1, Ordering::SeqCst) == 0
                    && let Some(m) = mgr_weak.upgrade()
                {
                    // Re-entrant registration from inside the callback.
                    let _ = m.register_event_callback(
                        gid(9),
                        "/other".to_string(),
                        ZenohEventType::SubscriptionMatched,
                        |_| {},
                    );
                }
            },
        )
        .expect("register");

        let appearing = Entity::Endpoint(EndpointEntity {
            id: 1,
            node: None,
            kind: EndpointKind::Publisher,
            topic: "/reentrant".to_string(),
            type_info: None,
            qos: Default::default(),
        });
        mgr.trigger_graph_change(&appearing, true, test_zid());

        assert_eq!(
            ran.load(Ordering::SeqCst),
            1,
            "the graph-change callback never ran"
        );
    });
}

// ---------------------------------------------------------------------------
// End-to-end: the liveliness path that holds GraphData
// ---------------------------------------------------------------------------

/// A graph-change callback that queries the graph, driven by a *remote* entity.
///
/// This is the full hazard, not just the registry half. A remote entity arrives
/// on the liveliness subscriber declared in `Graph::new_with_pattern`, whose
/// callback held the `GraphData` mutex across `trigger_graph_change`. A
/// graph-change callback that asks the graph anything — `count`,
/// `get_topic_names_and_types`, `node_exists` — takes that same mutex on the same
/// thread and never returns.
///
/// Counting matched endpoints from inside a matched-event callback is the
/// canonical rmw use, so this is the shape that actually ships.
#[test]
#[serial]
fn graph_change_callback_querying_the_graph_does_not_deadlock() {
    with_deadline("graph_change_query_graph", || {
        const TOPIC: &str = "/reentrant_graph_event";

        let router = TestRouter::new();

        // Observer side: registers the graph-change callback.
        let ctx_a =
            create_hiroz_context_with_endpoint(router.endpoint()).expect("observer context");
        let node_a = ctx_a
            .create_node("graph_evt_observer")
            .build()
            .expect("node a");

        // A local publisher, only so we can read back the *qualified* topic name
        // the graph indexes entities under.
        let local_pub = node_a.create_pub::<Tick>(TOPIC).build().expect("local pub");
        let qualified_topic = local_pub.entity().topic.clone();

        let graph_a = node_a.graph().clone();
        let observed = Arc::new(AtomicUsize::new(0));
        let counted = Arc::new(AtomicUsize::new(0));

        let observed_c = observed.clone();
        let counted_c = counted.clone();
        // Weak: the callback is owned by the graph's event manager, which the
        // graph owns — an Arc here would be a cycle.
        let graph_weak = Arc::downgrade(&graph_a);
        graph_a
            .event_manager
            .register_event_callback(
                gid(7),
                qualified_topic.clone(),
                // A Publisher appearing notifies subscriptions.
                ZenohEventType::SubscriptionMatched,
                move |_change| {
                    observed_c.fetch_add(1, Ordering::SeqCst);
                    if let Some(g) = graph_weak.upgrade() {
                        // Re-entrant graph query from inside the callback: this
                        // takes the same `GraphData` mutex the liveliness
                        // callback holds.
                        let n = g.count(EndpointKind::Publisher, &qualified_topic);
                        counted_c.store(n, Ordering::SeqCst);
                    }
                },
            )
            .expect("register graph event callback");

        // Remote side: a second context whose publisher reaches the observer
        // through a liveliness token, i.e. through the subscriber callback that
        // used to hold `GraphData`.
        let ctx_b = create_hiroz_context_with_endpoint(router.endpoint()).expect("remote context");
        let node_b = ctx_b
            .create_node("graph_evt_remote")
            .build()
            .expect("node b");
        let _remote_pub = node_b
            .create_pub::<Tick>(TOPIC)
            .build()
            .expect("remote pub");

        // Wait for the liveliness token to propagate and the callback to run.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while observed.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }

        assert!(
            observed.load(Ordering::SeqCst) >= 1,
            "the graph-change callback never ran for the remote publisher — \
             the scenario proved nothing"
        );
        // Two, not one. `local_pub` is on this node for the whole scenario, so
        // `>= 1` was satisfiable without the remote publisher ever being in the
        // graph — which is precisely the regression this is meant to catch: a
        // callback invoked *before* the entity is inserted would still observe
        // the local publisher and pass. Requiring both makes the assertion
        // actually depend on the ordering it claims to verify.
        assert!(
            counted.load(Ordering::SeqCst) >= 2,
            "the re-entrant graph query saw {} publisher(s) on the topic; it must \
             see both the local one and the remote one whose appearance triggered \
             this callback. Seeing exactly 1 means the callback ran before the \
             remote entity was inserted into the graph",
            counted.load(Ordering::SeqCst)
        );
    });
}
