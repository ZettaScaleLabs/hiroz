//! Structured subscriber-timeout tests for hiroz.
//!
//! Regression coverage for [hiroz#224](https://github.com/ZettaScaleLabs/hiroz/issues/224):
//! a subscriber `recv_timeout` that elapses must return a *structured*
//! [`hiroz::error::Error::Timeout`], detectable via [`hiroz::error::is_timeout`],
//! rather than a stringly-typed error that bindings have to sniff.

// Uses generated message types (`hiroz-msgs`), an optional dep linked only via
// the `ros-msgs` feature. The `-F rmw`-only clippy pass does not enable it, so
// gate the whole file like the sibling interop tests (e.g. cache.rs).
#![cfg(feature = "ros-msgs")]

mod common;

use std::time::Duration;

use common::{TestRouter, create_hiroz_context_with_endpoint};
use hiroz::Builder;
use hiroz_msgs::std_msgs;
use serial_test::serial;

/// A typed `recv_timeout` on an idle subscriber must surface a structured timeout.
#[test]
#[serial]
fn typed_recv_timeout_is_structured() {
    let router = TestRouter::new();
    let ctx =
        create_hiroz_context_with_endpoint(router.endpoint()).expect("Failed to create context");
    let node = ctx
        .create_node("timeout_sub")
        .build()
        .expect("Failed to create node");
    let sub = node
        .create_sub::<std_msgs::String>("/no_publisher_topic")
        .build()
        .expect("Failed to create subscriber");

    // No publisher exists, so this must time out.
    let err = sub
        .recv_timeout(Duration::from_millis(200))
        .expect_err("recv_timeout on an idle topic should fail");

    assert!(
        hiroz::error::is_timeout(&*err),
        "expected a structured timeout error, got: {err}"
    );
}

/// The raw-sample `recv_serialized_timeout` path must also be structured.
#[test]
#[serial]
fn serialized_recv_timeout_is_structured() {
    let router = TestRouter::new();
    let ctx =
        create_hiroz_context_with_endpoint(router.endpoint()).expect("Failed to create context");
    let node = ctx
        .create_node("timeout_sub_raw")
        .build()
        .expect("Failed to create node");
    let sub = node
        .create_sub::<std_msgs::String>("/no_publisher_topic_raw")
        .build()
        .expect("Failed to create subscriber");

    let err = sub
        .recv_serialized_timeout(Duration::from_millis(200))
        .expect_err("recv_serialized_timeout on an idle topic should fail");

    assert!(
        hiroz::error::is_timeout(&*err),
        "expected a structured timeout error, got: {err}"
    );
}
