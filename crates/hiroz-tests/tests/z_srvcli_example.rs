//! Tests for the z_srvcli example.
//!
//! The example file is included directly via `#[path]` so the server and
//! client functions run in-process against a test router.

#![cfg(feature = "ros-msgs")]

mod common;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_srvcli.rs"]
mod z_srvcli;

use std::{thread, time::Duration};

use common::*;

#[test]
fn test_z_srvcli_add_two_ints() {
    let router = TestRouter::new();

    // Run server in a background thread
    let server_endpoint = router.endpoint().to_string();
    let server_handle = thread::spawn(move || {
        let ctx =
            create_hiroz_context_with_endpoint(&server_endpoint).expect("server context failed");
        z_srvcli::run_server(ctx)
    });

    // Give server time to start
    thread::sleep(Duration::from_millis(500));

    // Run client in the foreground
    let client_ctx = create_hiroz_context_with_router(&router).expect("client context failed");
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(z_srvcli::run_client(client_ctx, 3, 4))
        .expect("client failed");

    // Server loops forever — the test is done once the client succeeds.
    drop(server_handle);
}
