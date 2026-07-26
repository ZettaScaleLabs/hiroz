//! Tests for [`ZNode::discover_service_schema`], no-server path: with no node
//! serving the name, discovery must fail with [`DynamicError::SchemaNotFound`]
//! after the graph-poll timeout rather than hang or panic.
//!
//! The happy path needs a live server exposing `~get_type_description` and is
//! covered by the hiroz-union / hu-meter integration tests instead.

#![cfg(feature = "ros-msgs")]

mod common;

use std::time::{Duration, Instant};

use common::*;
use hiroz::Builder;
use hiroz::dynamic::DynamicError;

/// With no service server present, discovery polls the graph until the timeout
/// and then returns `SchemaNotFound` — it must not hang or panic.
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

    // It should actually wait out (roughly) the graph-poll timeout, not fail
    // instantly on the first miss.
    assert!(
        start.elapsed() >= timeout,
        "discovery returned before the graph-poll timeout elapsed"
    );
}
