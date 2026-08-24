//! The library and `rmw-zenoh-rs` must derive the same topic key expression.
//!
//! A hiroz library node and a ROS 2 node running `rmw_zenoh_rs` publish on a key
//! expression built from the type name. If the two derive that name differently,
//! they match no topic and exchange nothing. Neither side reports an error.
//!
//! These tests pin the derivation against the form measured on the wire from
//! upstream `rmw_zenoh_cpp`.

use hiroz_protocol::{
    entity::{EndpointEntity, EndpointKind, NodeEntity, TypeHash, TypeInfo},
    // `topic_key_expr` is an associated function on the KeyExprFormatter trait,
    // so the trait must be in scope here.
    format::{rmw_zenoh::dds_type_name, rmw_zenoh::RmwZenohFormatter, KeyExprFormatter},
    qos::QosProfile,
};
use hiroz_schema::type_name::service_from_response;

/// The exact string upstream `rmw_zenoh_cpp` was measured to declare.
///
/// Recorded from `ros-jazzy-rmw-zenoh-cpp` running `demo_nodes_cpp talker`, with
/// a raw zenoh subscriber on `**` attached to its router:
///
/// ```text
/// 0/chatter/std_msgs::msg::dds_::String_/RIHS01_df668c74...
/// ```
const WIRE_TYPE_NAME: &str = "std_msgs::msg::dds_::String_";

fn zid() -> zenoh::session::ZenohId {
    "1234567890abcdef1234567890abcdef".parse().unwrap()
}

/// An endpoint of `kind` on `topic`, carrying `type_name`.
fn endpoint(kind: EndpointKind, topic: &str, type_name: &str) -> EndpointEntity {
    let node = NodeEntity::new(
        0,
        zid(),
        0,
        "talker".to_string(),
        "/".to_string(),
        String::new(),
    );
    EndpointEntity {
        id: 1,
        node: Some(node),
        kind,
        topic: topic.to_string(),
        type_info: Some(TypeInfo::new(type_name, TypeHash::zero())),
        qos: QosProfile::default(),
    }
}

/// Build the topic key expression for `/chatter` carrying `type_name`.
fn chatter_key(type_name: &str) -> String {
    let zid: zenoh::session::ZenohId = "1234567890abcdef1234567890abcdef".parse().unwrap();
    let node = NodeEntity::new(
        0,
        zid,
        0,
        "talker".to_string(),
        "/".to_string(),
        String::new(),
    );
    let entity = EndpointEntity {
        id: 1,
        node: Some(node),
        kind: EndpointKind::Publisher,
        topic: "chatter".to_string(),
        type_info: Some(TypeInfo::new(type_name, TypeHash::zero())),
        qos: QosProfile::default(),
    };
    RmwZenohFormatter::topic_key_expr(&entity)
        .unwrap()
        .as_str()
        .to_string()
}

/// A hiroz library node and a `rmw_zenoh_rs` node must publish `std_msgs/String`
/// on the SAME key expression, or they cannot hear each other at all.
///
/// The library agrees with the measured wire form. Before this test,
/// `rmw-zenoh-rs` derived `std_msgs/msg/String` instead. It then published on
///
/// ```text
/// 0/chatter/std_msgs/msg/String/RIHS01_df668c74...
/// ```
///
/// Same domain, same topic, byte-identical type hash. Only the type-name
/// segment differed, and the two stacks were silently deaf to each other.
#[test]
fn library_and_rmw_derive_the_same_topic_key() {
    // What rmw-zenoh-rs derives from the C++ typesupport for this type:
    // message_namespace_ = "std_msgs::msg", message_name_ = "String".
    let rmw_type_name = dds_type_name("std_msgs::msg", "String");

    assert_eq!(
        rmw_type_name, WIRE_TYPE_NAME,
        "rmw-zenoh-rs derives a different type name than the library, and than \
         rmw_zenoh_cpp puts on the wire"
    );

    // The whole key expression must agree, not only the segment.
    assert_eq!(
        chatter_key(&rmw_type_name),
        chatter_key(WIRE_TYPE_NAME),
        "topic key expressions disagree"
    );
    assert_eq!(
        chatter_key(WIRE_TYPE_NAME),
        format!("0/chatter/{WIRE_TYPE_NAME}/{}", TypeHash::zero()),
    );
}

/// The service key uses the same mangling on the request and response pair.
///
/// This calls `service_from_response`, which is the rule the production path
/// uses. An earlier version of this test re-implemented the suffix strip, so it
/// could not have caught a regression in that rule.
#[test]
fn service_type_name_is_dds_mangled() {
    let response = dds_type_name("example_interfaces::srv", "AddTwoInts_Response");
    assert_eq!(
        response,
        "example_interfaces::srv::dds_::AddTwoInts_Response_"
    );

    let service = service_from_response(&response).expect("a response name yields a service name");
    assert_eq!(service, "example_interfaces::srv::dds_::AddTwoInts_");
}

/// The liveliness token must carry the same mangled name as the topic key.
///
/// Both consume `get_type_info()`, so the fix corrects the token by
/// construction. This test states that rather than leaving it to inference: a
/// token carrying the slash form would make hiroz and `rmw_zenoh_rs` invisible
/// to each other in the graph, even once their topic keys agreed.
#[test]
fn the_liveliness_token_carries_the_wire_type_name() {
    let entity = endpoint(EndpointKind::Publisher, "chatter", WIRE_TYPE_NAME);
    let ke = RmwZenohFormatter::liveliness_key_expr(&entity, &zid()).unwrap();
    let ke = ke.as_str();

    assert!(
        ke.contains(WIRE_TYPE_NAME),
        "the liveliness token does not carry the wire type name: {ke}"
    );
    assert!(
        !ke.contains("std_msgs/msg/String"),
        "the liveliness token still carries the slash form: {ke}"
    );
}

/// A service key expression uses the mangled service name.
///
/// The end-to-end runs covered `std_msgs/String` only, so this pins the service
/// path at the formatter instead of leaving it untested.
#[test]
fn a_service_key_expression_uses_the_mangled_name() {
    let response = dds_type_name("example_interfaces::srv", "AddTwoInts_Response");
    let service = service_from_response(&response).expect("a service name");

    let entity = endpoint(EndpointKind::Service, "add_two_ints", &service);
    let ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();

    assert_eq!(
        ke.as_str(),
        format!("0/add_two_ints/{service}/{}", TypeHash::zero()),
    );
    assert!(service.starts_with("example_interfaces::srv::dds_::"));
}

/// An action's three interfaces use the same mangling as everything else.
///
/// No end-to-end run exercised an action, so this pins the names the code
/// generator bakes in against the shared rule.
#[test]
fn the_action_interfaces_use_the_same_mangling() {
    let ns = "action_tutorials_interfaces::action";
    for (suffix, expected) in [
        ("_SendGoal", "Fibonacci_SendGoal_"),
        ("_GetResult", "Fibonacci_GetResult_"),
        ("_FeedbackMessage", "Fibonacci_FeedbackMessage_"),
    ] {
        let name = dds_type_name(ns, &format!("Fibonacci{suffix}"));
        assert_eq!(name, format!("{ns}::dds_::{expected}"));

        let entity = endpoint(EndpointKind::Publisher, "fibonacci", &name);
        let ke = RmwZenohFormatter::topic_key_expr(&entity).unwrap();
        assert!(
            ke.as_str().contains(&name),
            "action key lost the name: {ke}"
        );
    }
}
