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
// The join policy, one table per builder
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

/// An empty `UInt8MultiArray`, `Float64MultiArray` or `Blob`: header, no dimensions, offset 0 and
/// no data. It decodes as each of the three.
const EMPTY: [u8; 16] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// One subscriber of a table. The table states what its token must announce. `probe` reads one
/// sample from the subscriber and says whether it got one. `delivery` is the message type to put
/// on the subscriber's keys, and `None` where the test has no independent key to put on.
struct Row {
    node: &'static str,
    topic: &'static str,
    name: &'static str,
    hash: Option<String>,
    delivery: Option<&'static Wire>,
    joined: bool,
    qos: &'static str,
    probe: Box<dyn Fn() -> bool>,
}

impl Row {
    fn wired(
        topic: &'static str,
        wire: &'static Wire,
        joined: bool,
        probe: Box<dyn Fn() -> bool>,
    ) -> Row {
        Row {
            node: "tok_node",
            topic,
            name: wire.name,
            hash: Some(wire.key_hash()),
            delivery: Some(wire),
            joined,
            qos: DEFAULT_QOS,
            probe,
        }
    }

    /// The row is on the node of this name.
    fn on(mut self, node: &'static str) -> Row {
        self.node = node;
        self
    }

    /// The row's token carries this QoS chunk.
    fn qos(mut self, qos: &'static str) -> Row {
        self.qos = qos;
        self
    }

    /// A subscriber of an ad-hoc schema: the hash is whatever hiroz derives, and nothing is put on
    /// its keys.
    fn adhoc(topic: &'static str, joined: bool) -> Row {
        Row {
            node: "tok_node",
            topic,
            name: "pkg::msg::dds_::M_",
            hash: None,
            delivery: None,
            joined,
            qos: DEFAULT_QOS,
            probe: Box::new(|| false),
        }
    }
}

/// Check every row of a table against what a raw publisher sees. A row's token must be the whole
/// token of its shape, with the group field if and only if the row joined. A row that joined must
/// receive a sample put on its group key. A row that stays out must receive nothing on the group
/// key, and must still receive a sample put on its base key: that is its live positive control,
/// and the joined twins of the table are the others. Rows that join and have a probe prove the
/// group key delivers, so they need no base-key check.
fn check_rows(raw: &zenoh::Session, table: &str, rows: &[Row]) {
    for row in rows {
        let shape = Shape {
            node: row.node,
            topic: row.topic,
            name: row.name,
            hash: row.hash.clone(),
            qos: row.qos,
            joined: row.joined,
        };
        assert_token(&token_for(raw, row.topic), shape);
    }
    let delivered: Vec<&Row> = rows.iter().filter(|r| r.delivery.is_some()).collect();
    for row in delivered.iter().filter(|r| r.joined) {
        let key = group_key(row.topic, row.delivery.unwrap());
        assert!(
            put_until(raw, &key, &EMPTY, RECV, || (row.probe)()),
            "{table}: the joined row {} received nothing on {key}",
            row.topic
        );
    }
    for row in delivered.iter().filter(|r| !r.joined) {
        let key = group_key(row.topic, row.delivery.unwrap());
        for _ in 0..5 {
            raw.put(&key, EMPTY.to_vec()).wait().expect("put");
        }
    }
    std::thread::sleep(POLL);
    for row in delivered.iter().filter(|r| !r.joined) {
        let wire = row.delivery.unwrap();
        assert!(
            !(row.probe)(),
            "{table}: the row {} stays out and received a sample on {}",
            row.topic,
            group_key(row.topic, wire)
        );
        let key = base_key(row.topic, wire);
        assert!(
            put_until(raw, &key, &EMPTY, RECV, || (row.probe)()),
            "{table}: control: the row {} received nothing on its base key {key}",
            row.topic
        );
    }
}

macro_rules! typed_row {
    ($node:expr, $T:ty, $topic:expr, $wire:expr, $joined:expr, |$b:ident| $cfg:expr) => {{
        let $b = $node.create_sub::<$T>(concat!("/", $topic));
        let sub = $cfg.build().unwrap();
        Row::wired(
            $topic,
            &$wire,
            $joined,
            Box::new(move || sub.recv_timeout(POLL).is_ok()),
        )
    }};
}

/// The join policy of a typed subscriber, one row per case. Each row that stays out has a joined
/// twin that differs in the property under test only: `def_typed` for `Off`, `float_cpu` for
/// `Auto` on a type with no byte field, `vol_default` and `vol_cpu` for transient-local,
/// `fmt_rmw_cpu` for the custom key format. A message with no schema (`Blob`) joins by default and
/// follows `Off` like a typed one. The token of every row is the whole expected token.
#[test]
fn typed_subscriber_join_policy() {
    assert_eq!(
        BufferGroup::default(),
        BufferGroup::Auto,
        "typed_subscriber_join_policy: the default group"
    );
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let custom_ctx = ZContextBuilder::default()
        .keyexpr_format(KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<
            RmwZenohFormatter,
        >::new())))
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .expect("custom ctx");
    let custom = custom_ctx.create_node("tok_node2").build().unwrap();
    let qos = |durability| QosProfile {
        durability,
        ..Default::default()
    };
    let (off, cpu) = (BufferGroup::Off, BufferGroup::Cpu);
    let (tl, vol) = (QosDurability::TransientLocal, QosDurability::Volatile);
    const TL_QOS: &str = ":1:,10:,:,:,,";
    const DEPTH_QOS: &str = "::,5:,:,:,,";
    let depth = QosProfile {
        history: QosHistory::KeepLast(std::num::NonZeroUsize::new(5).unwrap()),
        ..Default::default()
    };
    let rows = vec![
        // The default joins a buffer type, with or without a schema, and follows the QoS depth.
        typed_row!(node, UInt8MultiArray, "def_typed", UINT8, true, |b| b),
        typed_row!(node, Blob, "def_blob", UINT8, true, |b| b),
        typed_row!(node, UInt8MultiArray, "depth5", UINT8, true, |b| b
            .with_qos(depth))
        .qos(DEPTH_QOS),
        // Off stays out, with or without a schema.
        typed_row!(node, UInt8MultiArray, "def_off", UINT8, false, |b| b
            .with_buffer_group(off)),
        typed_row!(node, Blob, "blob_off", UINT8, false, |b| b
            .with_buffer_group(off)),
        // Auto stays out of a type with no byte field, and Cpu forces the join.
        typed_row!(node, Float64MultiArray, "float_auto", FLOAT64, false, |b| b),
        typed_row!(node, Float64MultiArray, "float_cpu", FLOAT64, true, |b| b
            .with_buffer_group(cpu)),
        // Transient-local stays out on the default and when forced. The volatile twins join.
        typed_row!(node, UInt8MultiArray, "tl_default", UINT8, false, |b| b
            .with_qos(qos(tl)))
        .qos(TL_QOS),
        typed_row!(node, UInt8MultiArray, "vol_default", UINT8, true, |b| b
            .with_qos(qos(vol))),
        typed_row!(node, UInt8MultiArray, "tl_cpu", UINT8, false, |b| b
            .with_qos(qos(tl))
            .with_buffer_group(cpu))
        .qos(TL_QOS),
        typed_row!(node, UInt8MultiArray, "vol_cpu", UINT8, true, |b| b
            .with_qos(qos(vol))
            .with_buffer_group(cpu)),
        // A custom key format stays out on the default and when forced. The rmw_zenoh format joins.
        typed_row!(node, UInt8MultiArray, "fmt_rmw_cpu", UINT8, true, |b| b
            .with_buffer_group(cpu)),
        typed_row!(custom, UInt8MultiArray, "fmt_custom", UINT8, false, |b| b).on("tok_node2"),
        typed_row!(
            custom,
            UInt8MultiArray,
            "fmt_custom_cpu",
            UINT8,
            false,
            |b| b.with_buffer_group(cpu)
        )
        .on("tok_node2"),
    ];
    check_rows(&raw, "typed_subscriber_join_policy", &rows);
}

/// The join policy of a dynamic subscriber. The schema decides under `Auto`: an unbounded `uint8[]`
/// joins, directly or nested in a message, an array or a sequence, and a bounded sequence or a
/// fixed array of `uint8` does not. The unbounded twin `dyn_bytes` shows the same schema shape
/// joins. `Off` stays out of a schema that would join.
#[test]
fn dynamic_subscriber_join_policy() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let u8s = || Box::new(FieldType::Uint8);
    let bytes = || FieldType::Sequence(u8s());
    let msg = || Box::new(FieldType::Message(schema(bytes())));
    macro_rules! adhoc_row {
        ($topic:expr, $joined:expr, $ft:expr) => {{
            let sub = node
                .create_dyn_sub(concat!("/", $topic), schema($ft))
                .build()
                .unwrap();
            (Row::adhoc($topic, $joined), sub)
        }};
    }
    macro_rules! uint8_row {
        ($topic:expr, $joined:expr, |$b:ident| $cfg:expr) => {{
            let message = UInt8MultiArray::message_schema().unwrap();
            let $b = node.create_dyn_sub(concat!("/", $topic), message);
            let sub = $cfg.build().unwrap();
            let probe = Box::new(move || sub.recv_timeout(POLL).is_ok());
            Row::wired($topic, &UINT8, $joined, probe)
        }};
    }
    let (r1, _s1) = adhoc_row!("dyn_bytes", true, bytes());
    let (r2, _s2) = adhoc_row!("dyn_nested", true, *msg());
    let (r3, _s3) = adhoc_row!("dyn_array", true, FieldType::Array(msg(), 2));
    let (r4, _s4) = adhoc_row!("dyn_seq", true, FieldType::Sequence(msg()));
    let (r5, _s5) = adhoc_row!("dyn_bounded", false, FieldType::BoundedSequence(u8s(), 4));
    let (r6, _s6) = adhoc_row!("dyn_fixed", false, FieldType::Array(u8s(), 4));
    let rows = vec![
        uint8_row!("dyn_default", true, |b| b),
        uint8_row!("dyn_off", false, |b| b.with_buffer_group(BufferGroup::Off)),
        r1,
        r2,
        r3,
        r4,
        r5,
        r6,
    ];
    check_rows(&raw, "dynamic_subscriber_join_policy", &rows);
}

macro_rules! cache_row {
    ($node:expr, $T:ty, $topic:expr, $wire:expr, $joined:expr, |$b:ident| $cfg:expr) => {{
        let $b = $node.create_cache::<$T>(concat!("/", $topic), 10);
        let cache = $cfg.build().unwrap();
        Row::wired($topic, &$wire, $joined, Box::new(move || !cache.is_empty()))
    }};
}

/// The join policy of a cache. A cache of a buffer type joins and fills from the group key. A cache
/// of a type with no byte field stays out. `Off` keeps a cache out of the group and it fills from
/// the base key, whether `with_buffer_group` comes before or after `with_stamp`. A cache that never
/// sets the group joins with a stamp too, so a missing token is the setting and not a dead setup.
#[test]
fn cache_join_policy() {
    let stamp = |_: &UInt8MultiArray| std::time::SystemTime::now();
    let off = BufferGroup::Off;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let rows = vec![
        cache_row!(node, UInt8MultiArray, "cache_default", UINT8, true, |b| b),
        cache_row!(node, UInt8MultiArray, "cache_stamped", UINT8, true, |b| b
            .with_stamp(stamp)),
        cache_row!(
            node,
            Float64MultiArray,
            "cache_plain",
            FLOAT64,
            false,
            |b| b
        ),
        cache_row!(node, UInt8MultiArray, "cache_off", UINT8, false, |b| b
            .with_buffer_group(off)),
        cache_row!(node, UInt8MultiArray, "order_before", UINT8, false, |b| b
            .with_buffer_group(off)
            .with_stamp(stamp)),
        cache_row!(node, UInt8MultiArray, "order_after", UINT8, false, |b| b
            .with_stamp(stamp)
            .with_buffer_group(off)),
    ];
    check_rows(&raw, "cache_join_policy", &rows);
}

/// The join policy of an action client. With no schema the feedback and status subscribers join on
/// topics that carry no buffer, as a typed subscriber with no schema does. A transient-local
/// feedback subscriber stays out, and the feedback subscriber of a volatile client joins as the
/// control.
#[test]
fn action_client_join_policy() {
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let qos = |durability| QosProfile {
        durability,
        ..Default::default()
    };
    let _default = node
        .create_action_client::<NoSchemaAction>("ac_default")
        .build()
        .unwrap();
    let _transient = node
        .create_action_client::<NoSchemaAction>("ac_transient")
        .with_feedback_topic_qos(qos(QosDurability::TransientLocal))
        .build()
        .unwrap();
    let _volatile = node
        .create_action_client::<NoSchemaAction>("ac_volatile")
        .with_feedback_topic_qos(qos(QosDurability::Volatile))
        .build()
        .unwrap();
    for (topic, expected) in [
        ("ac_default%_action%feedback", true),
        ("ac_default%_action%status", true),
        ("ac_volatile%_action%feedback", true),
        ("ac_transient%_action%feedback", false),
    ] {
        assert_eq!(
            joined(&raw, topic),
            expected,
            "action_client_join_policy: group token on {topic}"
        );
    }
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

/// Dropping a joined typed subscriber removes its token and stops its callback on the group key.
/// The token is present and the callback runs on that key before the drop.
#[test]
fn dropping_a_joined_typed_subscriber_removes_its_token_and_stops_callbacks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const NAME: &str = "dropping_a_joined_typed_subscriber_removes_its_token_and_stops_callbacks";
    const AFTER_DROP_PUTS: usize = 10;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = node
        .create_sub::<UInt8MultiArray>("/drop_topic")
        .with_buffer_group(BufferGroup::Auto)
        .build_with_callback({
            let calls = calls.clone();
            move |_msg| {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        })
        .expect("sub");
    assert_token(
        &token_for(&raw, "drop_topic"),
        shape("tok_node", "drop_topic", &UINT8, true),
    );
    let key = group_key("drop_topic", &UINT8);

    // Positive control: the callback runs on the group key before the drop.
    assert!(
        put_until(&raw, &key, &PAYLOAD, RECV, || calls.load(Ordering::SeqCst)
            > 0),
        "{NAME}: control: no callback ran on {key} before the drop"
    );
    std::thread::sleep(POLL);
    let before = calls.load(Ordering::SeqCst);
    assert!(
        before >= 1,
        "{NAME}: control: {before} callbacks before the drop"
    );

    drop(sub);
    let deadline = Instant::now() + RECV;
    let count = || {
        tokens(&raw)
            .iter()
            .filter(|k| k.contains("/%drop_topic/"))
            .count()
    };
    let mut left = count();
    while left > 0 && Instant::now() < deadline {
        std::thread::sleep(POLL);
        left = count();
    }
    assert!(
        left == 0,
        "{NAME}: the token outlived the subscriber: {left} left"
    );
    for _ in 0..AFTER_DROP_PUTS {
        raw.put(&key, PAYLOAD.to_vec()).wait().expect("put");
    }
    std::thread::sleep(QUIET);
    let after = calls.load(Ordering::SeqCst);
    assert!(
        after == before,
        "{NAME}: {} callbacks ran on {key} after the drop ({AFTER_DROP_PUTS} puts, {before} before)",
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
// The debug log and access control
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

/// Each join logs one `debug` line that names the subscribed key and the group key. Three
/// subscribers on one key log three lines; a second key logs one; a subscriber that stays out logs
/// nothing.
#[test]
fn joining_logs_the_group_key_at_debug_level() {
    let out = Captured::default();
    // Only the hiroz target is enabled: a debug span of another crate would cross threads and
    // break the thread-local subscriber.
    let _guard = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer({
                    let out = out.clone();
                    move || out.clone()
                })
                .with_ansi(false),
        )
        .with(Targets::new().with_target("hiroz", LevelFilter::DEBUG))
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
        "joining_logs_the_group_key_at_debug_level: control: the joined subscriber has no group token"
    );
    assert!(
        !token_for(&raw, "log_off").contains("backends:"),
        "joining_logs_the_group_key_at_debug_level: control: the opted-out subscriber has a group token"
    );
    let count = |topic: &str| {
        lines
            .iter()
            .filter(|l| l.ends_with(&expected(topic)))
            .count()
    };
    assert!(
        lines.len() == 4,
        "joining_logs_the_group_key_at_debug_level: want one line per join, got {}: {text}",
        lines.len()
    );
    assert_eq!(
        count("log_a"),
        3,
        "joining_logs_the_group_key_at_debug_level: lines for log_a: {text}"
    );
    assert_eq!(
        count("log_b"),
        1,
        "joining_logs_the_group_key_at_debug_level: lines for log_b: {text}"
    );
    assert!(
        lines.iter().all(|l| l.contains(" DEBUG ")),
        "joining_logs_the_group_key_at_debug_level: a line is not at debug level: {text}"
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

/// `Num` as a token and a key write it: a message with no schema and no byte field.
const NUM: Wire = Wire {
    name: "test_msgs::msg::dds_::Num_",
    hash: "RIHS01_0000000000000000000000000000000000000000000000000000000000000000",
};

/// CDR header and one little-endian u32.
const NUM_PAYLOAD: [u8; 8] = [0, 1, 0, 0, 7, 0, 0, 0];

/// One subscriber under a router rule. Each case of a run has its own topic. `num` selects the
/// message with no schema, and otherwise the subscriber reads a buffer type. `group` selects the
/// key a raw publisher puts on: the group key or the base key.
struct AclCase {
    topic: &'static str,
    num: bool,
    mode: BufferGroup,
    group: bool,
    delivers: bool,
}

impl AclCase {
    fn wire(&self) -> &'static Wire {
        if self.num { &NUM } else { &UINT8 }
    }

    fn key(&self) -> String {
        if self.group {
            group_key(self.topic, self.wire())
        } else {
            base_key(self.topic, self.wire())
        }
    }
}

/// Build one subscriber per case on a node of `router`, put samples on each case's key, and check
/// what reached it. `build` returns `Ok` under every rule: the starvation is silent.
fn run_acl_cases(name: &str, router: &TestRouter, cases: &[AclCase]) {
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let subs: Vec<Box<dyn Fn() -> bool>> = cases
        .iter()
        .map(|c| -> Box<dyn Fn() -> bool> {
            let topic = format!("/{}", c.topic);
            if c.num {
                let sub = node
                    .create_sub::<Num>(&topic)
                    .with_buffer_group(c.mode)
                    .build()
                    .expect("build returns Ok under a deny rule");
                Box::new(move || sub.recv_timeout(POLL).is_ok())
            } else {
                let sub = node
                    .create_sub::<UInt8MultiArray>(&topic)
                    .with_buffer_group(c.mode)
                    .build()
                    .expect("build returns Ok under a deny rule");
                Box::new(move || sub.recv_timeout(POLL).is_ok())
            }
        })
        .collect();
    // The token is declared after the subscription, so a visible token ends the wait for it.
    for c in cases {
        let token = token_for(&raw, c.topic);
        assert_eq!(
            token.ends_with("/backends:cpu:"),
            c.mode == BufferGroup::Auto,
            "{name}: token for {} ({:?}): {token}",
            c.topic,
            c.mode
        );
    }
    for _ in 0..20 {
        for c in cases {
            let payload: &[u8] = if c.num { &NUM_PAYLOAD } else { &PAYLOAD };
            raw.put(c.key(), payload.to_vec()).wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for (c, recv) in cases.iter().zip(&subs) {
        let mut received = 0;
        while recv() {
            received += 1;
        }
        assert!(
            (received > 0) == c.delivers,
            "{name}: {} ({:?}, group key {}): received {received}, expected {}",
            c.topic,
            c.mode,
            c.group,
            if c.delivers { "some" } else { "none" }
        );
    }
}

/// A router that denies by default and allows the base key of every case, and the keys under each
/// base key when `wide` is set.
fn router_allowing_cases(cases: &[AclCase], wide: bool) -> TestRouter {
    let mut keys = Vec::new();
    for c in cases {
        let base = base_key(c.topic, c.wire());
        if wide {
            keys.push(format!("{base}/**"));
        }
        keys.push(base);
    }
    router_allowing_only(&keys)
}

/// Router access control starves a joined subscriber silently. A joined subscriber declares
/// `<topic key>/**`, so a default-deny router that allows `<topic key>` and not `<topic key>/**`
/// starves it, and `build` still returns `Ok`. This holds for a buffer type and for a subscriber
/// with no message definition, which joins on a topic that carries no buffer too. A rule that
/// denies the group key starves the group key and leaves the base key flowing.
///
/// Controls in the same test: an `Off` subscriber receives under the allow-only rule, so the puts
/// reach the router and the rule lets the base key through. A joined subscriber receives once the
/// rule also allows `<topic key>/**`, which is the remedy the docs give. Without the deny rule the
/// same joined subscriber receives on the group key.
#[test]
fn router_access_control_starves_a_joined_subscriber_silently() {
    const NAME: &str = "router_access_control_starves_a_joined_subscriber_silently";
    use BufferGroup::{Auto, Off};
    let case = |topic, num, mode, group, delivers| AclCase {
        topic,
        num,
        mode,
        group,
        delivers,
    };

    // Allow the topic key only. Each joined subscriber is starved, each Off subscriber is not.
    let allow_base = [
        case("al_buf_off", false, Off, false, true),
        case("al_num_off", true, Off, false, true),
        case("al_buf", false, Auto, false, false),
        case("al_num", true, Auto, false, false),
    ];
    run_acl_cases(
        NAME,
        &router_allowing_cases(&allow_base, false),
        &allow_base,
    );

    // Allow the topic key and everything under it, the remedy the docs give.
    let allow_wide = [
        case("aw_buf", false, Auto, false, true),
        case("aw_num", true, Auto, false, true),
    ];
    run_acl_cases(NAME, &router_allowing_cases(&allow_wide, true), &allow_wide);

    // Deny the group key. The same joined subscriber receives on the group key of an open router.
    let open = [case("ad_buf", false, Auto, true, true)];
    run_acl_cases(NAME, &TestRouter::new(), &open);
    let deny = [
        case("ad_group", false, Auto, true, false),
        case("ad_base", false, Auto, false, true),
    ];
    run_acl_cases(NAME, &router_denying_the_group_key(), &deny);
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
