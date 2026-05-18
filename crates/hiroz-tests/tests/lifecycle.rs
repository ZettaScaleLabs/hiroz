//! Lifecycle node and publisher unit / integration tests.
//!
//! These tests exercise:
//! - State machine transitions (configure, activate, deactivate, cleanup, shutdown)
//! - Callback returns (Success, Failure, Error)
//! - Lifecycle publisher gating (publish() drops when deactivated, delivers when active)
//! - `create_publisher` mid-lifecycle registration
//!
//! The tests do **not** require a running Zenoh router; they use `ZContextBuilder`
//! in peer mode (no connect endpoints) so they can run offline.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use hiroz::{
    Builder,
    context::ZContextBuilder,
    lifecycle::{CallbackReturn, LifecycleState, ZLifecycleNode},
};
#[cfg(feature = "ros-msgs")]
use hiroz_msgs::std_msgs::String as RosString;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a standalone lifecycle node (no router needed for unit-level tests).
fn make_node(name: &str) -> ZLifecycleNode {
    ZContextBuilder::default()
        .disable_multicast_scouting()
        .build()
        .expect("context")
        .create_lifecycle_node(name)
        .build()
        .expect("lifecycle node")
}

// ---------------------------------------------------------------------------
// State machine transition tests
// ---------------------------------------------------------------------------

#[test]
fn test_initial_state_is_unconfigured() {
    let node = make_node("lc_initial");
    assert_eq!(node.get_current_state(), LifecycleState::Unconfigured);
}

#[test]
fn test_configure_transitions_to_inactive() {
    let mut node = make_node("lc_configure");
    let s = node.configure().expect("configure");
    assert_eq!(s, LifecycleState::Inactive);
    assert_eq!(node.get_current_state(), LifecycleState::Inactive);
}

#[test]
fn test_configure_activate_deactivate_cleanup() {
    let mut node = make_node("lc_full_cycle");

    assert_eq!(node.configure().unwrap(), LifecycleState::Inactive);
    assert_eq!(node.activate().unwrap(), LifecycleState::Active);
    assert_eq!(node.deactivate().unwrap(), LifecycleState::Inactive);
    assert_eq!(node.cleanup().unwrap(), LifecycleState::Unconfigured);
}

#[test]
fn test_shutdown_from_unconfigured() {
    let mut node = make_node("lc_shutdown_unconfigured");
    let s = node.shutdown().expect("shutdown");
    assert_eq!(s, LifecycleState::Finalized);
}

#[test]
fn test_shutdown_from_inactive() {
    let mut node = make_node("lc_shutdown_inactive");
    node.configure().unwrap();
    let s = node.shutdown().unwrap();
    assert_eq!(s, LifecycleState::Finalized);
}

#[test]
fn test_shutdown_from_active() {
    let mut node = make_node("lc_shutdown_active");
    node.configure().unwrap();
    node.activate().unwrap();
    let s = node.shutdown().unwrap();
    assert_eq!(s, LifecycleState::Finalized);
}

// ---------------------------------------------------------------------------
// Callback tests
// ---------------------------------------------------------------------------

#[test]
fn test_callbacks_invoked_on_transitions() {
    let mut node = make_node("lc_callbacks");
    let counter = Arc::new(AtomicU32::new(0));

    let c = counter.clone();
    node.on_configure = Box::new(move |_| {
        c.fetch_add(1, Ordering::Relaxed);
        CallbackReturn::Success
    });
    let c = counter.clone();
    node.on_activate = Box::new(move |_| {
        c.fetch_add(1, Ordering::Relaxed);
        CallbackReturn::Success
    });
    let c = counter.clone();
    node.on_deactivate = Box::new(move |_| {
        c.fetch_add(1, Ordering::Relaxed);
        CallbackReturn::Success
    });
    let c = counter.clone();
    node.on_cleanup = Box::new(move |_| {
        c.fetch_add(1, Ordering::Relaxed);
        CallbackReturn::Success
    });

    node.configure().unwrap();
    node.activate().unwrap();
    node.deactivate().unwrap();
    node.cleanup().unwrap();

    assert_eq!(counter.load(Ordering::Relaxed), 4);
}

#[test]
fn test_failure_callback_stays_in_current_state() {
    let mut node = make_node("lc_failure_cb");
    node.on_configure = Box::new(|_| CallbackReturn::Failure);

    // configure() returns the state after error processing (Unconfigured for Failure)
    let s = node.configure().unwrap();
    // Failure on configure → stays Unconfigured
    assert_eq!(s, LifecycleState::Unconfigured);
}

#[test]
fn test_error_callback_reaches_finalized_by_default() {
    let mut node = make_node("lc_error_cb");
    // Default on_error returns Failure (→ Finalized)
    node.on_configure = Box::new(|_| CallbackReturn::Error);

    let s = node.configure().unwrap();
    assert_eq!(s, LifecycleState::Finalized);
}

#[test]
fn test_error_callback_can_recover_to_unconfigured() {
    let mut node = make_node("lc_error_recover");
    node.on_configure = Box::new(|_| CallbackReturn::Error);
    node.on_error = Box::new(|_| CallbackReturn::Success); // Success → Unconfigured

    let s = node.configure().unwrap();
    assert_eq!(s, LifecycleState::Unconfigured);
}

// ---------------------------------------------------------------------------
// Publisher gating tests (mirrors test_lifecycle_publisher.cpp)
// ---------------------------------------------------------------------------

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publisher_deactivated_by_default() {
    let node = make_node("lc_pub_default");
    let pub_ = node
        .create_publisher::<RosString>("chatter")
        .expect("create_publisher");
    assert!(!pub_.is_activated());
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publisher_activated_after_node_activate() {
    let mut node = make_node("lc_pub_activate");
    let pub_ = node
        .create_publisher::<RosString>("chatter")
        .expect("create_publisher");

    node.configure().unwrap();
    assert!(!pub_.is_activated());

    node.activate().unwrap();
    assert!(pub_.is_activated());

    node.deactivate().unwrap();
    assert!(!pub_.is_activated());
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publish_does_not_error_when_deactivated() {
    let node = make_node("lc_pub_drop");
    let pub_ = node
        .create_publisher::<RosString>("chatter")
        .expect("create_publisher");

    // Publisher is deactivated — publish() must not error, it just drops the message.
    let msg = RosString {
        data: "hello".to_string(),
    };
    pub_.publish(&msg)
        .expect("publish when deactivated should be Ok");
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publish_succeeds_when_activated() {
    let mut node = make_node("lc_pub_active_publish");
    let pub_ = node
        .create_publisher::<RosString>("chatter")
        .expect("create_publisher");

    node.configure().unwrap();
    node.activate().unwrap();
    assert!(pub_.is_activated());

    let msg = RosString {
        data: "hello".to_string(),
    };
    pub_.publish(&msg).expect("publish when activated");
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publisher_registered_during_active_is_immediately_activated() {
    let mut node = make_node("lc_pub_late_register");
    node.configure().unwrap();
    node.activate().unwrap();

    // Publisher created after node is already active must be immediately activated.
    let pub_ = node
        .create_publisher::<RosString>("chatter_late")
        .expect("create_publisher");
    assert!(pub_.is_activated());
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_multiple_publishers_bulk_activated() {
    let mut node = make_node("lc_pub_bulk");
    let p1 = node.create_publisher::<RosString>("topic1").unwrap();
    let p2 = node.create_publisher::<RosString>("topic2").unwrap();
    let p3 = node.create_publisher::<RosString>("topic3").unwrap();

    node.configure().unwrap();
    node.activate().unwrap();

    assert!(p1.is_activated());
    assert!(p2.is_activated());
    assert!(p3.is_activated());

    node.deactivate().unwrap();

    assert!(!p1.is_activated());
    assert!(!p2.is_activated());
    assert!(!p3.is_activated());
}

// ---------------------------------------------------------------------------
// topic_name helper
// ---------------------------------------------------------------------------

#[cfg(feature = "ros-msgs")]
#[test]
fn test_publisher_topic_name() {
    let node = make_node("lc_pub_topic_name");
    let pub_ = node.create_publisher::<RosString>("my_topic").unwrap();
    // After qualification the topic should contain "my_topic"
    assert!(pub_.topic_name().contains("my_topic"));
}

// ---------------------------------------------------------------------------
// disable_communication_interface tests
// ---------------------------------------------------------------------------

#[test]
fn test_disable_communication_interface() {
    // Building a lifecycle node with services disabled should still work
    // for purely local usage (no service overhead).
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .build()
        .expect("context");
    let mut node = ctx
        .create_lifecycle_node("lc_no_comm")
        .disable_communication_interface()
        .build()
        .expect("lifecycle node");

    // State machine works fine without services
    assert_eq!(node.get_current_state(), LifecycleState::Unconfigured);
    node.configure().unwrap();
    assert_eq!(node.get_current_state(), LifecycleState::Inactive);
}

// ---------------------------------------------------------------------------
// ZLifecycleClient integration tests (require Zenoh router)
// ---------------------------------------------------------------------------

mod common;

use hiroz::lifecycle::ZLifecycleClient;

struct ClientTestEnv {
    _router: common::TestRouter,
    _node: hiroz::lifecycle::ZLifecycleNode,
    _ctx_node: hiroz::context::ZContext,
    _ctx_client: hiroz::context::ZContext,
    _mgr_node: hiroz::node::ZNode,
    client: ZLifecycleClient,
}

fn make_client_test_env(node_name: &str) -> ClientTestEnv {
    use std::{thread, time::Duration};

    let router = common::TestRouter::new();

    let ctx_node =
        common::create_hiroz_context_with_endpoint(router.endpoint()).expect("node context");
    let lc_node = ctx_node
        .create_lifecycle_node(node_name)
        .build()
        .expect("lifecycle node");

    thread::sleep(Duration::from_millis(500));

    let ctx_client =
        common::create_hiroz_context_with_endpoint(router.endpoint()).expect("client context");
    let mgr_node = ctx_client
        .create_node("lifecycle_manager")
        .build()
        .expect("manager node");
    let client = ZLifecycleClient::new(&mgr_node, node_name).expect("lifecycle client");

    thread::sleep(Duration::from_millis(500));

    ClientTestEnv {
        _router: router,
        _node: lc_node,
        _ctx_node: ctx_node,
        _ctx_client: ctx_client,
        _mgr_node: mgr_node,
        client,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_get_state() {
    let env = make_client_test_env("lc_client_get_state");
    let timeout = std::time::Duration::from_secs(5);

    let state = env.client.get_state(timeout).await.expect("get_state");
    assert_eq!(state, LifecycleState::Unconfigured);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_configure_activate_full_cycle() {
    let env = make_client_test_env("lc_client_full_cycle");
    let timeout = std::time::Duration::from_secs(5);

    // configure: Unconfigured → Inactive
    assert!(env.client.configure(timeout).await.expect("configure"));
    assert_eq!(
        env.client.get_state(timeout).await.expect("state"),
        LifecycleState::Inactive
    );

    // activate: Inactive → Active
    assert!(env.client.activate(timeout).await.expect("activate"));
    assert_eq!(
        env.client.get_state(timeout).await.expect("state"),
        LifecycleState::Active
    );

    // deactivate: Active → Inactive
    assert!(env.client.deactivate(timeout).await.expect("deactivate"));
    assert_eq!(
        env.client.get_state(timeout).await.expect("state"),
        LifecycleState::Inactive
    );

    // cleanup: Inactive → Unconfigured
    assert!(env.client.cleanup(timeout).await.expect("cleanup"));
    assert_eq!(
        env.client.get_state(timeout).await.expect("state"),
        LifecycleState::Unconfigured
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_shutdown() {
    let env = make_client_test_env("lc_client_shutdown");
    let timeout = std::time::Duration::from_secs(5);

    // shutdown from Unconfigured → Finalized
    assert!(env.client.shutdown(timeout).await.expect("shutdown"));
    assert_eq!(
        env.client.get_state(timeout).await.expect("state"),
        LifecycleState::Finalized
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_get_available_states() {
    let env = make_client_test_env("lc_client_avail_states");
    let timeout = std::time::Duration::from_secs(5);

    let states = env
        .client
        .get_available_states(timeout)
        .await
        .expect("get_available_states");
    // Should contain all 11 states (4 primary + 7 transition states)
    assert!(states.len() >= 4);
    let ids: Vec<u8> = states.iter().map(|s| s.id).collect();
    assert!(ids.contains(&1)); // Unconfigured
    assert!(ids.contains(&2)); // Inactive
    assert!(ids.contains(&3)); // Active
    assert!(ids.contains(&4)); // Finalized
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_get_available_transitions() {
    let env = make_client_test_env("lc_client_avail_trans");
    let timeout = std::time::Duration::from_secs(5);

    let transitions = env
        .client
        .get_available_transitions(timeout)
        .await
        .expect("get_available_transitions");
    // From Unconfigured: configure + shutdown = 2 transitions
    assert_eq!(transitions.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_client_invalid_transition_returns_false() {
    let env = make_client_test_env("lc_client_invalid");
    let timeout = std::time::Duration::from_secs(5);

    // Trying to activate from Unconfigured (should fail — need configure first)
    let result = env
        .client
        .trigger(hiroz::lifecycle::TransitionId::Activate, timeout)
        .await
        .expect("trigger");
    assert!(!result);
}

// ---------------------------------------------------------------------------
// Context-level namespace inheritance
// ---------------------------------------------------------------------------

#[test]
fn test_lifecycle_node_inherits_context_namespace() {
    let ctx = ZContextBuilder::default()
        .with_namespace("/my_ns")
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx
        .create_lifecycle_node("lc_ns_inherit")
        .build()
        .expect("lifecycle node");

    assert_eq!(node.inner.namespace(), "/my_ns");
}

#[test]
fn test_lifecycle_node_namespace_override_takes_precedence() {
    let ctx = ZContextBuilder::default()
        .with_namespace("/ctx_ns")
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx
        .create_lifecycle_node("lc_ns_override")
        .with_namespace("/override_ns")
        .build()
        .expect("lifecycle node");

    assert_eq!(node.inner.namespace(), "/override_ns");
}

#[test]
fn test_lifecycle_node_no_context_namespace_defaults_to_root() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx
        .create_lifecycle_node("lc_ns_root")
        .build()
        .expect("lifecycle node");

    assert_eq!(node.inner.namespace(), "");
}

// ---------------------------------------------------------------------------
// Context-level namespace inheritance — regular nodes
// ---------------------------------------------------------------------------

#[test]
fn test_node_inherits_context_namespace() {
    let ctx = ZContextBuilder::default()
        .with_namespace("/my_ns")
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx.create_node("ns_inherit").build().expect("node");

    assert_eq!(node.namespace(), "/my_ns");
}

#[test]
fn test_node_namespace_override_takes_precedence() {
    let ctx = ZContextBuilder::default()
        .with_namespace("/ctx_ns")
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx
        .create_node("ns_override")
        .with_namespace("/override_ns")
        .build()
        .expect("node");

    assert_eq!(node.namespace(), "/override_ns");
}

#[test]
fn test_node_no_context_namespace_defaults_to_root() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .build()
        .expect("context");

    let node = ctx.create_node("ns_root").build().expect("node");

    assert_eq!(node.namespace(), "");
}
