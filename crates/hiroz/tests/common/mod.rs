//! Shared integration-test helpers.
//!
//! `tests/*.rs` files are each compiled as a separate crate, so anything
//! reused across them belongs here (`mod common;` per Rust's own
//! `tests/common/mod.rs` convention -- naming it `common.rs` instead would
//! make cargo treat it as its own test binary). Add a helper here rather
//! than copying it into a new test file.

use std::time::{Duration, Instant};

use hiroz::{
    Builder,
    context::{ZContext, ZContextBuilder},
    node::ZNode,
};
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

/// A node on a peer session that has no router and no peer.
#[allow(dead_code)]
pub fn isolated_node(name: &str) -> (ZContext, ZNode) {
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx.create_node(name).build().expect("node");
    (ctx, node)
}

/// Two peer sessions on one machine: the second connects to the first.
#[allow(dead_code)]
pub fn connected_nodes(name: &str) -> (ZContext, ZNode, ZContext, ZNode) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port();
    let endpoint = format!("tcp/127.0.0.1:{port}");
    let ctx_a = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints([endpoint.clone()])
        .build()
        .expect("context a");
    let node_a = ctx_a
        .create_node(format!("{name}_a"))
        .build()
        .expect("node a");
    let ctx_b = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context b");
    let node_b = ctx_b
        .create_node(format!("{name}_b"))
        .build()
        .expect("node b");
    (ctx_a, node_a, ctx_b, node_b)
}

/// Poll `condition` until it holds. The test fails with `what` after ten seconds.
#[allow(dead_code)]
pub fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
