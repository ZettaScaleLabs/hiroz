//! A native wire fixture for the Go callback-server regression test.
//!
//! Build this target with `cargo test -p hiroz --features ffi --test
//! ffi_service_malformed_query --no-run`, then pass its executable path through
//! HIROZ_SERVICE_MALFORMED_FIXTURE when running the Go test suite. The Go test
//! supplies the endpoint and selector. No test-only native API is exported.
#![cfg(feature = "ffi")]

use std::time::Duration;
use zenoh::Wait;

#[test]
fn send_malformed_service_query() {
    let Ok(endpoint) = std::env::var("HIROZ_SERVICE_MALFORMED_ENDPOINT") else {
        return; // Invoked as a fixture only by the cross-language Go test.
    };
    let selector = std::env::var("HIROZ_SERVICE_MALFORMED_SELECTOR").unwrap();
    let mut config = zenoh::Config::default();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    config
        .insert_json5("scouting/gossip/enabled", "false")
        .unwrap();
    config.insert_json5("listen/endpoints", "[]").unwrap();
    config
        .insert_json5(
            "connect/endpoints",
            &serde_json::to_string(&[endpoint]).unwrap(),
        )
        .unwrap();
    let session = zenoh::open(config).wait().unwrap();
    eprintln!(
        "fixture peers={:?}, routers={:?}, selector={selector}",
        session.info().peers_zid().wait().collect::<Vec<_>>(),
        session.info().routers_zid().wait().collect::<Vec<_>>()
    );
    let queried = session
        .liveliness()
        .get("@ros2_lv/**")
        .timeout(Duration::from_secs(1))
        .wait()
        .unwrap();
    while let Ok(reply) = queried.recv() {
        eprintln!(
            "fixture liveliness={:?}",
            reply.result().map(|sample| sample.key_expr())
        );
    }
    let querier = session
        .declare_querier(selector)
        .target(zenoh::query::QueryTarget::All)
        .timeout(Duration::from_secs(5))
        .wait()
        .unwrap();
    let changes = querier.matching_listener().wait().unwrap();
    while !querier.matching_status().wait().unwrap().matching() {
        assert!(
            changes
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_some(),
            "service was not discovered"
        );
    }
    // A ROS service request requires a correlation attachment. Deliberately
    // omit it while sending a valid payload through an actual Zenoh query.
    let replies = querier.get().payload(vec![0, 1, 0, 0, 1]).wait().unwrap();
    match replies.recv_timeout(Duration::from_secs(3)) {
        Err(_) => {} // Query finalized without reply: malformed request consumed.
        Ok(Some(_)) => panic!("malformed request received a service reply"),
        Ok(None) => panic!("malformed request was not consumed before the query timeout"),
    }
}
