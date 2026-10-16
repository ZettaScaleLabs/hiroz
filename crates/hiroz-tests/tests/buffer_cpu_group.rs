//! A hiroz subscriber to a buffer message must join rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base key to a subscriber whose
//! liveliness token has no `backends:` field. These tests play the publisher with a raw Zenoh
//! session, so they need no ROS installation. They check the two things the publisher reads:
//! the token field, and the `<base>/_buf_cpu` subscription.

#![cfg(feature = "ros-msgs")]

mod common;

use std::time::{Duration, Instant};

use common::*;
use hiroz::{Builder, MessageTypeInfo, msg::ZMessage};
use hiroz_msgs::{
    sensor_msgs::PointCloud2,
    std_msgs::{Float64MultiArray, UInt8MultiArray},
};
use zenoh::Wait;

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);

fn raw_session(endpoint: &str) -> zenoh::Session {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    zenoh::open(config).wait().expect("raw session")
}

/// Every liveliness token currently declared. An error reply fails the test.
fn tokens(session: &zenoh::Session) -> Vec<String> {
    let replies = session
        .liveliness()
        .get("@ros2_lv/**")
        .timeout(Duration::from_secs(2))
        .wait()
        .expect("liveliness get");
    let mut keys = Vec::new();
    while let Ok(reply) = replies.recv() {
        match reply.result() {
            Ok(sample) => keys.push(sample.key_expr().to_string()),
            Err(e) => panic!("liveliness query returned an error: {e:?}"),
        }
    }
    keys
}

/// The one token for `topic`, once it is visible. A token that never appears fails the test.
fn token_for(session: &zenoh::Session, topic: &str) -> String {
    let needle = format!("/%{topic}/");
    let deadline = Instant::now() + RECV;
    loop {
        let all = tokens(session);
        let hits: Vec<&String> = all.iter().filter(|k| k.contains(&needle)).collect();
        match hits.len() {
            1 => return hits[0].clone(),
            0 if Instant::now() < deadline => std::thread::sleep(POLL),
            n => panic!("{n} tokens for {topic} in {all:?}"),
        }
    }
}

/// `<domain>/<topic>/<type>/<hash>`, with the domain read from a token of the same context.
fn base_key<T: MessageTypeInfo>(token: &str, topic: &str) -> String {
    let domain = token.split('/').nth(1).expect("a token has a domain");
    format!(
        "{domain}/{topic}/{}/{}",
        T::type_name(),
        T::type_hash().to_rihs_string()
    )
}

/// Put `payload` on `key` until `done` is true or `limit` passes.
fn put_until(
    session: &zenoh::Session,
    key: &str,
    payload: &[u8],
    limit: Duration,
    mut done: impl FnMut() -> bool,
) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        session.put(key, payload.to_vec()).wait().expect("put");
        if done() {
            return true;
        }
    }
    false
}

/// Pins: the token field follows the join decision for typed and dynamic subscribers.
#[test]
fn token_announces_cpu_backend_where_the_subscriber_joins() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");

    let _joined = node
        .create_sub::<UInt8MultiArray>("/tok_joined")
        .build()
        .unwrap();
    let _plain = node
        .create_sub::<Float64MultiArray>("/tok_plain")
        .build()
        .unwrap();
    let _opted_out = node
        .create_sub::<UInt8MultiArray>("/tok_out")
        .with_cpu_buffer_group(false)
        .build()
        .unwrap();
    let _forced = node
        .create_sub::<Float64MultiArray>("/tok_forced")
        .with_cpu_buffer_group(true)
        .build()
        .unwrap();
    let _dynamic = node
        .create_dyn_sub("/tok_dyn", PointCloud2::message_schema().unwrap())
        .build()
        .unwrap();

    for (topic, joins) in [
        ("tok_joined", true),
        ("tok_forced", true),
        ("tok_dyn", true),
        ("tok_plain", false),
        ("tok_out", false),
    ] {
        let token = token_for(&raw, topic);
        assert_eq!(token.ends_with("/backends:cpu:"), joins, "{topic}: {token}");
        assert!(joins || !token.contains("backends:"), "{topic}: {token}");
    }
}

/// Pins: a cache joins the group like a subscriber, announcing it and reading its key.
#[test]
fn cache_joins_the_group() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cache_node").build().expect("node");
    let cache = node
        .create_cache::<UInt8MultiArray>("/cache_topic", 10)
        .build()
        .unwrap();

    let token = token_for(&raw, "cache_topic");
    assert!(token.ends_with("/backends:cpu:"), "{token}");
    let key = format!(
        "{}/_buf_cpu",
        base_key::<UInt8MultiArray>(&token, "cache_topic")
    );
    let payload = UInt8MultiArray::default().serialize();
    assert!(
        put_until(&raw, &key, &payload, RECV, || !cache.is_empty()),
        "cache received nothing on {key}"
    );
}

/// Pins: a joined subscriber reads the group key, and `Locality::Remote` holds on that key too.
#[test]
fn joined_subscriber_reads_the_group_key_with_its_locality() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("loc_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/loc_topic")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()
        .unwrap();

    let key = format!(
        "{}/_buf_cpu",
        base_key::<UInt8MultiArray>(&token_for(&raw, "loc_topic"), "loc_topic")
    );
    let payload = UInt8MultiArray::default().serialize();
    let mut received = || sub.recv_timeout(POLL).is_ok();

    // Positive control: a sample from another session arrives.
    assert!(
        put_until(&raw, &key, &payload, RECV, &mut received),
        "control: nothing on {key}"
    );
    while received() {}
    // A sample from the subscriber's own session must not.
    assert!(
        !put_until(node.session(), &key, &payload, QUIET, &mut received),
        "a local sample reached a Remote-only subscriber"
    );
}
