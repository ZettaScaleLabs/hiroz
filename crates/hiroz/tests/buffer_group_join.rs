//! Which message shapes make a subscriber join rmw_zenoh's CPU buffer group.
//!
//! A joined subscriber announces the group at the end of its liveliness token. Each test builds
//! subscribers on one session with no router and reads the tokens from that session. The tests
//! cover the schema rule that decides under `BufferGroup::Auto`. The `buffer_cpu_group` tests of
//! `hiroz-tests` cover delivery and router access control.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hiroz::{
    BufferGroup, Builder, SubscriberOptions,
    context::{ZContext, ZContextBuilder},
    dynamic::{FieldType, MessageSchema},
    node::ZNode,
};
use hiroz_msgs::{
    rcl_interfaces::ParameterEvent,
    std_msgs::{ByteMultiArray, Float64MultiArray, Int8MultiArray, UInt8MultiArray},
};
use zenoh::Wait;

/// A node on a session that has no router and no peer, so every token it sees is its own.
fn isolated_node() -> (ZContext, ZNode) {
    isolated_node_with(SubscriberOptions::default())
}

/// As [`isolated_node`], with `options` as the node-level subscriber default.
fn isolated_node_with(options: SubscriberOptions) -> (ZContext, ZNode) {
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx
        .create_node("join_node")
        .with_subscriber_options(options)
        .build()
        .expect("node");
    (ctx, node)
}

/// Whether the one token for `topic` ends in `backends:cpu:`. A token that never appears, or
/// appears twice, fails the test.
fn joined(node: &ZNode, topic: &str) -> bool {
    let needle = format!("/%{topic}/");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let replies = node
            .session()
            .liveliness()
            .get("@ros2_lv/**")
            .timeout(Duration::from_secs(2))
            .wait()
            .expect("liveliness get");
        let mut hits = Vec::new();
        while let Ok(reply) = replies.recv() {
            if let Ok(sample) = reply.result() {
                let key = sample.key_expr().to_string();
                if key.contains(&needle) {
                    hits.push(key);
                }
            }
        }
        match hits.len() {
            1 => return hits[0].ends_with("/backends:cpu:"),
            0 if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            n => panic!("{n} tokens for {topic}: {hits:?}"),
        }
    }
}

/// A one-field message schema.
fn schema(field_type: FieldType) -> Arc<MessageSchema> {
    MessageSchema::builder("pkg/msg/M")
        .field("f", field_type)
        .build()
        .unwrap()
}

/// The join decision of a dynamic subscriber follows its schema. An unbounded `uint8[]` joins,
/// directly or nested in a message, an array or a sequence. A bounded sequence, a fixed array, an
/// unbounded `int8[]` and a sequence of another type do not.
///
/// Control in the same test: the first row joins, so a missing token field is the rule and not a
/// subscriber that never announces the group.
#[test]
fn dynamic_schema_shapes_decide_the_join() {
    const NAME: &str = "dynamic_schema_shapes_decide_the_join";
    let (_ctx, node) = isolated_node();
    let u8s = || Box::new(FieldType::Uint8);
    let bytes = || FieldType::Sequence(u8s());
    let msg = |inner: FieldType| Box::new(FieldType::Message(schema(inner)));
    let rows: Vec<(&str, FieldType, bool)> = vec![
        ("bytes", bytes(), true),
        ("nested", *msg(bytes()), true),
        ("array_of_messages", FieldType::Array(msg(bytes()), 2), true),
        (
            "sequence_of_messages",
            FieldType::Sequence(msg(bytes())),
            true,
        ),
        ("bounded", FieldType::BoundedSequence(u8s(), 4), false),
        ("fixed", FieldType::Array(u8s(), 4), false),
        (
            "int8_sequence",
            FieldType::Sequence(Box::new(FieldType::Int8)),
            false,
        ),
        (
            "f64_sequence",
            FieldType::Sequence(Box::new(FieldType::Float64)),
            false,
        ),
        ("nested_without_bytes", *msg(FieldType::Float64), false),
    ];
    let _subs: Vec<_> = rows
        .iter()
        .map(|(topic, field_type, _)| {
            node.create_dyn_sub(&format!("/{topic}"), schema(field_type.clone()))
                .build()
                .unwrap()
        })
        .collect();
    let observed: Vec<(&str, bool)> = rows
        .iter()
        .map(|(topic, _, _)| (*topic, joined(&node, topic)))
        .collect();
    let expected: Vec<(&str, bool)> = rows
        .iter()
        .map(|(topic, _, want)| (*topic, *want))
        .collect();
    assert_eq!(
        observed, expected,
        "{NAME}: (topic, joined) as observed, then as expected"
    );
}

/// The join decision of a typed subscriber follows the message definition that the code generator
/// wrote. An unbounded `uint8[]` joins, and so does an unbounded `byte[]`: the generator maps ROS
/// `byte` and `uint8` to one type, so the typed rule is wider than the rule of `rmw_zenoh_cpp`.
/// A nested `byte[]` joins. An unbounded `int8[]` and a `float64[]` do not.
///
/// Control in the same test: `UInt8MultiArray` joins.
#[test]
fn typed_message_fields_decide_the_join() {
    const NAME: &str = "typed_message_fields_decide_the_join";
    let (_ctx, node) = isolated_node();
    let _uint8 = node
        .create_sub::<UInt8MultiArray>("/uint8")
        .build()
        .unwrap();
    let _byte = node.create_sub::<ByteMultiArray>("/byte").build().unwrap();
    let _int8 = node.create_sub::<Int8MultiArray>("/int8").build().unwrap();
    let _f64 = node
        .create_sub::<Float64MultiArray>("/f64")
        .build()
        .unwrap();
    let _nested = node
        .create_sub::<ParameterEvent>("/nested")
        .build()
        .unwrap();
    let observed: Vec<(&str, bool)> = ["uint8", "byte", "int8", "f64", "nested"]
        .into_iter()
        .map(|topic| (topic, joined(&node, topic)))
        .collect();
    assert_eq!(
        observed,
        vec![
            ("uint8", true),
            ("byte", true),
            ("int8", false),
            ("f64", false),
            ("nested", true),
        ],
        "{NAME}: (topic, joined) as observed, then as expected"
    );
}

/// A node-level default group applies to a subscriber that sets none, and a group set on the
/// builder wins over it. `UInt8MultiArray` joins under `Auto` and `Float64MultiArray` does not, so
/// the two message types show a default that forces a join and a default that forces a stay-out.
///
/// Control in the same test: a node with no options joins the first type and not the second, so
/// each row differs from that baseline only through the node default or the builder choice.
#[test]
fn node_default_group_applies_and_builder_overrides() {
    const NAME: &str = "node_default_group_applies_and_builder_overrides";
    let default_node = isolated_node();
    let off_node =
        isolated_node_with(SubscriberOptions::default().with_buffer_group(BufferGroup::Off));
    let cpu_node =
        isolated_node_with(SubscriberOptions::default().with_buffer_group(BufferGroup::Cpu));

    let _d = (
        default_node
            .1
            .create_sub::<UInt8MultiArray>("/d_u8")
            .build()
            .unwrap(),
        default_node
            .1
            .create_sub::<Float64MultiArray>("/d_f64")
            .build()
            .unwrap(),
    );
    let _off = (
        off_node
            .1
            .create_sub::<UInt8MultiArray>("/off_u8")
            .build()
            .unwrap(),
        off_node
            .1
            .create_sub::<UInt8MultiArray>("/off_u8_cpu")
            .with_buffer_group(BufferGroup::Cpu)
            .build()
            .unwrap(),
        off_node
            .1
            .create_sub::<UInt8MultiArray>("/off_u8_auto")
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap(),
    );
    let _cpu = (
        cpu_node
            .1
            .create_sub::<Float64MultiArray>("/cpu_f64")
            .build()
            .unwrap(),
        cpu_node
            .1
            .create_sub::<Float64MultiArray>("/cpu_f64_off")
            .with_buffer_group(BufferGroup::Off)
            .build()
            .unwrap(),
        cpu_node
            .1
            .create_sub::<Float64MultiArray>("/cpu_f64_auto")
            .with_buffer_group(BufferGroup::Auto)
            .build()
            .unwrap(),
    );
    let observed: Vec<(&str, bool)> = vec![
        ("d_u8", joined(&default_node.1, "d_u8")),
        ("d_f64", joined(&default_node.1, "d_f64")),
        ("off_u8", joined(&off_node.1, "off_u8")),
        ("off_u8_cpu", joined(&off_node.1, "off_u8_cpu")),
        ("off_u8_auto", joined(&off_node.1, "off_u8_auto")),
        ("cpu_f64", joined(&cpu_node.1, "cpu_f64")),
        ("cpu_f64_off", joined(&cpu_node.1, "cpu_f64_off")),
        ("cpu_f64_auto", joined(&cpu_node.1, "cpu_f64_auto")),
    ];
    assert_eq!(
        observed,
        vec![
            ("d_u8", true),
            ("d_f64", false),
            ("off_u8", false),
            ("off_u8_cpu", true),
            ("off_u8_auto", true),
            ("cpu_f64", true),
            ("cpu_f64_off", false),
            ("cpu_f64_auto", false),
        ],
        "{NAME}: (topic, joined) as observed, then as expected"
    );
}

/// A raw subscriber that sets no group follows the node default. `create_raw_subscriber_with_group`
/// takes the group as an argument and ignores the node default.
#[cfg(feature = "ffi")]
#[test]
fn raw_subscriber_follows_node_default() {
    const NAME: &str = "raw_subscriber_follows_node_default";
    const TYPE_NAME: &str = "std_msgs::msg::dds_::UInt8MultiArray_";
    const TYPE_HASH: &str =
        "RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";
    let (_default_ctx, default_node) = isolated_node();
    let (_off_ctx, off_node) =
        isolated_node_with(SubscriberOptions::default().with_buffer_group(BufferGroup::Off));
    let _default = default_node
        .create_raw_subscriber("/raw_default", TYPE_NAME, TYPE_HASH, |_| {})
        .unwrap();
    let _off = off_node
        .create_raw_subscriber("/raw_off", TYPE_NAME, TYPE_HASH, |_| {})
        .unwrap();
    let _explicit = off_node
        .create_raw_subscriber_with_group(
            "/raw_explicit",
            TYPE_NAME,
            TYPE_HASH,
            |_| {},
            None,
            BufferGroup::Cpu,
        )
        .unwrap();
    assert!(
        joined(&default_node, "raw_default"),
        "{NAME}: control, a node with no options joins a raw subscriber"
    );
    assert!(
        joined(&off_node, "raw_explicit"),
        "{NAME}: an explicit group on the raw entry point wins over the node default"
    );
    // The explicit subscriber's token is visible, so a token of `raw_off` would be too. A raw
    // subscriber that stays out declares no token.
    let replies = off_node
        .session()
        .liveliness()
        .get("@ros2_lv/**")
        .timeout(Duration::from_secs(2))
        .wait()
        .expect("liveliness get");
    let mut stayed_out = true;
    while let Ok(reply) = replies.recv() {
        if let Ok(sample) = reply.result() {
            stayed_out &= !sample.key_expr().as_str().contains("/%raw_off/");
        }
    }
    assert!(
        stayed_out,
        "{NAME}: a raw subscriber under a node default of Off declared a token"
    );
}
