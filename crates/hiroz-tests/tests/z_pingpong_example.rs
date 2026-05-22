//! Tests for the z_pingpong example.
//!
//! The example file is included directly via `#[path]` so ping and pong
//! run in-process against a test router.

#![cfg(feature = "ros-msgs")]

mod common;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_pingpong.rs"]
mod z_pingpong;

use std::{thread, time::Duration};

use common::*;

#[test]
fn test_z_pingpong_roundtrip() {
    let router = TestRouter::new();

    // Run pong in a background thread (loops forever until dropped)
    let pong_endpoint = router.endpoint().to_string();
    let pong_handle = thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&pong_endpoint).expect("pong context failed");
        let _ = z_pingpong::run_pong(ctx);
    });

    // Give pong time to start
    thread::sleep(Duration::from_millis(500));

    // Run ping for a small number of samples
    let ping_ctx = create_hiroz_context_with_router(&router).expect("ping context failed");
    let args = z_pingpong::Args {
        mode: "ping".to_string(),
        payload: 64,
        frequency: 10,
        sample: 5,
        log: String::new(),
    };
    z_pingpong::run_ping(ping_ctx, &args).expect("ping failed");

    // Pong loops forever — drop the handle once ping has completed.
    drop(pong_handle);
}
