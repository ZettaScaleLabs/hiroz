//! Tests for the z_custom_message example.
//!
//! The example file is included directly via `#[path]` so the server and
//! client functions run in-process against a test router.

#![cfg(feature = "ros-msgs")]

mod common;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_custom_message.rs"]
mod z_custom_message;

use std::{thread, time::Duration};

use common::*;

#[test]
fn test_z_custom_message_navigation() {
    let router = TestRouter::new();

    // Run nav-server in a background thread
    let server_endpoint = router.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&server_endpoint).expect("server context failed");
        z_custom_message::run_navigation_server(ctx)
    });

    // Give server time to start
    thread::sleep(Duration::from_millis(500));

    // Run client
    let client_ctx = create_hiroz_context_with_router(&router).expect("client context failed");
    z_custom_message::run_navigation_client(client_ctx, 3.0, 4.0, 1.5)
        .expect("navigation client failed");

    // Server loops forever — the test is done once the client succeeds.
    drop(server_handle);
}
