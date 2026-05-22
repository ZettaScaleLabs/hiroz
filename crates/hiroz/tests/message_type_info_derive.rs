use std::time::Duration;

use hiroz::{
    Builder, MessageTypeInfo, TypeHash,
    context::ZContextBuilder,
    dynamic::{FieldType, MessageSchemaTypeDescription},
};
use serde::{Deserialize, Serialize};
use zenoh::Wait;
use zenoh::config::WhatAmI;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, hiroz::MessageTypeInfo)]
#[ros_msg(type_name = "custom_msgs/msg/Position2D")]
struct Position2D {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, hiroz::MessageTypeInfo)]
#[ros_msg(type_name = "custom_msgs/msg/RobotTelemetry")]
struct RobotTelemetry {
    label: String,
    pose: Position2D,
    temperatures: Vec<f32>,
    flags: [bool; 2],
    payload: Vec<u8>,
}

impl hiroz::msg::ZMessage for RobotTelemetry {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Self>;
}

struct TestRouter {
    endpoint: String,
    _session: zenoh::Session,
}

impl TestRouter {
    fn new() -> Self {
        let port = {
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("failed to bind port 0");
            listener.local_addr().unwrap().port()
        };

        let endpoint = format!("tcp/127.0.0.1:{port}");
        let mut config = zenoh::Config::default();
        config.set_mode(Some(WhatAmI::Router)).unwrap();
        config
            .insert_json5("listen/endpoints", &format!("[\"{endpoint}\"]"))
            .unwrap();
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();

        let session = zenoh::open(config)
            .wait()
            .expect("failed to open test router");
        std::thread::sleep(Duration::from_millis(300));

        Self {
            endpoint,
            _session: session,
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

fn create_context_with_router(router: &TestRouter) -> hiroz::Result<hiroz::context::ZContext> {
    ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .build()
}

#[test]
fn derive_generates_type_info_and_schema() {
    let schema = RobotTelemetry::message_schema().expect("schema should be generated");

    assert_eq!(
        RobotTelemetry::type_name(),
        "custom_msgs::msg::dds_::RobotTelemetry_"
    );
    assert_eq!(schema.type_name, "custom_msgs/msg/RobotTelemetry");
    assert_eq!(schema.field_count(), 5);

    let label = schema.field("label").expect("label field");
    assert!(matches!(label.field_type, FieldType::String));

    let pose = schema.field("pose").expect("pose field");
    match &pose.field_type {
        FieldType::Message(nested) => {
            assert_eq!(nested.type_name, "custom_msgs/msg/Position2D");
            assert_eq!(nested.field_count(), 2);
        }
        other => panic!("expected nested message field, got {:?}", other),
    }

    let temperatures = schema.field("temperatures").expect("temperatures field");
    match &temperatures.field_type {
        FieldType::Sequence(inner) => {
            assert!(matches!(inner.as_ref(), FieldType::Float32));
        }
        other => panic!("expected sequence field, got {:?}", other),
    }

    let flags = schema.field("flags").expect("flags field");
    match &flags.field_type {
        FieldType::Array(inner, len) => {
            assert_eq!(*len, 2);
            assert!(matches!(inner.as_ref(), FieldType::Bool));
        }
        other => panic!("expected fixed array field, got {:?}", other),
    }

    let payload = schema.field("payload").expect("payload field");
    match &payload.field_type {
        FieldType::Sequence(inner) => {
            assert!(matches!(inner.as_ref(), FieldType::Uint8));
        }
        other => panic!("expected byte sequence field, got {:?}", other),
    }

    if TypeHash::is_supported() {
        let expected_rihs = schema
            .compute_type_hash()
            .expect("schema hash")
            .to_rihs_string();
        assert_eq!(RobotTelemetry::type_hash().to_rihs_string(), expected_rihs);
        assert_eq!(
            schema.type_hash.as_deref(),
            Some(expected_rihs.as_str()),
            "message_schema() must carry the computed type_hash"
        );
    } else {
        assert_eq!(RobotTelemetry::type_hash(), TypeHash::zero());
        assert!(schema.type_hash.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn derived_message_schema_is_auto_registered_and_discoverable() {
    let router = TestRouter::new();

    let pub_ctx = create_context_with_router(&router).expect("publisher context");
    let pub_node = pub_ctx
        .create_node("derived_talker")
        .with_type_description_service()
        .build()
        .expect("publisher node");

    let publisher = pub_node
        .create_pub::<RobotTelemetry>("/derived_topic")
        .build()
        .expect("publisher");

    let registered = pub_node
        .type_description_service()
        .expect("type description service")
        .get_schema("custom_msgs/msg/RobotTelemetry")
        .expect("query registered schema");
    assert!(registered.is_some(), "schema should be auto-registered");

    let sub_ctx = create_context_with_router(&router).expect("subscriber context");
    let sub_node = sub_ctx
        .create_node("derived_listener")
        .build()
        .expect("subscriber node");

    let publish_task = tokio::spawn(async move {
        for _ in 0..25 {
            let msg = RobotTelemetry {
                label: "robot-1".to_string(),
                pose: Position2D { x: 1.25, y: -2.5 },
                temperatures: vec![20.5, 21.0, 21.5],
                flags: [true, false],
                payload: vec![1, 2, 3, 4],
            };
            publisher.publish(&msg).expect("publish");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let subscriber = sub_node
        .create_dyn_sub_auto("/derived_topic", Duration::from_secs(10))
        .await
        .expect("dynamic subscriber with auto-discovery");
    let discovered_schema = subscriber.schema().expect("discovered schema");

    assert_eq!(
        discovered_schema.type_name,
        "custom_msgs/msg/RobotTelemetry"
    );
    assert_eq!(discovered_schema.field_count(), 5);

    let msg = subscriber
        .recv_timeout(Duration::from_secs(3))
        .expect("received dynamic message");
    assert_eq!(
        msg.get::<String>("label").expect("label field"),
        "robot-1".to_string()
    );
    assert_eq!(msg.get::<f64>("pose.x").expect("nested pose.x"), 1.25);
    assert_eq!(msg.get::<f64>("pose.y").expect("nested pose.y"), -2.5);

    publish_task.await.expect("publisher task");
}
