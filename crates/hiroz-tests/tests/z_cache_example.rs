//! Smoke tests for the `z_cache` examples.
//!
//! Each example file is included directly as a module via `#[path]` so the
//! async helper functions can be called in-process from `#[tokio::test]`
//! without spawning a subprocess.

#![cfg(feature = "ros-msgs")]

mod common;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_cache/talker.rs"]
mod z_cache_talker;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_cache/zenoh_stamp.rs"]
mod z_cache_zenoh_stamp;

#[allow(dead_code, unused_imports)]
#[path = "../../hiroz/examples/z_cache/app_stamp.rs"]
mod z_cache_app_stamp;

use common::*;
use hiroz::{Builder, context::ZContextBuilder};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_ctx(endpoint: &str) -> hiroz::context::ZContext {
    ZContextBuilder::default()
        .with_mode(String::from("client"))
        .with_connect_endpoints([endpoint])
        .build()
        .expect("context")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Cache role with ZenohStamp: subscribe, wait for connections, run one query
/// cycle, exit cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn z_cache_cache_role_zenoh_stamp() {
    let router = TestRouter::new();
    let ctx = make_ctx(&router.endpoint());
    z_cache_zenoh_stamp::run(ctx, "/smoke/cache_zenoh".into(), 20, 500, 1)
        .await
        .expect("run returned Err");
}

/// Cache role with ExtractorStamp: subscribe, run one query cycle, exit cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn z_cache_cache_role_app_stamp() {
    let router = TestRouter::new();
    let ctx = make_ctx(&router.endpoint());
    z_cache_app_stamp::run(ctx, "/smoke/cache_app".into(), 20, 1)
        .await
        .expect("run returned Err");
}

/// Talker role: publish 3 messages then exit cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn z_cache_talker_role() {
    let router = TestRouter::new();
    let ctx = make_ctx(&router.endpoint());
    z_cache_talker::run(ctx, "/smoke/talker".into(), 3)
        .await
        .expect("run returned Err");
}
