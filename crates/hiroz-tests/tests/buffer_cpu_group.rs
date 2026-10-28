//! A hiroz subscriber to a buffer message must join rmw_zenoh's CPU buffer group.
//!
//! A buffer-aware `rmw_zenoh_cpp` publisher writes a message with a buffer field on the group key
//! for a subscriber whose token ends in `backends:cpu:`. These tests play the publisher with a raw
//! Zenoh session, so they need no ROS installation. They check the two things the publisher reads:
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
    cell::RefCell,
    collections::BTreeSet,
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use common::{
    buffer_rmw::{self, client_session as raw_session, put_until, router_allowing_only, tokens},
    *,
};
use hiroz::{
    BufferGroup, Builder, MessageTypeInfo,
    cache::ZCache,
    context::ZContextBuilder,
    dynamic::{DynamicMessage, FieldType, MessageSchema},
    qos::{QosDurability, QosProfile},
};
use hiroz_msgs::std_msgs::{Float64MultiArray, UInt8MultiArray};
use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter, RmwZenohFormatter};
use serde::{Deserialize, Serialize};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};
use zenoh::Wait;

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);
/// The wait for a further sample after one has arrived.
const DRAIN: Duration = Duration::from_millis(5);

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
/// `std_msgs/UInt8MultiArray` on the wire, without a message schema: the no-schema case.
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

/// The value of `counter` once it stops rising. `put_until` stops at the first sample, but samples
/// it put before then can still be in flight, so a read after a fixed sleep can be too early.
fn settled(counter: &std::sync::atomic::AtomicUsize) -> usize {
    let mut last = counter.load(std::sync::atomic::Ordering::SeqCst);
    loop {
        std::thread::sleep(POLL);
        let now = counter.load(std::sync::atomic::Ordering::SeqCst);
        if now == last {
            return now;
        }
        last = now;
    }
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

/// The delivery phases of a table row. Each phase puts a payload of its own on the row, so the
/// sample a row receives says which phase it came from.
const GROUP_IN: u32 = 1;
const GROUP_OUT: u32 = 2;
const BASE: u32 = 3;

/// The identity of the sample that `phase` puts on row number `row`.
fn tag(row: usize, phase: u32) -> u32 {
    (row as u32 + 1) * 10 + phase
}

/// The data every `tagged` payload carries.
const DATA: [u8; 3] = [1, 2, 3];

/// A `UInt8MultiArray` or `Blob` that carries `tag` in `layout.data_offset` and the data `DATA`:
/// header, no dimensions, the tag and three bytes. A receiver that decodes it must read the data
/// too, so a decoding fault shows as a failed assertion in `Tagged::tag`.
fn tagged(tag: u32) -> Vec<u8> {
    let mut bytes = tagged_empty(tag);
    let end = bytes.len() - 4;
    bytes.truncate(end);
    bytes.extend_from_slice(&(DATA.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&DATA);
    bytes
}

/// A message that carries `tag` in `layout.data_offset` and no data. It decodes as a
/// `Float64MultiArray` as well as a `UInt8MultiArray` or a `Blob`.
fn tagged_empty(tag: u32) -> Vec<u8> {
    let mut bytes = vec![0, 1, 0, 0, 0, 0, 0, 0];
    bytes.extend_from_slice(&tag.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes
}

/// The payload that a table row of `wire` receives: `Float64MultiArray` rows get no data.
fn row_payload(wire: &Wire, tag: u32) -> Vec<u8> {
    if wire.name == FLOAT64.name {
        tagged_empty(tag)
    } else {
        tagged(tag)
    }
}

/// A message that carries a sample identity in `layout.data_offset`.
trait Tagged {
    fn tag(&self) -> u32;
}
impl Tagged for UInt8MultiArray {
    fn tag(&self) -> u32 {
        assert_eq!(self.data, hiroz::ZBuf::from(DATA.to_vec()), "decoded data");
        self.layout.data_offset
    }
}
impl Tagged for Float64MultiArray {
    fn tag(&self) -> u32 {
        self.layout.data_offset
    }
}
impl Tagged for Blob {
    fn tag(&self) -> u32 {
        assert_eq!(self.data, DATA.to_vec(), "decoded data");
        self.layout.data_offset
    }
}
impl Tagged for DynamicMessage {
    fn tag(&self) -> u32 {
        let data = self.get_dynamic("data").expect("data field");
        assert_eq!(
            data.as_bytes().expect("data bytes"),
            &DATA[..],
            "decoded data"
        );
        self.get::<u32>("layout.data_offset")
            .expect("layout.data_offset")
    }
}

/// The identities of the samples a subscriber has queued. `recv` waits the given time for one sample.
fn drain<T: Tagged>(mut recv: impl FnMut(Duration) -> Option<T>) -> Vec<u32> {
    let mut tags = Vec::new();
    let mut wait = POLL;
    while let Some(message) = recv(wait) {
        tags.push(message.tag());
        wait = DRAIN;
    }
    tags
}

/// The identities of every message a cache holds.
fn cached<T: Tagged + hiroz::msg::ZMessage>(cache: &ZCache<T>) -> Vec<u32> {
    let end = SystemTime::now() + Duration::from_secs(3600);
    cache
        .get_interval(SystemTime::UNIX_EPOCH, end)
        .iter()
        .map(|message| message.tag())
        .collect()
}

/// One subscriber of a table. The table states what its token must announce. `probe` returns the
/// identity of every sample the subscriber holds that the previous call did not return, and a cache
/// returns all it holds. `delivery` is the message type to put on the subscriber's keys, and `None`
/// where the test has no independent key to put on.
struct Row {
    node: &'static str,
    topic: &'static str,
    name: &'static str,
    hash: Option<String>,
    delivery: Option<&'static Wire>,
    joined: bool,
    qos: &'static str,
    probe: Box<dyn Fn() -> Vec<u32>>,
}

impl Row {
    fn wired(
        topic: &'static str,
        wire: &'static Wire,
        joined: bool,
        probe: Box<dyn Fn() -> Vec<u32>>,
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
            probe: Box::new(Vec::new),
        }
    }
}

/// Check every row of a table against what a raw publisher sees. A row's token must be the whole
/// token of its shape, with the group field if and only if the row joined.
///
/// Each delivery phase puts a payload of its own, so a late sample of one phase cannot pass for
/// another. A row that joined must receive the sample of the group phase and the sample of the base
/// phase, because a publisher that is not buffer-aware writes the base key only. A row that stays
/// out must receive the sample of the base phase: that is its live positive control, and it is
/// separate from the check that no sample of the group phase arrived. That check runs last, after
/// the base sample arrived, so a delayed group sample has had time to show.
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
    let delivered: Vec<(usize, &Row)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.delivery.is_some())
        .collect();
    // Every identity a row has shown so far.
    let seen: Vec<RefCell<BTreeSet<u32>>> = rows.iter().map(|_| RefCell::default()).collect();
    let observe = |i: usize, row: &Row, want: u32| {
        seen[i].borrow_mut().extend((row.probe)());
        seen[i].borrow().contains(&want)
    };
    for &(i, row) in delivered.iter().filter(|(_, r)| r.joined) {
        let wire = row.delivery.unwrap();
        let key = group_key(row.topic, wire);
        let want = tag(i, GROUP_IN);
        assert!(
            put_until(raw, &key, &row_payload(wire, want), RECV, || observe(
                i, row, want
            )),
            "{table}: the joined row {} did not receive its group sample {want} on {key}: saw {:?}",
            row.topic,
            seen[i].borrow()
        );
    }
    for &(i, row) in delivered.iter().filter(|(_, r)| !r.joined) {
        let wire = row.delivery.unwrap();
        let key = group_key(row.topic, wire);
        for _ in 0..5 {
            raw.put(&key, row_payload(wire, tag(i, GROUP_OUT)))
                .wait()
                .expect("put");
        }
    }
    for &(i, row) in &delivered {
        let wire = row.delivery.unwrap();
        let key = base_key(row.topic, wire);
        let want = tag(i, BASE);
        assert!(
            put_until(raw, &key, &row_payload(wire, want), RECV, || observe(
                i, row, want
            )),
            "{table}: the {} row {} did not receive its base sample {want} on {key}: saw {:?}",
            if row.joined { "joined" } else { "control" },
            row.topic,
            seen[i].borrow()
        );
    }
    std::thread::sleep(POLL);
    for &(i, row) in delivered.iter().filter(|(_, r)| !r.joined) {
        let leaked = tag(i, GROUP_OUT);
        observe(i, row, leaked);
        assert!(
            !seen[i].borrow().contains(&leaked),
            "{table}: the row {} stays out and received its group sample {leaked} on {}: saw {:?}",
            row.topic,
            group_key(row.topic, row.delivery.unwrap()),
            seen[i].borrow()
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
            Box::new(move || drain(|wait| sub.recv_timeout(wait).ok())),
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
    let rows = vec![
        // The default joins a buffer type, with or without a schema.
        typed_row!(node, UInt8MultiArray, "def_typed", UINT8, true, |b| b),
        typed_row!(node, Blob, "def_blob", UINT8, true, |b| b),
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

/// The join policy of a dynamic subscriber, with a delivery check on the two rows that carry the
/// `UInt8MultiArray` schema. The schema decides under `Auto`: `dyn_nested` joins on a nested
/// unbounded `uint8[]`, and its twin `dyn_bounded` stays out on a bounded one. `Off` stays out of a
/// schema that would join. `dynamic_schema_shapes_decide_the_join` in the `hiroz` crate holds the
/// whole table of schema shapes.
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
            let probe = Box::new(move || drain(|wait| sub.recv_timeout(wait).ok()));
            Row::wired($topic, &UINT8, $joined, probe)
        }};
    }
    let (nested, _nested_sub) = adhoc_row!("dyn_nested", true, *msg());
    let (bounded, _bounded_sub) =
        adhoc_row!("dyn_bounded", false, FieldType::BoundedSequence(u8s(), 4));
    let rows = vec![
        uint8_row!("dyn_default", true, |b| b),
        uint8_row!("dyn_off", false, |b| b.with_buffer_group(BufferGroup::Off)),
        nested,
        bounded,
    ];
    check_rows(&raw, "dynamic_subscriber_join_policy", &rows);
}

macro_rules! cache_row {
    ($node:expr, $T:ty, $topic:expr, $wire:expr, $joined:expr, |$b:ident| $cfg:expr) => {{
        let $b = $node.create_cache::<$T>(concat!("/", $topic), 1000);
        let cache = $cfg.build().unwrap();
        Row::wired($topic, &$wire, $joined, Box::new(move || cached(&cache)))
    }};
}

/// The join policy of a cache. The cache holds 1000 messages, so no sample of a phase can push out
/// another. A cache of a buffer type joins and fills from the group key and from the base key. A
/// cache of a type with no byte field stays out. `Off` keeps a cache out of the group and it fills from the base key,
/// whether `with_buffer_group` comes before or after `with_stamp`. A cache that never sets the
/// group joins with a stamp too, so a missing token is the setting and not a dead setup.
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

/// The join policy of an action client. The status subscriber stays out, because its message has
/// no buffer field. A transient-local feedback subscriber stays out, as for every subscriber. The
/// feedback subscriber of the default client joins, and it is the control that shows the group
/// token is visible in this run.
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
    for (topic, expected) in [
        ("ac_default%_action%feedback", true),
        ("ac_default%_action%status", false),
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
    let before = settled(&calls);
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
    let before = settled(&stamped);
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
    buffer_rmw::assert_group_declared_before_token(
        "group_subscription_is_declared_before_the_token_is_visible",
        |topic| group_key(topic, &UINT8),
        |node, topic| {
            node.create_sub::<UInt8MultiArray>(&format!("/{topic}"))
                .with_buffer_group(BufferGroup::Auto)
                .build()
                .unwrap()
        },
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

/// A join logs one `debug` event that carries the subscribed key and the group key. Two joined
/// subscribers on one topic log two events, a joined subscriber with no schema on a second topic
/// logs one, and an `Off` subscriber logs none. The test selects the events by the marker
/// `CPU buffer group`, so a change of the wording after the marker does not fail it.
///
/// Controls in the same test: the joined subscribers announce the group in their tokens, so a
/// missing event is a missing log and not a missing join. The `Off` subscriber announces none, and
/// the events of the joined topics show that the capture works.
#[test]
fn joining_logs_the_group_key_at_debug_level() {
    const NAME: &str = "joining_logs_the_group_key_at_debug_level";
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
    let _a: Vec<_> = (0..2)
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
    let _off = node
        .create_sub::<UInt8MultiArray>("/log_off")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .unwrap();

    let raw = raw_session(router.endpoint());
    assert!(
        joined(&raw, "log_b"),
        "{NAME}: control: the joined subscriber has no group token"
    );
    assert!(
        !joined(&raw, "log_off"),
        "{NAME}: control: the opted-out subscriber has a group token"
    );
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("CPU buffer group"))
        .collect();
    let naming = |topic: &str| {
        let (base, group) = (base_key(topic, &UINT8), group_key(topic, &UINT8));
        lines
            .iter()
            .filter(|l| l.contains(&base) && l.contains(&group))
            .count()
    };
    assert_eq!(naming("log_a"), 2, "{NAME}: events for log_a: {text}");
    assert_eq!(naming("log_b"), 1, "{NAME}: events for log_b: {text}");
    assert_eq!(naming("log_off"), 0, "{NAME}: events for log_off: {text}");
    assert_eq!(lines.len(), 3, "{NAME}: one event per join: {text}");
    assert!(
        lines.iter().all(|l| l.contains(" DEBUG ")),
        "{NAME}: an event is not at debug level: {text}"
    );
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

/// A router that denies by default and allows the base key of every case, and the group key of
/// each case when `group` is set.
fn router_allowing_cases(cases: &[AclCase], group: bool) -> TestRouter {
    let mut keys = Vec::new();
    for c in cases {
        if group {
            keys.push(group_key(c.topic, c.wire()));
        }
        keys.push(base_key(c.topic, c.wire()));
    }
    router_allowing_only(&keys)
}

/// Router access control starves the group key of a joined subscriber silently. A joined
/// subscriber declares `<topic key>` and `<topic key>/_buf_cpu`, so a default-deny router that
/// allows `<topic key>` and not the group key starves the group key, and `build` still returns
/// `Ok`. The base key still flows. This holds for a buffer type and for a message with no
/// schema. A real publisher writes nothing on the group key of a topic that carries no buffer,
/// so the cases for that message put on the group key only to show that the rule denies it.
///
/// Controls in the same test: an `Off` subscriber receives under the allow-only rule, so the puts
/// reach the router and the rule lets the base key through. A joined subscriber receives on the
/// group key and on the base key once the rule allows exactly those two keys, which is the remedy
/// the docs give. The rule has no `<topic key>/**` entry, so a subscriber with a wider
/// subscription would receive nothing in the second table.
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

    // Allow the topic key only. The group key of each joined subscriber is starved, its base key
    // and each Off subscriber are not.
    let allow_base = [
        case("al_buf_off", false, Off, false, true),
        case("al_num_off", true, Off, false, true),
        case("al_buf", false, Auto, true, false),
        case("al_num", true, Auto, true, false),
        case("al_buf_base", false, Auto, false, true),
    ];
    run_acl_cases(
        NAME,
        &router_allowing_cases(&allow_base, false),
        &allow_base,
    );

    // Allow the topic key and the group key and nothing else, the remedy the docs give. A joined
    // subscriber receives on both keys. An Off subscriber receives on its base key.
    let allow_pair = [
        case("aw_buf", false, Auto, true, true),
        case("aw_buf_base", false, Auto, false, true),
        case("aw_num", true, Auto, true, true),
        case("aw_num_base", true, Auto, false, true),
        case("aw_off", false, Off, false, true),
    ];
    run_acl_cases(NAME, &router_allowing_cases(&allow_pair, true), &allow_pair);
}

// ---------------------------------------------------------------------------------------------
// The exact-key join: what a joined subscriber declares, and what it does not
// ---------------------------------------------------------------------------------------------

/// Whether a subscription matches `key`. A raw publisher on `key` reports a match as soon as one
/// appears, and a miss only after `RECV`, so a late match cannot pass for a miss.
fn key_is_subscribed(raw: &zenoh::Session, key: &str) -> bool {
    let publisher = raw
        .declare_publisher(key.to_owned())
        .wait()
        .expect("publisher");
    let end = Instant::now() + RECV;
    while Instant::now() < end {
        if publisher.matching_status().wait().unwrap().matching() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// For a key that must have no subscription: wait `QUIET`, then read once, so a match that arrives
/// late still fails the check.
fn key_stays_unsubscribed(raw: &zenoh::Session, key: &str) -> bool {
    let publisher = raw
        .declare_publisher(key.to_owned())
        .wait()
        .expect("publisher");
    std::thread::sleep(QUIET);
    !publisher.matching_status().wait().unwrap().matching()
}

/// Record the identity of a sample that a callback saw.
fn record(log: &Mutex<Vec<u32>>, tag: u32) {
    log.lock().unwrap().push(tag);
}

/// A sample on `<topic key>/_buf/<gid>` never reaches a joined subscriber, and no subscription of
/// the subscriber matches that key. A buffer-aware publisher writes such a sample for every
/// subscriber outside the group. The check reads the declared subscriptions through matching
/// status, so a filter in the callback cannot hide a subscription.
///
/// Controls in the same test: the base key and the group key each match the subscriber's
/// subscriptions, so the probe detects a match. The sibling key flips to matching once a plain
/// session subscribes `<topic key>/**`, and that session counts the same samples, so the samples
/// reach the router and the key would match a subscription on `<topic key>/**`. That session is a
/// control probe, not the code under test.
#[test]
fn sibling_sample_key_has_no_subscription_and_its_samples_never_reach_a_joined_subscriber() {
    const NAME: &str =
        "sibling_sample_key_has_no_subscription_and_its_samples_never_reach_a_joined_subscriber";
    const SIBLING_TAG: u32 = 1;
    const GROUP_TAG: u32 = 2;
    const BASE_TAG: u32 = 3;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let star = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let log: Arc<Mutex<Vec<u32>>> = Arc::default();
    let _sub = node
        .create_sub::<UInt8MultiArray>("/e2_sib")
        .with_buffer_group(BufferGroup::Auto)
        .build_with_callback({
            let log = log.clone();
            move |msg| record(&log, msg.tag())
        })
        .expect("sub");
    assert!(joined(&raw, "e2_sib"), "{NAME}: no group token");

    let base = base_key("e2_sib", &UINT8);
    let group = group_key("e2_sib", &UINT8);
    let sibling = format!("{base}/_buf/0123456789abcdef0123456789abcdef");
    assert!(
        key_is_subscribed(&raw, &base),
        "{NAME}: control: no subscription matches the base key {base}"
    );
    assert!(
        key_is_subscribed(&raw, &group),
        "{NAME}: control: no subscription matches the group key {group}"
    );
    let probe = raw
        .declare_publisher(sibling.clone())
        .wait()
        .expect("publisher");
    std::thread::sleep(QUIET);
    assert!(
        !probe.matching_status().wait().unwrap().matching(),
        "{NAME}: a subscription of the joined subscriber matches the sibling key {sibling}"
    );
    for _ in 0..10 {
        probe.put(tagged(SIBLING_TAG)).wait().expect("put");
    }
    std::thread::sleep(QUIET);
    assert!(
        log.lock().unwrap().is_empty(),
        "{NAME}: the joined subscriber saw samples of the sibling key: {:?}",
        log.lock().unwrap()
    );

    // Control: a subscription on `{base}/**` matches the sibling key and counts its samples.
    let star_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _star = star
        .declare_subscriber(format!("{base}/**"))
        .callback({
            let c = star_count.clone();
            move |_| {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        })
        .wait()
        .expect("control probe");
    let end = Instant::now() + RECV;
    while !probe.matching_status().wait().unwrap().matching() && Instant::now() < end {
        std::thread::sleep(POLL);
    }
    assert!(
        probe.matching_status().wait().unwrap().matching(),
        "{NAME}: control: the sibling key never matched the probe on <topic key>/**, so a match cannot be seen"
    );
    assert!(
        put_until(&raw, &sibling, &tagged(SIBLING_TAG), RECV, || star_count
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0),
        "{NAME}: control: the probe on <topic key>/** counted no sibling sample"
    );
    std::thread::sleep(QUIET);
    assert!(
        log.lock().unwrap().is_empty(),
        "{NAME}: the joined subscriber saw samples of the sibling key once the probe on <topic key>/** existed: {:?}",
        log.lock().unwrap()
    );

    // Control: both of the subscriber's own keys deliver, each its own sample.
    let has = |tag: u32| log.lock().unwrap().contains(&tag);
    assert!(
        put_until(&raw, &group, &tagged(GROUP_TAG), RECV, || has(GROUP_TAG)),
        "{NAME}: control: no sample on the group key: {:?}",
        log.lock().unwrap()
    );
    assert!(
        put_until(&raw, &base, &tagged(BASE_TAG), RECV, || has(BASE_TAG)),
        "{NAME}: control: no sample on the base key: {:?}",
        log.lock().unwrap()
    );
}

/// N joined subscribers on one topic announce one group token each. One sample on the base key and
/// one on the group key reach every joined subscriber once per key, so the subscribers see 2N
/// deliveries in all. An `Off` subscriber announces no `backends:` token field and receives on its
/// base key only.
///
/// This test pins the token count and the deliveries per key. It does not pin the number of
/// subscriptions: Zenoh does not list them, and any set of subscriptions that covers both keys
/// delivers each sample once. Two tests pin the subscriptions through matching status and router
/// access control:
/// `sibling_sample_key_has_no_subscription_and_its_samples_never_reach_a_joined_subscriber` and
/// `router_access_control_starves_a_joined_subscriber_silently`.
///
/// Controls in the same test: the `Off` subscriber receives its base sample, and no subscription
/// matches its group key, while a subscription matches the group key of the joined topic.
#[test]
fn n_joined_subscribers_announce_one_group_token_each_and_receive_on_both_keys() {
    const NAME: &str =
        "n_joined_subscribers_announce_one_group_token_each_and_receive_on_both_keys";
    const N: usize = 3;
    const BASE_TAG: u32 = 10;
    const GROUP_TAG: u32 = 20;
    const OFF_BASE_TAG: u32 = 30;
    const OFF_GROUP_TAG: u32 = 40;
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("tok_node").build().expect("node");
    let logs: Vec<Arc<Mutex<Vec<u32>>>> = (0..N).map(|_| Arc::default()).collect();
    let _joined: Vec<_> = logs
        .iter()
        .map(|log| {
            let log = log.clone();
            node.create_sub::<UInt8MultiArray>("/cnt_join")
                .build_with_callback(move |msg| record(&log, msg.tag()))
                .expect("joined sub")
        })
        .collect();
    let off_log: Arc<Mutex<Vec<u32>>> = Arc::default();
    let _off = node
        .create_sub::<UInt8MultiArray>("/cnt_off")
        .with_buffer_group(BufferGroup::Off)
        .build_with_callback({
            let log = off_log.clone();
            move |msg| record(&log, msg.tag())
        })
        .expect("off sub");

    let tokens_of = |topic: &str| -> Vec<String> {
        let needle = format!("/%{topic}/");
        tokens(&raw)
            .into_iter()
            .filter(|k| k.contains(&needle))
            .collect()
    };
    let end = Instant::now() + RECV;
    while (tokens_of("cnt_join").len() != N || tokens_of("cnt_off").len() != 1)
        && Instant::now() < end
    {
        std::thread::sleep(POLL);
    }
    let join_tokens = tokens_of("cnt_join");
    assert_eq!(
        join_tokens.len(),
        N,
        "{NAME}: tokens of the joined topic: {join_tokens:?}"
    );
    assert!(
        join_tokens.iter().all(|t| t.ends_with("/backends:cpu:")),
        "{NAME}: a joined token has no group field: {join_tokens:?}"
    );
    let off_tokens = tokens_of("cnt_off");
    assert!(
        off_tokens.len() == 1 && !off_tokens[0].contains("backends:"),
        "{NAME}: control: the Off token: {off_tokens:?}"
    );

    let join_base = base_key("cnt_join", &UINT8);
    let join_group = group_key("cnt_join", &UINT8);
    let off_base = base_key("cnt_off", &UINT8);
    let off_group = group_key("cnt_off", &UINT8);
    assert!(
        key_is_subscribed(&raw, &join_base) && key_is_subscribed(&raw, &join_group),
        "{NAME}: control: a key of the joined topic has no subscription"
    );
    assert!(
        key_is_subscribed(&raw, &off_base),
        "{NAME}: control: the Off base key has no subscription"
    );
    assert!(
        key_stays_unsubscribed(&raw, &off_group),
        "{NAME}: a subscription matches the group key of the Off subscriber"
    );

    raw.put(&join_base, tagged(BASE_TAG)).wait().unwrap();
    raw.put(&join_group, tagged(GROUP_TAG)).wait().unwrap();
    raw.put(&off_base, tagged(OFF_BASE_TAG)).wait().unwrap();
    raw.put(&off_group, tagged(OFF_GROUP_TAG)).wait().unwrap();
    let end = Instant::now() + RECV;
    while logs.iter().any(|l| l.lock().unwrap().len() < 2) && Instant::now() < end {
        std::thread::sleep(POLL);
    }
    std::thread::sleep(QUIET);
    let mut total = 0;
    for (i, log) in logs.iter().enumerate() {
        let mut seen = log.lock().unwrap().clone();
        seen.sort_unstable();
        total += seen.len();
        assert_eq!(
            seen,
            vec![BASE_TAG, GROUP_TAG],
            "{NAME}: subscriber {i} saw the wrong deliveries"
        );
    }
    assert_eq!(total, 2 * N, "{NAME}: deliveries in all");
    assert_eq!(
        *off_log.lock().unwrap(),
        vec![OFF_BASE_TAG],
        "{NAME}: control: the Off subscriber saw the wrong deliveries"
    );
}
