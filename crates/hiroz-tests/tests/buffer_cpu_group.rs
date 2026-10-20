//! A hiroz subscriber to a buffer message must join rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh` publisher sends nothing on the base key to a subscriber whose
//! liveliness token has no `backends:` field. These tests play the publisher with a raw Zenoh
//! session, so they need no ROS installation. They check the two things the publisher reads:
//! the token, and the subscription.
//!
//! Expected values are literals from real `rmw_zenoh` output: the token layout and the message
//! hashes come from tokens a Lyrical publisher and subscriber wrote, the QoS chunks from the
//! recorded fixtures in `hiroz-protocol/tests/fixtures/rmw_zenoh_qos/`. Only the three opaque ids
//! of a token (zid, node id, entity id) are read from the observed token.

#![cfg(target_os = "linux")]
#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    io::Write,
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
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};
use zenoh::{Wait, sample::SampleKind};

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);

/// A message type as a token and a key write it.
struct Wire {
    name: &'static str,
    hash: &'static str,
}

impl Wire {
    /// The hash segment of a key or token in the key format of the distro this binary is built
    /// for: `hash` itself on Jazzy and later, `TypeHashNotSupported` on Humble.
    fn key_hash(&self) -> String {
        hiroz::TypeHash::from_rihs_string(self.hash)
            .expect("a valid RIHS string")
            .to_rihs_string()
    }
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
/// the test has no independent value for it; the observed hash segment is then used as it is.
/// `hash` is the segment as the distro writes it, see `Wire::key_hash`.
struct Shape<'a> {
    node: &'a str,
    topic: &'a str,
    name: &'a str,
    hash: Option<String>,
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
            .find(|p| p.starts_with("RIHS01_") || **p == "TypeHashNotSupported")
            .map(|p| (*p).to_owned())
            .unwrap_or_else(|| panic!("no type hash in {token}"))
    });
    let backends = if shape.joined { "/backends:cpu:" } else { "" };
    let expected = format!(
        "@ros2_lv/0/{zid}/{node_id}/{entity_id}/MS/%/%/{}/%{}/{}/{hash}/{}{backends}",
        shape.node, shape.topic, shape.name, shape.qos
    );
    // The test harness names each test thread after the test, so the message starts with it.
    let thread = std::thread::current();
    let name = thread
        .name()
        .unwrap_or("unnamed")
        .rsplit("::")
        .next()
        .unwrap_or("unnamed");
    assert!(
        token == expected,
        "{name}: token differs\n  got:  {token}\n  want: {expected}"
    );
}

const DEFAULT_QOS: &str = "::,10:,:,:,,";

fn shape<'a>(node: &'a str, topic: &'a str, wire: &'a Wire, joined: bool) -> Shape<'a> {
    Shape {
        node,
        topic,
        name: wire.name,
        hash: Some(wire.key_hash()),
        qos: DEFAULT_QOS,
        joined,
    }
}

/// The key a Lyrical publisher writes a buffer message to: domain, topic, type, hash, group.
fn group_key(topic: &str, wire: &Wire) -> String {
    format!("0/{topic}/{}/{}/_buf_cpu", wire.name, wire.key_hash())
}

fn base_key(topic: &str, wire: &Wire) -> String {
    format!("0/{topic}/{}/{}", wire.name, wire.key_hash())
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

/// Whether the one token for `topic` announces the group.
fn joined(session: &zenoh::Session, topic: &str) -> bool {
    token_for(session, topic).ends_with("/backends:cpu:")
}

// ---------------------------------------------------------------------------------------------
// The join matrix
// ---------------------------------------------------------------------------------------------

/// An action whose messages have no schema: every type is a hand-written serde type.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Num {
    value: u32,
}
impl MessageTypeInfo for Num {
    fn type_name() -> &'static str {
        "test_msgs::msg::dds_::Num_"
    }
    fn type_hash() -> hiroz::TypeHash {
        hiroz::TypeHash::zero()
    }
}
impl hiroz::ros_msg::WithTypeInfo for Num {}
impl hiroz::msg::ZMessage for Num {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Num>;
}

struct NoSchemaAction;
impl hiroz::action::ZAction for NoSchemaAction {
    type Goal = Num;
    type Result = Num;
    type Feedback = Num;
    fn name() -> &'static str {
        "test_msgs/action/NoSchema"
    }
}

/// With no schema and `Auto`, a typed subscriber joins. A typed subscriber with a schema joins
/// too, so a missing group token cannot come from a dead setup.
#[test]
fn typed_subscriber_without_schema_joins() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _unknown = node
        .create_sub::<Blob>("/no_schema")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _known = node
        .create_sub::<UInt8MultiArray>("/with_schema")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    assert_token(
        &token_for(&raw, "with_schema"),
        shape("tok_node", "with_schema", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "no_schema"),
        shape("tok_node", "no_schema", &UINT8, true),
    );
}

/// A subscriber that never calls `with_buffer_group` stays out of the group, for a typed
/// message, a message with no schema, a dynamic message, a cache and the feedback and status
/// subscribers of an action client. The same typed message on `Auto` joins, so a missing group
/// token comes from the default and not from a dead setup.
#[test]
fn default_subscribers_stay_out() {
    assert_eq!(
        BufferGroup::default(),
        BufferGroup::Off,
        "default_subscribers_stay_out: the default group"
    );
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _control = node
        .create_sub::<UInt8MultiArray>("/def_control")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _typed = node
        .create_sub::<UInt8MultiArray>("/def_typed")
        .build()
        .unwrap();
    let _blob = node.create_sub::<Blob>("/def_blob").build().unwrap();
    let _dynamic = node
        .create_dyn_sub("/def_dyn", UInt8MultiArray::message_schema().unwrap())
        .build()
        .unwrap();
    let _cache = node
        .create_cache::<UInt8MultiArray>("/def_cache", 10)
        .build()
        .unwrap();
    let _client = node
        .create_action_client::<NoSchemaAction>("def_action")
        .build()
        .unwrap();

    assert!(
        joined(&raw, "def_control"),
        "default_subscribers_stay_out: control: the Auto subscriber has no group token"
    );
    for topic in [
        "def_typed",
        "def_blob",
        "def_dyn",
        "def_cache",
        "def_action%_action%feedback",
        "def_action%_action%status",
    ] {
        assert!(
            !joined(&raw, topic),
            "default_subscribers_stay_out: a default subscriber joined on {topic}"
        );
    }
}

/// `Off` stays out for a subscriber with no schema. The `Auto` twin joins.
#[test]
fn opt_out_stays_out_without_a_schema() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _off = node
        .create_sub::<Blob>("/blob_opt_out")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();
    let _auto = node
        .create_sub::<Blob>("/blob_opt_in")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    assert!(
        joined(&raw, "blob_opt_in"),
        "opt_out_stays_out_without_a_schema: control: the Auto twin has no group token"
    );
    assert!(
        !joined(&raw, "blob_opt_out"),
        "opt_out_stays_out_without_a_schema: the Off subscriber has a group token"
    );
}

/// A message with no `uint8[]` stays out under `Auto`. The buffer type is the joined twin.
#[test]
fn typed_subscriber_without_byte_fields_does_not_join() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _plain = node
        .create_sub::<Float64MultiArray>("/plain")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _twin = node
        .create_sub::<UInt8MultiArray>("/twin")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    assert!(
        joined(&raw, "twin"),
        "typed_subscriber_without_byte_fields_does_not_join: control: the buffer twin has no group token"
    );
    assert!(
        !joined(&raw, "plain"),
        "typed_subscriber_without_byte_fields_does_not_join: a type with no byte field joined"
    );
}

/// A joined token carries the QoS chunk `rmw_zenoh` writes for the profile.
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
                .with_buffer_group(BufferGroup::Auto)
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

/// A message with an unbounded `uint8[]` joins, as a typed message and as a schema.
#[test]
fn typed_subscriber_with_buffer_fields_joins() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _typed = node
        .create_sub::<UInt8MultiArray>("/typed_buf")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _dynamic = node
        .create_dyn_sub("/dyn_buf", UInt8MultiArray::message_schema().unwrap())
        .with_buffer_group(BufferGroup::Auto)
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

/// A dynamic subscriber joins for an unbounded `uint8[]`, also nested.
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
                .with_buffer_group(BufferGroup::Auto)
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

/// A bounded sequence or a fixed array of `uint8` is no buffer. The unbounded twin shows the
/// same schema shape joins.
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
                .with_buffer_group(BufferGroup::Auto)
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

/// `Off` stays out of the group on a buffer topic, with or without a schema. `Auto` on the same
/// type joins.
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
    let _auto = node
        .create_sub::<UInt8MultiArray>("/auto")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _blob_off = node
        .create_sub::<Blob>("/blob_off")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "off"),
        shape("tok_node", "off", &UINT8, false),
    );
    assert_token(
        &token_for(&raw, "blob_off"),
        shape("tok_node", "blob_off", &UINT8, false),
    );
    assert_token(
        &token_for(&raw, "auto"),
        shape("tok_node", "auto", &UINT8, true),
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
        .with_buffer_group(BufferGroup::Auto)
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

/// `rmw_zenoh` serves transient-local history on the base key only, so a
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

/// Only the `rmw_zenoh` key format has the group. A custom format stays out even when
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

/// A cache of a buffer type joins and receives on the group key. A cache of a type
/// with no byte field stays out.
#[test]
fn cache_joins_the_group() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let cache = node
        .create_cache::<UInt8MultiArray>("/cache_topic", 10)
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();

    let _plain = node
        .create_cache::<Float64MultiArray>("/cache_plain", 10)
        .with_buffer_group(BufferGroup::Auto)
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
        "cache_joins_the_group: cache received nothing on {key}"
    );
}

/// `Off` on a cache keeps it out of the group and it reads the base key. A cache on `Auto` for
/// the same type joins, so a missing group token is the setting and not a dead setup.
#[test]
fn cache_with_group_off_stays_out() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let off = node
        .create_cache::<UInt8MultiArray>("/cache_off", 10)
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();
    let _auto = node
        .create_cache::<UInt8MultiArray>("/cache_auto", 10)
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();

    assert_token(
        &token_for(&raw, "cache_auto"),
        shape("tok_node", "cache_auto", &UINT8, true),
    );
    assert_token(
        &token_for(&raw, "cache_off"),
        shape("tok_node", "cache_off", &UINT8, false),
    );
    let key = base_key("cache_off", &UINT8);
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || !off.is_empty()),
        "cache_with_group_off_stays_out: the cache received nothing on {key}"
    );
}

// ---------------------------------------------------------------------------------------------
// Group keys and teardown
// ---------------------------------------------------------------------------------------------

/// A joined subscriber receives on the key a real publisher writes, and a subscriber
/// with no schema receives on it too. The key is a literal, not derived from the token.
#[test]
fn joined_subscriber_receives_on_the_key_a_real_publisher_writes() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let typed = node
        .create_sub::<UInt8MultiArray>("/recv_typed")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let blob = node
        .create_sub::<Blob>("/recv_blob")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
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

/// Dropping a joined subscriber removes its token. The token is present before.
#[test]
fn dropping_the_subscriber_removes_its_token() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/drop_topic")
        .with_buffer_group(BufferGroup::Auto)
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
    assert!(
        left == 0,
        "dropping_the_subscriber_removes_its_token: the token outlived the subscriber: {left} left"
    );
}

/// When a joined typed subscriber is dropped, its callback does not run again for a
/// sample on the group key. The callback runs on that key before the drop.
#[test]
fn dropping_a_joined_subscriber_stops_group_key_callbacks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const AFTER_DROP_PUTS: usize = 10;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = node
        .create_sub::<UInt8MultiArray>("/drop_cb_topic")
        .with_buffer_group(BufferGroup::Auto)
        .build_with_callback({
            let calls = calls.clone();
            move |_msg| {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        })
        .expect("sub");
    assert_token(
        &token_for(&raw, "drop_cb_topic"),
        shape("tok_node", "drop_cb_topic", &UINT8, true),
    );
    let key = group_key("drop_cb_topic", &UINT8);

    // Positive control: the callback runs on the group key before the drop.
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || calls.load(Ordering::SeqCst)
            > 0),
        "dropping_a_joined_subscriber_stops_group_key_callbacks: control: no callback ran on {key} before the drop"
    );
    std::thread::sleep(POLL);
    let before = calls.load(Ordering::SeqCst);
    assert!(
        before >= 1,
        "dropping_a_joined_subscriber_stops_group_key_callbacks: control: {before} callbacks before the drop"
    );

    drop(sub);
    for _ in 0..AFTER_DROP_PUTS {
        raw.put(&key, PAYLOAD.to_vec()).wait().expect("put");
    }
    std::thread::sleep(QUIET);
    let after = calls.load(Ordering::SeqCst);
    assert!(
        after == before,
        "dropping_a_joined_subscriber_stops_group_key_callbacks: {} callbacks ran on {key} after the drop ({AFTER_DROP_PUTS} puts, {before} before)",
        after - before
    );
}

/// When a joined cache is dropped, its group subscription stops. The stamp extractor
/// of the cache runs once per sample the subscription delivers, so it counts deliveries after the
/// cache itself is gone. The extractor runs on the group key before the drop.
#[test]
fn dropping_a_joined_cache_stops_group_key_updates() {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::SystemTime,
    };
    const AFTER_DROP_PUTS: usize = 10;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let stamped = Arc::new(AtomicUsize::new(0));
    let cache = node
        .create_cache::<UInt8MultiArray>("/drop_cache_topic", 10)
        .with_stamp({
            let stamped = stamped.clone();
            move |_msg: &UInt8MultiArray| {
                stamped.fetch_add(1, Ordering::SeqCst);
                SystemTime::now()
            }
        })
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .expect("cache");
    assert_token(
        &token_for(&raw, "drop_cache_topic"),
        shape("tok_node", "drop_cache_topic", &UINT8, true),
    );
    let key = group_key("drop_cache_topic", &UINT8);

    // Positive control: the cache takes samples from the group key before the drop.
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || stamped
            .load(Ordering::SeqCst)
            > 0),
        "dropping_a_joined_cache_stops_group_key_updates: control: no sample reached the cache on {key} before the drop"
    );
    std::thread::sleep(POLL);
    let before = stamped.load(Ordering::SeqCst);
    assert!(
        before >= 1 && !cache.is_empty(),
        "dropping_a_joined_cache_stops_group_key_updates: control: {before} samples stamped, cache empty: {}",
        cache.is_empty()
    );

    drop(cache);
    for _ in 0..AFTER_DROP_PUTS {
        raw.put(&key, PAYLOAD.to_vec()).wait().expect("put");
    }
    std::thread::sleep(QUIET);
    let after = stamped.load(Ordering::SeqCst);
    assert!(
        after == before,
        "dropping_a_joined_cache_stops_group_key_updates: {} samples reached the dropped cache on {key} ({AFTER_DROP_PUTS} puts, {before} before)",
        after - before
    );
}

/// A joined subscriber reads the group key, and `Locality::Remote` holds on that key.
#[test]
fn joined_subscriber_reads_the_group_key_with_its_locality() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/loc_topic")
        .with_locality(zenoh::sample::Locality::Remote)
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    token_for(&raw, "loc_topic");

    let key = group_key("loc_topic", &UINT8);
    let mut received = || sub.recv_timeout(POLL).is_ok();
    // Positive control: a sample from another session arrives.
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, &mut received),
        "joined_subscriber_reads_the_group_key_with_its_locality: control: nothing on {key}"
    );
    while received() {}
    // A sample from the subscriber's own session must not.
    assert!(
        !put_until(node.session(), &key, &PAYLOAD, QUIET, &mut received),
        "joined_subscriber_reads_the_group_key_with_its_locality: a local sample reached a Remote-only subscriber"
    );
}

/// A sample on the group key whose sequence length is over the decoder's ceiling reaches a joined
/// typed subscriber as a decode error, and the next valid sample still arrives. The first field is
/// a sequence whose length prefix is far above the ceiling, and no data follows. This covers that
/// one check. It does not cover other malformed shapes, such as a short buffer or a bad header.
#[test]
fn typed_subscriber_survives_a_length_over_the_ceiling_on_the_group_key() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let sub = node
        .create_sub::<UInt8MultiArray>("/hostile_group")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    token_for(&raw, "hostile_group");
    let key = group_key("hostile_group", &UINT8);
    let hostile = [&[0u8, 1, 0, 0][..], &u32::MAX.to_le_bytes()].concat();
    assert!(
        put_until(&raw, &key, &hostile, RECV, || matches!(
            sub.recv_timeout(POLL),
            Err(e) if e.to_string().contains("exceeds maximum")
        )),
        "typed_subscriber_survives_a_length_over_the_ceiling_on_the_group_key: no decode error for the hostile sample on {key}"
    );
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || sub
            .recv_timeout(POLL)
            .is_ok()),
        "typed_subscriber_survives_a_length_over_the_ceiling_on_the_group_key: no valid sample after the hostile one"
    );
}

/// When the token becomes visible, the group subscription is already declared. A raw peer
/// session, linked straight to the node, has declared a publisher on the group key for each trial.
/// The node declares its subscription and then its token on one link. So when the raw session
/// sees the token, its publisher must already match the subscription. The check runs inside the
/// callback that reports the token.
#[test]
fn group_subscription_is_declared_before_the_token_is_visible() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    const TRIALS: usize = 30;
    let endpoint = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
        format!("tcp/127.0.0.1:{}", listener.local_addr().unwrap().port())
    };
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"peer\"").unwrap();
    config
        .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    let raw = zenoh::open(config).wait().expect("raw peer session");
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_mode("peer")
        .build()
        .expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");

    // The raw session declares a matching listener on each key before the node declares anything.
    // The check below reads the matching state when the token callback runs.
    let publishers: Arc<Vec<_>> = Arc::new(
        (0..TRIALS)
            .map(|i| {
                let key = group_key(&format!("order_{i}"), &UINT8);
                raw.declare_publisher(key).wait().unwrap()
            })
            .collect(),
    );
    let _listeners: Vec<_> = publishers
        .iter()
        .map(|p| p.matching_listener().callback(|_| {}).wait().unwrap())
        .collect();
    std::thread::sleep(Duration::from_secs(1));

    // Per trial: 0 token not seen, 1 matched when the token was seen, 2 not matched.
    let seen: Arc<Vec<AtomicUsize>> = Arc::new((0..TRIALS).map(|_| AtomicUsize::new(0)).collect());
    let _watch = raw
        .liveliness()
        .declare_subscriber("@ros2_lv/**")
        .callback({
            let (publishers, seen) = (publishers.clone(), seen.clone());
            move |sample| {
                let trial = sample
                    .key_expr()
                    .as_str()
                    .split('/')
                    .find_map(|c| c.strip_prefix("%order_")?.parse::<usize>().ok());
                if let (SampleKind::Put, Some(i)) = (sample.kind(), trial) {
                    let matching = publishers[i].matching_status().wait().unwrap().matching();
                    seen[i].store(if matching { 1 } else { 2 }, SeqCst);
                }
            }
        })
        .wait()
        .unwrap();

    let mut wrong = Vec::new();
    for i in 0..TRIALS {
        let _sub = node
            .create_sub::<UInt8MultiArray>(&format!("/order_{i}"))
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap();
        let end = Instant::now() + RECV;
        while seen[i].load(SeqCst) == 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(2));
        }
        if seen[i].load(SeqCst) != 1 {
            wrong.push((i, seen[i].load(SeqCst)));
        }
        if !wrong.is_empty() {
            break;
        }
    }
    assert!(
        wrong.is_empty(),
        "group_subscription_is_declared_before_the_token_is_visible: no match at the token, or no token (trial, state: 0 no token, 2 no match): {wrong:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The info log and access control
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);
impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One `info` line per group key and process names the subscribed key and the group
/// key. Three subscribers on one key log once; a second key logs once; a subscriber that stays
/// out logs nothing.
#[test]
fn joining_logs_the_group_key_once_per_key() {
    let out = Captured::default();
    let _guard = tracing_subscriber::fmt()
        .with_writer({
            let out = out.clone();
            move || out.clone()
        })
        .with_ansi(false)
        .with_max_level(LevelFilter::INFO)
        .finish()
        .set_default();
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let _a: Vec<_> = (0..3)
        .map(|_| {
            node.create_sub::<UInt8MultiArray>("/log_a")
                .with_buffer_group(BufferGroup::Auto)
                .build()
                .unwrap()
        })
        .collect();
    let _b = node
        .create_sub::<Blob>("/log_b")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();
    let _out = node
        .create_sub::<UInt8MultiArray>("/log_off")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();
    let _plain = node
        .create_sub::<Float64MultiArray>("/log_plain")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .unwrap();

    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("CPU buffer group"))
        .collect();
    let expected = |topic: &str| {
        format!(
            "joined the rmw_zenoh CPU buffer group: subscribed to key {base}/**, which includes \
             key {group}. A router access-control rule that denies {base}/** or {group} drops \
             buffer messages and reports no error. Allow {base}/** wherever you allow {base}.",
            base = base_key(topic, &UINT8),
            group = group_key(topic, &UINT8)
        )
    };
    // Control: the joined subscribers exist, so a missing line is a missing log and not a missing
    // join. The subscriber that stays out declares no group token.
    let raw = raw_session(router.endpoint());
    assert!(
        token_for(&raw, "log_b").ends_with("/backends:cpu:"),
        "joining_logs_the_group_key_once_per_key: control: the joined subscriber has no group token"
    );
    assert!(
        !token_for(&raw, "log_off").contains("backends:"),
        "joining_logs_the_group_key_once_per_key: control: the opted-out subscriber has a group token"
    );
    assert!(
        lines.len() == 2,
        "joining_logs_the_group_key_once_per_key: want one line per joined key, got {}: {text}",
        lines.len()
    );
    assert!(
        lines.iter().any(|l| l.ends_with(&expected("log_a"))),
        "joining_logs_the_group_key_once_per_key: no line for log_a: {text}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with(&expected("log_b"))),
        "joining_logs_the_group_key_once_per_key: no line for log_b: {text}"
    );
    assert!(
        lines.iter().all(|l| l.contains(" INFO ")),
        "joining_logs_the_group_key_once_per_key: a line is not at info level: {text}"
    );
}

/// The process remembers a bounded number of group keys for the one-line log. A joined subscriber
/// on a key past that bound logs at `debug` and not at `info`, so a process that joins many
/// topics keeps no unbounded state.
#[test]
fn join_log_remembers_a_bounded_number_of_keys() {
    const NAME: &str = "join_log_remembers_a_bounded_number_of_keys";
    const CAP: usize = 1024;
    const TOPICS: usize = CAP + 16;
    let out = Captured::default();
    // Only the `hiroz` targets are captured at debug level: debug spans of the transport would
    // cross threads that carry no subscriber.
    let _guard = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer({
                    let out = out.clone();
                    move || out.clone()
                })
                .with_ansi(false)
                .with_filter(Targets::new().with_target("hiroz", LevelFilter::DEBUG)),
        )
        .set_default();
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cap_node").build().expect("node");
    let subs: Vec<_> = (0..TOPICS)
        .map(|i| {
            node.create_sub::<UInt8MultiArray>(&format!("/cap_{i}"))
                .with_buffer_group(BufferGroup::Auto)
                .build()
                .unwrap()
        })
        .collect();

    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("CPU buffer group"))
        .collect();
    let info = lines.iter().filter(|l| l.contains(" INFO ")).count();
    let debug = lines.iter().filter(|l| l.contains(" DEBUG ")).count();
    // Control: every subscriber joined and each join logged once at some level, so a short count
    // is a missing log and not a missing join.
    assert_eq!(subs.len(), TOPICS, "{NAME}: control: subscribers built");
    assert_eq!(
        info + debug,
        TOPICS,
        "{NAME}: control: want one line per joined key, got {info} info and {debug} debug"
    );
    assert!(
        info > 0,
        "{NAME}: control: no line at info level, the log never fired"
    );
    assert!(
        info <= CAP,
        "{NAME}: {info} keys logged at info level, the bound is {CAP}"
    );
}

/// A router that denies `key_expr` on every interface. A rule whose subject names only `lo`
/// would match nothing.
fn router_denying(key_expr: &str) -> TestRouter {
    let interfaces: Vec<String> = std::fs::read_dir("/sys/class/net")
        .expect("list interfaces")
        .map(|e| format!("\"{}\"", e.unwrap().file_name().to_string_lossy()))
        .collect();
    let access_control = format!(
        r#"{{
            enabled: true,
            default_permission: "allow",
            rules: [{{
                id: "deny_rule", permission: "deny",
                messages: ["put", "declare_subscriber"], flows: ["egress", "ingress"],
                key_exprs: ["{key_expr}"],
            }}],
            subjects: [{{ id: "all", interfaces: [{}] }}],
            policies: [{{ rules: ["deny_rule"], subjects: ["all"] }}],
        }}"#,
        interfaces.join(",")
    );
    TestRouter::with_config(move |config| {
        config
            .insert_json5("access_control", &access_control)
            .unwrap();
    })
}

fn router_denying_the_group_key() -> TestRouter {
    router_denying("0/*/*/*/_buf_cpu")
}

/// Under a rule that denies the group key, a joined
/// subscriber on a buffer topic gets nothing on that key, `build` returns `Ok`, and the base key
/// still flows. Without the rule the same subscriber receives on the group key.
#[test]
fn denied_group_key_starves_silently_while_the_base_key_flows() {
    let denying = router_denying_the_group_key();
    let open = TestRouter::new();
    let arrivals = |router: &TestRouter, topic: &str, key: &str| {
        let raw = raw_session(router.endpoint());
        let ctx = create_hiroz_context_with_router(router).expect("ctx");
        let node = ctx.create_node("tok_node").build().expect("node");
        let sub = node
            .create_sub::<UInt8MultiArray>(&format!("/{topic}"))
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .expect("build returns Ok under a deny rule");
        token_for(&raw, topic);
        std::thread::sleep(POLL);
        for _ in 0..20 {
            raw.put(key, PAYLOAD.to_vec()).wait().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut received = 0;
        while sub.recv_timeout(POLL).is_ok() {
            received += 1;
        }
        received
    };
    let group = group_key("acl_buf", &UINT8);
    let base = base_key("acl_buf", &UINT8);
    assert!(
        arrivals(&open, "acl_buf", &group) > 0,
        "control: open router, group key"
    );
    assert_eq!(
        arrivals(&denying, "acl_buf", &group),
        0,
        "group key under the rule"
    );
    assert!(
        arrivals(&denying, "acl_buf", &base) > 0,
        "base key under the rule"
    );
}

/// Under a rule that denies the group key, a subscriber on a topic with no buffer stays out of the
/// group and still receives on the base key. Control: on the same router, a joined subscriber on a
/// buffer topic receives nothing on its group key, so the rule is in force.
#[test]
fn non_buffer_topic_still_delivers_under_a_group_key_deny_rule() {
    let denying = router_denying_the_group_key();
    let raw = raw_session(denying.endpoint());
    let ctx = create_hiroz_context_with_router(&denying).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let twin = node
        .create_sub::<UInt8MultiArray>("/acl_plain_twin")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .expect("build returns Ok under a deny rule");
    assert!(
        joined(&raw, "acl_plain_twin"),
        "non_buffer_topic_still_delivers_under_a_group_key_deny_rule: control: the twin did not join the group"
    );
    let twin_key = group_key("acl_plain_twin", &UINT8);
    assert!(
        !put_until(&raw, &twin_key, &PAYLOAD, QUIET, || twin
            .recv_timeout(POLL)
            .is_ok()),
        "non_buffer_topic_still_delivers_under_a_group_key_deny_rule: control: the joined twin received on {twin_key}, so the rule does not bite"
    );
    // A group-key rule cannot touch this subscriber: it reads the base key only, so its delivery is
    // unaffected by construction. The joined twin above shows the rule does bite.
    let sub = node
        .create_sub::<Float64MultiArray>("/acl_plain")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .expect("build returns Ok under a deny rule");
    assert!(
        !joined(&raw, "acl_plain"),
        "non_buffer_topic_still_delivers_under_a_group_key_deny_rule: the subscriber joined the group"
    );
    let key = base_key("acl_plain", &FLOAT64);
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || sub
            .recv_serialized_timeout(POLL)
            .is_ok()),
        "non_buffer_topic_still_delivers_under_a_group_key_deny_rule: nothing arrived on {key}"
    );
}

/// Under a rule that denies `<topic key>/**` alone, a joined subscriber on a buffer topic gets
/// nothing, on the group key and on the base key. `<topic key>/**` includes `<topic key>`, so the
/// rule also starves a subscriber set to `Off`, which reads the base key only. A rule on the group
/// key alone leaves the base key open (see the test above).
///
/// Two controls show the zeros come from the rule. A router with no rule delivers on both keys,
/// to a joined and to an `Off` subscriber. On the router with the rule, a joined subscriber on a
/// second topic still receives on its group key, so the rule is scoped to one topic key.
#[test]
fn denied_wide_key_starves_a_joined_subscriber() {
    let base = base_key("acl_wide", &UINT8);
    let group = group_key("acl_wide", &UINT8);
    let denying = router_denying(&format!("{base}/**"));
    let open = TestRouter::new();
    let arrivals = |router: &TestRouter, topic: &str, mode: BufferGroup, key: &str| {
        let raw = raw_session(router.endpoint());
        let ctx = create_hiroz_context_with_router(router).expect("ctx");
        let node = ctx.create_node("tok_node").build().expect("node");
        let sub = node
            .create_sub::<UInt8MultiArray>(&format!("/{topic}"))
            .with_buffer_group(mode)
            .build()
            .expect("build returns Ok under a deny rule");
        token_for(&raw, topic);
        std::thread::sleep(POLL);
        for _ in 0..20 {
            raw.put(key, PAYLOAD.to_vec()).wait().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut received = 0;
        while sub.recv_timeout(POLL).is_ok() {
            received += 1;
        }
        received
    };
    let other = group_key("acl_wide_other", &UINT8);
    let check = |what: &str, received: usize, delivers: bool| {
        assert!(
            (received > 0) == delivers,
            "denied_wide_key_starves_a_joined_subscriber: {what}: received {received}, expected {}",
            if delivers { "some" } else { "none" }
        );
    };
    check(
        "open router, joined, group key",
        arrivals(&open, "acl_wide", BufferGroup::Auto, &group),
        true,
    );
    check(
        "open router, joined, base key",
        arrivals(&open, "acl_wide", BufferGroup::Auto, &base),
        true,
    );
    check(
        "open router, Off, base key",
        arrivals(&open, "acl_wide", BufferGroup::Off, &base),
        true,
    );
    check(
        "rule, joined, other topic, group key",
        arrivals(&denying, "acl_wide_other", BufferGroup::Auto, &other),
        true,
    );
    check(
        "rule, joined, group key",
        arrivals(&denying, "acl_wide", BufferGroup::Auto, &group),
        false,
    );
    check(
        "rule, joined, base key",
        arrivals(&denying, "acl_wide", BufferGroup::Auto, &base),
        false,
    );
    check(
        "rule, Off, base key",
        arrivals(&denying, "acl_wide", BufferGroup::Off, &base),
        false,
    );
}

/// A router with default-deny access control that allows only `key_exprs`, on every interface.
/// This is the allow-list shape: the operator names the topic key and nothing else.
fn router_allowing_only(key_exprs: &[String]) -> TestRouter {
    let interfaces: Vec<String> = std::fs::read_dir("/sys/class/net")
        .expect("list interfaces")
        .map(|e| format!("\"{}\"", e.unwrap().file_name().to_string_lossy()))
        .collect();
    let keys: Vec<String> = key_exprs.iter().map(|k| format!("\"{k}\"")).collect();
    let access_control = format!(
        r#"{{
            enabled: true,
            default_permission: "deny",
            rules: [{{
                id: "allow_rule", permission: "allow",
                messages: ["put", "declare_subscriber"], flows: ["egress", "ingress"],
                key_exprs: [{}],
            }}, {{
                id: "allow_tokens", permission: "allow",
                messages: ["liveliness_token", "liveliness_query", "declare_liveliness_subscriber"],
                flows: ["egress", "ingress"], key_exprs: ["@ros2_lv/**"],
            }}],
            subjects: [{{ id: "all", interfaces: [{}] }}],
            policies: [{{ rules: ["allow_rule", "allow_tokens"], subjects: ["all"] }}],
        }}"#,
        keys.join(","),
        interfaces.join(",")
    );
    TestRouter::with_config(move |config| {
        config
            .insert_json5("access_control", &access_control)
            .unwrap();
    })
}

/// A default-deny router that allows `<topic key>` and not `<topic key>/**` starves a joined
/// subscriber, because the joined subscriber declares `<topic key>/**`. The base key is starved
/// too. An `Off` subscriber declares `<topic key>` and keeps receiving.
///
/// Controls: an `Off` subscriber receives under the allow-only rule, so the puts reach the
/// router and the rule lets the base key through. A joined subscriber receives once the rule
/// also allows `<topic key>/**`, which is the remedy the docs give.
#[test]
fn allow_only_base_key_starves_a_joined_subscriber() {
    let base = base_key("acl_allow", &UINT8);
    let allow_base = router_allowing_only(std::slice::from_ref(&base));
    let allow_both = router_allowing_only(&[base.clone(), format!("{base}/**")]);
    let arrivals = |router: &TestRouter, mode: BufferGroup, key: &str| {
        let raw = raw_session(router.endpoint());
        let ctx = create_hiroz_context_with_router(router).expect("ctx");
        let node = ctx.create_node("tok_node").build().expect("node");
        let sub = node
            .create_sub::<UInt8MultiArray>("/acl_allow")
            .with_buffer_group(mode)
            .build()
            .expect("build returns Ok under a deny rule");
        // The token is declared after the subscription, so a visible token ends the wait for it.
        token_for(&raw, "acl_allow");
        for _ in 0..20 {
            raw.put(key, PAYLOAD.to_vec()).wait().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut received = 0;
        while sub.recv_timeout(POLL).is_ok() {
            received += 1;
        }
        received
    };
    let check = |what: &str, received: usize, delivers: bool| {
        assert!(
            (received > 0) == delivers,
            "allow_only_base_key_starves_a_joined_subscriber: {what}: received {received}, expected {}",
            if delivers { "some" } else { "none" }
        );
    };
    check(
        "control: allow base only, Off, base key",
        arrivals(&allow_base, BufferGroup::Off, &base),
        true,
    );
    check(
        "control: allow base and base/**, joined, base key",
        arrivals(&allow_both, BufferGroup::Auto, &base),
        true,
    );
    check(
        "allow base only, joined, base key",
        arrivals(&allow_base, BufferGroup::Auto, &base),
        false,
    );
}

// ---------------------------------------------------------------------------------------------
// The base subscription serves the group key
// ---------------------------------------------------------------------------------------------

/// The joined subscriber has one subscription, on `<base>/**`. It serves the base key and the
/// group key. Every other key under the base reaches that subscription, and the subscriber drops
/// it in process: another group of the same topic (`_buf_cuda`, say) never reaches the callback.
///
/// Control 1 shows the subscription is wide: a raw publisher on each other key sees a matching
/// subscriber. Without it, "nothing arrived" cannot tell a filter from a subscription that is too
/// narrow. Control 2 shows the group key and the base key still deliver.
#[test]
fn other_groups_of_the_topic_never_reach_a_joined_subscriber() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const PUTS: usize = 10;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let calls = Arc::new(AtomicUsize::new(0));
    let _sub = node
        .create_sub::<UInt8MultiArray>("/iso_groups")
        .with_buffer_group(BufferGroup::Auto)
        .build_with_callback({
            let calls = calls.clone();
            move |_msg| {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        })
        .expect("sub");
    token_for(&raw, "iso_groups");

    let base = base_key("iso_groups", &UINT8);
    let others = [
        format!("{base}/_buf_cuda"),
        format!("{base}/_buf_cpu/extra"),
        format!("{base}/deeper/_buf_cpu"),
    ];

    // Control 1: each other key reaches a subscription of the subscriber's session.
    for key in &others {
        let publisher = raw
            .declare_publisher(key.clone())
            .wait()
            .expect("publisher");
        let end = Instant::now() + RECV;
        while !publisher.matching_status().wait().unwrap().matching() && Instant::now() < end {
            std::thread::sleep(POLL);
        }
        assert!(
            publisher.matching_status().wait().unwrap().matching(),
            "other_groups_of_the_topic_never_reach_a_joined_subscriber: control: no subscription matches {key}, so the subscriber's key is not wide"
        );
        for _ in 0..PUTS {
            publisher.put(PAYLOAD.to_vec()).wait().expect("put");
        }
    }
    std::thread::sleep(QUIET);
    let leaked = calls.load(Ordering::SeqCst);
    assert!(
        leaked == 0,
        "other_groups_of_the_topic_never_reach_a_joined_subscriber: {leaked} callbacks ran for samples on other keys under {base}"
    );

    // Control 2: the group key and the base key deliver.
    let group = group_key("iso_groups", &UINT8);
    assert!(
        put_until(&raw, &group, &PAYLOAD, RECV, || calls
            .load(Ordering::SeqCst)
            > 0),
        "other_groups_of_the_topic_never_reach_a_joined_subscriber: control: nothing arrived on {group}"
    );
    let after_group = calls.load(Ordering::SeqCst);
    assert!(
        put_until(&raw, &base, &PAYLOAD, RECV, || calls.load(Ordering::SeqCst)
            > after_group),
        "other_groups_of_the_topic_never_reach_a_joined_subscriber: control: nothing arrived on {base}"
    );
}

// ---------------------------------------------------------------------------------------------
// The interop page
// ---------------------------------------------------------------------------------------------

/// Every claim in the buffer section of the interop page ends with `(not tested)` or with
/// `(tested: ...)` naming tests that exist in this directory. A claim is a sentence. The check
/// reports a claim with no marker and a marker that names a missing test, and two controls show
/// that it does.
#[test]
fn interop_page_marks_every_claim_tested_or_not_tested() {
    let tests_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let page = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/user-guide/interop.md"
    ))
    .expect("the interop page");
    let section = page
        .split("\n## Buffer messages from Lyrical publishers\n")
        .nth(1)
        .expect("the buffer section");
    let sources: String = std::fs::read_dir(tests_dir)
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
        .collect();
    let problems = |text: &str| -> Vec<String> {
        text.split("\n\n")
            .filter(|p| !p.trim_start().starts_with("A claim in this section"))
            .flat_map(|p| p.split(". "))
            .map(|s| s.trim().trim_end_matches('.').to_owned())
            .filter(|s| !s.is_empty() && !s.ends_with("(not tested)"))
            .filter(|s| {
                let names = s
                    .strip_suffix(')')
                    .and_then(|s| s.rsplit_once("(tested: "))
                    .map(|(_, names)| names.split('`').skip(1).step_by(2).collect::<Vec<_>>());
                match names {
                    Some(names) if !names.is_empty() => {
                        names.iter().any(|n| !sources.contains(&format!("fn {n}(")))
                    }
                    _ => true,
                }
            })
            .collect()
    };
    let bad = problems(section);
    assert!(
        bad.is_empty(),
        "interop_page_marks_every_claim_tested_or_not_tested: {} claims have no valid marker: {bad:#?}",
        bad.len()
    );
    // Controls: a claim that loses its marker, and a marker that names no test, are reported.
    let unmarked = section.replacen("(not tested).", ".", 1);
    let missing = section.replacen("`cache_joins_the_group`", "`no_such_test`", 1);
    assert_eq!(
        (problems(&unmarked).len(), problems(&missing).len()),
        (1, 1),
        "interop_page_marks_every_claim_tested_or_not_tested: control: the check missed a claim"
    );
}
