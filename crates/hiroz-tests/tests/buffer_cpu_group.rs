//! A hiroz subscriber to a buffer message must join rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base key to a subscriber whose
//! liveliness token has no `backends:` field. These tests play the publisher with a raw Zenoh
//! session, so they need no ROS installation. They check the two things the publisher reads:
//! the token, and the `<base>/_buf_cpu` subscription.
//!
//! Expected values are literals from real `rmw_zenoh` output: the token layout and the message
//! hashes come from tokens a Lyrical publisher and subscriber wrote, the QoS chunks from the
//! oracle fixtures in `hiroz-protocol/tests/fixtures/rmw_zenoh_qos/`. Only the three opaque ids
//! of a token (zid, node id, entity id) are read from the observed token.

#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use common::*;
use hiroz::{
    BufferGroup, Builder, MessageTypeInfo,
    context::ZContextBuilder,
    dynamic::{FieldType, MessageSchema},
    qos::{QosDurability, QosHistory, QosProfile},
};
use hiroz_msgs::std_msgs::{Float64MultiArray, UInt8MultiArray};
use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter, RmwZenohFormatter};
use serde::{Deserialize, Serialize};
use zenoh::Wait;

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);

/// A message type as a token and a key write it.
struct Wire {
    name: &'static str,
    hash: &'static str,
}

const UINT8: Wire = Wire {
    name: "std_msgs::msg::dds_::UInt8MultiArray_",
    hash: "RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385",
};
const FLOAT64: Wire = Wire {
    name: "std_msgs::msg::dds_::Float64MultiArray_",
    hash: "RIHS01_1025ddc6b9552d191f89ef1a8d2f60f3d373e28b283d8891ddcc974e8c55397f",
};

/// `std_msgs/UInt8MultiArray` on the wire, without a message schema: the no-schema case.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Dim {
    label: String,
    size: u32,
    stride: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Layout {
    dim: Vec<Dim>,
    data_offset: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Blob {
    layout: Layout,
    data: Vec<u8>,
}
impl MessageTypeInfo for Blob {
    fn type_name() -> &'static str {
        UINT8.name
    }
    fn type_hash() -> hiroz::TypeHash {
        hiroz::TypeHash::from_rihs_string(UINT8.hash).unwrap()
    }
}
impl hiroz::ros_msg::WithTypeInfo for Blob {}
impl hiroz::msg::ZMessage for Blob {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Blob>;
}

/// The CDR bytes of a `UInt8MultiArray` with the data `[1, 2, 3]`: header, empty layout, data.
const PAYLOAD: [u8; 19] = [
    0, 1, 0, 0, // little-endian CDR header
    0, 0, 0, 0, // layout.dim: no entries
    0, 0, 0, 0, // layout.data_offset
    3, 0, 0, 0, // data length
    1, 2, 3,
];

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

/// What a token looks like. `hash` is `None` where hiroz derives it from an ad-hoc schema, so
/// the test has no independent value for it; the observed hash is then used as it is.
struct Shape<'a> {
    node: &'a str,
    topic: &'a str,
    name: &'a str,
    hash: Option<&'a str>,
    qos: &'a str,
    joined: bool,
}

/// The whole token a subscriber of this shape must declare, per the layout of `rmw_zenoh`:
/// `@ros2_lv/<domain>/<zid>/<node id>/<entity id>/MS/<enclave>/<namespace>/<node>/<topic>/
/// <type>/<hash>/<qos>[/backends:cpu:]`. The domain is 0, the enclave and the namespace are `%`.
fn assert_token(token: &str, shape: Shape) {
    let parts: Vec<&str> = token.split('/').collect();
    assert!(parts.len() > 5, "short token {token}");
    let (zid, node_id, entity_id) = (parts[2], parts[3], parts[4]);
    let hash = shape.hash.unwrap_or_else(|| {
        parts
            .iter()
            .find(|p| p.starts_with("RIHS01_"))
            .copied()
            .unwrap_or_else(|| panic!("no type hash in {token}"))
    });
    let backends = if shape.joined { "/backends:cpu:" } else { "" };
    let expected = format!(
        "@ros2_lv/0/{zid}/{node_id}/{entity_id}/MS/%/%/{}/%{}/{}/{hash}/{}{backends}",
        shape.node, shape.topic, shape.name, shape.qos
    );
    assert_eq!(token, expected);
}

const DEFAULT_QOS: &str = "::,10:,:,:,,";

fn shape<'a>(node: &'a str, topic: &'a str, wire: &'a Wire, joined: bool) -> Shape<'a> {
    Shape {
        node,
        topic,
        name: wire.name,
        hash: Some(wire.hash),
        qos: DEFAULT_QOS,
        joined,
    }
}

/// The key a Lyrical publisher writes a buffer message to: domain, topic, type, hash, group.
fn group_key(topic: &str, wire: &Wire) -> String {
    format!("0/{topic}/{}/{}/_buf_cpu", wire.name, wire.hash)
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

/// A one-field message schema, to give a dynamic subscriber a definition to inspect.
fn schema(field_type: FieldType) -> Arc<MessageSchema> {
    MessageSchema::builder("pkg/msg/M")
        .field("f", field_type)
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------------------------
// Slice 2: the join matrix
// ---------------------------------------------------------------------------------------------

/// Cycle 2.1: with no schema and `Auto`, a typed subscriber joins. The schema twin joins too.
#[test]
fn typed_subscriber_without_schema_joins() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _unknown = node.create_sub::<Blob>("/no_schema").build().unwrap();
    let _known = node
        .create_sub::<UInt8MultiArray>("/with_schema")
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "no_schema"),
        shape("tok_node", "no_schema", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "with_schema"),
        shape("tok_node", "with_schema", &UINT8, true),
    );
}

/// Cycle 2.2: a joined token carries the QoS chunk `rmw_zenoh` writes for the profile.
#[test]
fn joined_subscriber_token_carries_the_writers_qos_chunk() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let cases = [
        ("qos_1", 1, "::,1:,:,:,,"),
        ("qos_5", 5, "::,5:,:,:,,"),
        ("qos_42", 42, "::,:,:,:,,"),
    ];
    let _subs: Vec<_> = cases
        .iter()
        .map(|(topic, depth, _)| {
            node.create_sub::<UInt8MultiArray>(&format!("/{topic}"))
                .with_qos(QosProfile {
                    history: QosHistory::KeepLast(std::num::NonZeroUsize::new(*depth).unwrap()),
                    ..Default::default()
                })
                .build()
                .unwrap()
        })
        .collect();
    for (topic, _, qos) in cases {
        let mut s = shape("tok_node", topic, &UINT8, true);
        s.qos = qos;
        assert_token(&token_for(&raw, topic), s);
    }
}

/// Cycle 2.5: a message with an unbounded `uint8[]` joins, as a typed message and as a schema.
#[test]
fn typed_subscriber_with_buffer_fields_joins() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _typed = node
        .create_sub::<UInt8MultiArray>("/typed_buf")
        .build()
        .unwrap();
    let _dynamic = node
        .create_dyn_sub("/dyn_buf", UInt8MultiArray::message_schema().unwrap())
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "typed_buf"),
        shape("tok_node", "typed_buf", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "dyn_buf"),
        shape("tok_node", "dyn_buf", &UINT8, true),
    );
}

/// Cycle 2.5: a dynamic subscriber joins for an unbounded `uint8[]`, also nested.
#[test]
fn dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let bytes = || FieldType::Sequence(Box::new(FieldType::Uint8));
    let msg = || Box::new(FieldType::Message(schema(bytes())));
    let cases = [
        ("dyn_bytes", bytes()),
        ("dyn_nested", *msg()),
        ("dyn_array", FieldType::Array(msg(), 2)),
        ("dyn_seq", FieldType::Sequence(msg())),
    ];
    let _subs: Vec<_> = cases
        .iter()
        .map(|(topic, ft)| {
            node.create_dyn_sub(&format!("/{topic}"), schema(ft.clone()))
                .build()
                .unwrap()
        })
        .collect();
    for (topic, _) in cases {
        let s = Shape {
            node: "tok_node",
            topic,
            name: "pkg::msg::dds_::M_",
            hash: None,
            qos: DEFAULT_QOS,
            joined: true,
        };
        assert_token(&token_for(&raw, topic), s);
    }
}

/// Cycle 2.6: a bounded sequence or a fixed array of `uint8` is no buffer. The joined twin shows
/// the same schema shape joins when the sequence is unbounded.
#[test]
fn dynamic_subscriber_with_bounded_or_fixed_bytes_does_not_join() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let u8s = || Box::new(FieldType::Uint8);
    let cases = [
        ("dyn_twin", FieldType::Sequence(u8s()), true),
        ("dyn_bounded", FieldType::BoundedSequence(u8s(), 4), false),
        ("dyn_fixed", FieldType::Array(u8s(), 4), false),
    ];
    let _subs: Vec<_> = cases
        .iter()
        .map(|(topic, ft, _)| {
            node.create_dyn_sub(&format!("/{topic}"), schema(ft.clone()))
                .build()
                .unwrap()
        })
        .collect();
    for (topic, _, joined) in cases {
        let s = Shape {
            node: "tok_node",
            topic,
            name: "pkg::msg::dds_::M_",
            hash: None,
            qos: DEFAULT_QOS,
            joined,
        };
        assert_token(&token_for(&raw, topic), s);
    }
}

/// Cycle 2.6: a message with no `uint8[]` stays out. The buffer type is the joined twin.
#[test]
fn typed_subscriber_without_byte_fields_does_not_join() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _plain = node
        .create_sub::<Float64MultiArray>("/plain")
        .build()
        .unwrap();
    let _twin = node.create_sub::<UInt8MultiArray>("/twin").build().unwrap();

    assert_token(
        &token_for(&raw, "plain"),
        shape("tok_node", "plain", &FLOAT64, false),
    );
    assert_token(
        &token_for(&raw, "twin"),
        shape("tok_node", "twin", &UINT8, true),
    );
}

/// Cycle 2.7: `Off` stays out of the group on a buffer topic. `Auto` on the same type joins.
#[test]
fn opt_out_announces_no_backends() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _off = node
        .create_sub::<UInt8MultiArray>("/off")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();
    let _auto = node.create_sub::<UInt8MultiArray>("/auto").build().unwrap();

    assert_token(
        &token_for(&raw, "off"),
        shape("tok_node", "off", &UINT8, false),
    );
    assert_token(
        &token_for(&raw, "auto"),
        shape("tok_node", "auto", &UINT8, true),
    );
}

/// Cycle 2.7: `Off` stays out for a subscriber with no schema too.
#[test]
fn opt_out_stays_out_without_a_schema() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _off = node
        .create_sub::<Blob>("/blob_off")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "blob_off"),
        shape("tok_node", "blob_off", &UINT8, false),
    );
}

/// `Cpu` joins a type that has no byte field. `Auto` on the same type stays out.
#[test]
fn cpu_joins_a_type_without_byte_fields() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _forced = node
        .create_sub::<Float64MultiArray>("/forced")
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .unwrap();
    let _auto = node
        .create_sub::<Float64MultiArray>("/unforced")
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "forced"),
        shape("tok_node", "forced", &FLOAT64, true),
    );
    assert_token(
        &token_for(&raw, "unforced"),
        shape("tok_node", "unforced", &FLOAT64, false),
    );
}

/// Cycle 2.8: `rmw_zenoh` serves transient-local history on the base key only, so a
/// transient-local subscriber stays out even when forced. The volatile twin joins.
#[test]
fn transient_local_stays_out_even_when_forced() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let qos = |durability| QosProfile {
        durability,
        ..Default::default()
    };
    let _transient = node
        .create_sub::<UInt8MultiArray>("/transient")
        .with_qos(qos(QosDurability::TransientLocal))
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .unwrap();
    let _volatile = node
        .create_sub::<UInt8MultiArray>("/volatile")
        .with_qos(qos(QosDurability::Volatile))
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .unwrap();

    // The writer omits a default-valued chunk field; the transient-local chunk is `:1:,10:,:,:,,`.
    let mut s = shape("tok_node", "transient", &UINT8, false);
    s.qos = ":1:,10:,:,:,,";
    assert_token(&token_for(&raw, "transient"), s);
    assert_token(
        &token_for(&raw, "volatile"),
        shape("tok_node", "volatile", &UINT8, true),
    );
}

/// Cycle 2.8: only the `rmw_zenoh` key format has the group. A custom format stays out even when
/// forced. The same node on the `rmw_zenoh` format joins.
#[test]
fn custom_key_format_stays_out_even_when_forced() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _rmw = node
        .create_sub::<UInt8MultiArray>("/format_rmw")
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .unwrap();
    let custom_ctx = ZContextBuilder::default()
        .keyexpr_format(KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<
            RmwZenohFormatter,
        >::new())))
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .expect("custom ctx");
    let custom_node = custom_ctx.create_node("tok_node2").build().unwrap();
    let _custom = custom_node
        .create_sub::<UInt8MultiArray>("/format_custom")
        .with_buffer_group(BufferGroup::Cpu)
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "format_rmw"),
        shape("tok_node", "format_rmw", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "format_custom"),
        shape("tok_node2", "format_custom", &UINT8, false),
    );
}

/// Cycle 2.9: a cache of a buffer type joins and receives on the group key. A cache of a type
/// with no byte field stays out.
#[test]
fn cache_joins_the_group() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let cache = node
        .create_cache::<UInt8MultiArray>("/cache_topic", 10)
        .build()
        .unwrap();

    let _plain = node
        .create_cache::<Float64MultiArray>("/cache_plain", 10)
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "cache_topic"),
        shape("tok_node", "cache_topic", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "cache_plain"),
        shape("tok_node", "cache_plain", &FLOAT64, false),
    );
    let key = group_key("cache_topic", &UINT8);
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || !cache.is_empty()),
        "cache received nothing on {key}"
    );
}

// ---------------------------------------------------------------------------------------------
// Slice 3: group keys, teardown and the subscription-before-token order
// ---------------------------------------------------------------------------------------------

/// Cycle 3.1: a joined subscriber receives on the key a real publisher writes, and a subscriber
/// with no schema receives on it too. The key is a literal, not derived from the token.
#[test]
fn joined_subscriber_receives_on_the_key_a_real_publisher_writes() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let typed = node
        .create_sub::<UInt8MultiArray>("/recv_typed")
        .build()
        .unwrap();
    let blob = node.create_sub::<Blob>("/recv_blob").build().unwrap();
    token_for(&raw, "recv_typed");
    token_for(&raw, "recv_blob");

    let mut got = None;
    assert!(
        put_until(
            &raw,
            &group_key("recv_typed", &UINT8),
            &PAYLOAD,
            RECV,
            || {
                got = typed.recv_timeout(POLL).ok();
                got.is_some()
            }
        ),
        "typed subscriber received nothing on the group key"
    );
    assert_eq!(got.unwrap().data, hiroz::ZBuf::from(vec![1u8, 2, 3]));
    let mut got = None;
    assert!(
        put_until(
            &raw,
            &group_key("recv_blob", &UINT8),
            &PAYLOAD,
            RECV,
            || {
                got = blob.recv_timeout(POLL).ok();
                got.is_some()
            }
        ),
        "no-schema subscriber received nothing on the group key"
    );
    assert_eq!(got.unwrap().data, vec![1, 2, 3]);
}

/// Cycle 3.2: dropping a joined subscriber removes its token. The token is present before.
#[test]
fn dropping_the_subscriber_removes_its_token() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/drop_topic")
        .build()
        .unwrap();
    assert_token(
        &token_for(&raw, "drop_topic"),
        shape("tok_node", "drop_topic", &UINT8, true),
    );

    drop(sub);
    let deadline = Instant::now() + RECV;
    let mut left = tokens(&raw)
        .iter()
        .filter(|k| k.contains("/%drop_topic/"))
        .count();
    while left > 0 && Instant::now() < deadline {
        std::thread::sleep(POLL);
        left = tokens(&raw)
            .iter()
            .filter(|k| k.contains("/%drop_topic/"))
            .count();
    }
    assert_eq!(left, 0, "the token outlived the subscriber");
}

/// Cycle 3.3: a joined subscriber reads the group key, and `Locality::Remote` holds on that key.
#[test]
fn joined_subscriber_reads_the_group_key_with_its_locality() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/loc_topic")
        .with_locality(zenoh::sample::Locality::Remote)
        .build()
        .unwrap();
    token_for(&raw, "loc_topic");

    let key = group_key("loc_topic", &UINT8);
    let mut received = || sub.recv_timeout(POLL).is_ok();
    // Positive control: a sample from another session arrives.
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, &mut received),
        "control: nothing on {key}"
    );
    while received() {}
    // A sample from the subscriber's own session must not.
    assert!(
        !put_until(node.session(), &key, &PAYLOAD, QUIET, &mut received),
        "a local sample reached a Remote-only subscriber"
    );
}

/// Cycle 3.4: when the token becomes visible, the group subscription is already declared. A raw
/// session sees two events for each subscriber: the token, and a subscriber matching a publisher
/// it declared on the group key. The match must come first in nearly every trial. The session
/// stamps both events with one counter, so the test needs no clock.
#[test]
fn group_subscription_is_declared_before_the_token_is_visible() {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };

    const TRIALS: usize = 200;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");

    // Per topic: (stamp of the match, stamp of the token).
    type Stamps = Arc<Mutex<HashMap<String, (Option<usize>, Option<usize>)>>>;
    let stamps: Stamps = Default::default();
    let clock = Arc::new(AtomicUsize::new(0));
    let _watch = {
        let (stamps, clock) = (stamps.clone(), clock.clone());
        raw.liveliness()
            .declare_subscriber("@ros2_lv/**")
            .callback(move |sample| {
                let key = sample.key_expr().as_str();
                if sample.kind() != zenoh::sample::SampleKind::Put {
                    return;
                }
                if let Some(topic) = key.split('/').find_map(|c| c.strip_prefix("%order_")) {
                    let stamp = clock.fetch_add(1, Ordering::SeqCst);
                    let topic = format!("order_{topic}");
                    stamps.lock().unwrap().entry(topic).or_default().1 = Some(stamp);
                }
            })
            .wait()
            .unwrap()
    };

    let mut publishers = Vec::new();
    let mut wrong = Vec::new();
    for i in 0..TRIALS {
        let topic = format!("order_{i}");
        let publisher = raw
            .declare_publisher(group_key(&topic, &UINT8))
            .wait()
            .unwrap();
        let listener = {
            let (stamps, clock, topic) = (stamps.clone(), clock.clone(), topic.clone());
            publisher
                .matching_listener()
                .callback(move |status| {
                    if status.matching() {
                        let stamp = clock.fetch_add(1, Ordering::SeqCst);
                        stamps.lock().unwrap().entry(topic.clone()).or_default().0 = Some(stamp);
                    }
                })
                .wait()
                .unwrap()
        };
        let _sub = node
            .create_sub::<UInt8MultiArray>(&format!("/{topic}"))
            .build()
            .unwrap();
        let end = Instant::now() + Duration::from_secs(1);
        let done = loop {
            let seen = stamps.lock().unwrap().get(&topic).copied();
            match seen {
                Some((Some(matched), Some(token))) => break Some((matched, token)),
                _ if Instant::now() > end => break None,
                _ => std::thread::sleep(Duration::from_millis(2)),
            }
        };
        match done {
            Some((matched, token)) if matched < token => {}
            other => wrong.push((i, other)),
        }
        publishers.push((publisher, listener));
        if wrong.len() > TRIALS / 10 {
            break;
        }
    }
    // The two events reach the raw session on different paths, so a few trials arrive swapped
    // even in the right order (3 to 5 of 200 in the calibration runs). A token declared first
    // is swapped in every trial, so a tenth of the trials is a wide margin on both sides.
    assert!(
        wrong.len() <= TRIALS / 10,
        "token before match, or an event missing, in {} of {TRIALS} trials: {wrong:?}",
        wrong.len()
    );
}
