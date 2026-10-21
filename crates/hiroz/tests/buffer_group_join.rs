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
    Builder,
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
    let ctx = ZContextBuilder::default()
        .with_mode("peer")
        .disable_multicast_scouting()
        .with_connect_endpoints(Vec::<String>::new())
        .with_listen_endpoints(["tcp/127.0.0.1:0"])
        .build()
        .expect("context");
    let node = ctx.create_node("join_node").build().expect("node");
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
