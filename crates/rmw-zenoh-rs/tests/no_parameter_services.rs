//! `NodeImpl::new` must not declare hiroz's own parameter services.
//!
//! rclcpp is the only real caller of rmw-zenoh-rs, and `rclcpp::Node`'s
//! constructor already runs its own generic parameter-service machinery over
//! `rmw_create_service` (the standard six services), unless the application
//! opts out. A second set from hiroz is not a fallback: two declarers of the
//! same six service names are both eligible to answer a real
//! `ros2 param get/set` call, which is a correctness concern, not only extra
//! router load.

use std::time::Duration;

use hiroz::{Builder, context::ZContextBuilder};
use rmw_zenoh_rs::node::NodeImpl;
use zenoh::Wait;

const DOMAIN_ID: usize = 0;
const NODE_NAME: &str = "no_parameter_services_probe";

fn free_tcp_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind free port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("tcp/127.0.0.1:{port}")
}

fn wait_for_tcp_accept(endpoint: &str) {
    let addr: std::net::SocketAddr = endpoint
        .trim_start_matches("tcp/")
        .parse()
        .expect("parse router endpoint");
    for _ in 0..40 {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("router at {endpoint} never accepted a connection");
}

#[test]
fn node_impl_declares_no_parameter_services() {
    let endpoint = free_tcp_endpoint();
    let router_config = hiroz::config::RouterConfigBuilder::new()
        .with_listen_endpoint(&endpoint)
        .build_config()
        .expect("build router config");
    let _router = zenoh::open(router_config).wait().expect("open router");
    wait_for_tcp_accept(&endpoint);

    let zcontext = ZContextBuilder::default()
        .with_domain_id(DOMAIN_ID)
        .with_router_endpoint(&endpoint)
        .expect("with_router_endpoint")
        .build()
        .expect("build zcontext");

    let node = NodeImpl::new(&zcontext, NODE_NAME, "").expect("NodeImpl::new");

    // Nothing to wait FOR here -- the assertion is an absence. Give the
    // node's own liveliness declarations a fixed settle window, then read
    // the final state, rather than polling for a count that (if the fix
    // regresses) would never arrive.
    std::thread::sleep(Duration::from_millis(1500));

    let snapshot = zcontext.graph().snapshot(DOMAIN_ID);
    let own_services: Vec<_> = snapshot
        .services
        .iter()
        .filter(|s| s.name.starts_with(&format!("/{NODE_NAME}/")))
        .map(|s| s.name.clone())
        .collect();

    assert!(
        own_services.is_empty(),
        "NodeImpl declared its own parameter services: {own_services:?}. \
         rclcpp already provides these; a second set is a Queryable conflict, \
         not a fallback."
    );

    drop(node);
}
