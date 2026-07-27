//! [`ZNode::discover_service_schema`], no-server path: with no node serving the
//! name, discovery must fail with [`DynamicError::SchemaNotFound`] after the
//! timeout, not hang or panic. The happy path needs a live server exposing
//! `~get_type_description` and is covered by the hu-meter integration tests.

#![cfg(feature = "ros-msgs")]

mod common;

use std::time::{Duration, Instant};

use common::*;
use hiroz::Builder;
use hiroz::dynamic::DynamicError;

/// With no service server present, discovery waits until the timeout and then
/// returns `SchemaNotFound` — it must not hang or panic.
#[tokio::test(flavor = "multi_thread")]
async fn discover_service_schema_times_out_without_server() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_endpoint(router.endpoint()).expect("ctx");
    let node = ctx.create_node("discovery_client").build().expect("node");

    let timeout = Duration::from_millis(300);
    let start = Instant::now();
    let result = node
        .discover_service_schema(
            "/no_such_service",
            "example_interfaces/msg/AddTwoIntsRequest",
            "example_interfaces/msg/AddTwoIntsResponse",
            timeout,
        )
        .await;

    // Must return SchemaNotFound rather than any other error / success.
    match result {
        Err(DynamicError::SchemaNotFound(msg)) => {
            assert!(
                msg.contains("no_such_service"),
                "error should name the missing service, got: {msg}"
            );
        }
        other => panic!("expected SchemaNotFound for absent server, got: {other:?}"),
    }

    // Waits out (roughly) the timeout — not an instant first-miss failure — and
    // returns promptly after the deadline (event-driven, no poll overshoot).
    let elapsed = start.elapsed();
    assert!(
        elapsed >= timeout,
        "discovery returned before the timeout elapsed: {elapsed:?}"
    );
    assert!(
        elapsed < timeout + Duration::from_millis(200),
        "discovery overshot the deadline by more than a small margin: {elapsed:?}"
    );
}
