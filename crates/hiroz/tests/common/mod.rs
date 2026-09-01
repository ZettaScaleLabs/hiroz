//! Shared integration-test helpers.
//!
//! `tests/*.rs` files are each compiled as a separate crate, so anything
//! reused across them belongs here (`mod common;` per Rust's own
//! `tests/common/mod.rs` convention -- naming it `common.rs` instead would
//! make cargo treat it as its own test binary). Add a helper here rather
//! than copying it into a new test file.

use std::time::Duration;

use zenoh::{Wait, config::WhatAmI};

/// A local zenoh router bound to an ephemeral port, for tests that need
/// real cross-session discovery rather than the default in-process/loopback
/// config.
pub struct TestRouter {
    pub endpoint: String,
    _session: zenoh::Session,
}

impl TestRouter {
    pub fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test port");
        let port = listener.local_addr().expect("test address").port();
        drop(listener);

        let endpoint = format!("tcp/127.0.0.1:{port}");
        let mut config = zenoh::Config::default();
        config.set_mode(Some(WhatAmI::Router)).unwrap();
        config
            .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
            .unwrap();
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        let session = zenoh::open(config).wait().expect("open test router");
        std::thread::sleep(Duration::from_millis(300));

        Self {
            endpoint,
            _session: session,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
