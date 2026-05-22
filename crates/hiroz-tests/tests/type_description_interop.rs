//! Type Description Service interoperability tests.
//!
//! Tests that hiroz can query the GetTypeDescription service from ROS 2 nodes.
//! This validates:
//! - Type hash computation matches ROS 2
//! - Service request/response serialization is compatible
//! - TypeDescription message structure is correct
//! - Dynamic subscription using retrieved type descriptions

#![cfg(feature = "ros-interop")]
#![cfg(not(ros_humble))]

mod common;

use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};

use common::*;
use hiroz::{
    Builder,
    dynamic::{
        DynamicMessage, DynamicSerdeCdrSerdes, MessageSchema, type_description_msg_to_schema,
    },
};
use hiroz_msgs::type_description_interfaces::{
    self, GetTypeDescriptionRequest, srv::GetTypeDescription,
};
use hiroz_schema::{FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg};

/// Convert from wire format TypeDescription to hiroz-schema TypeDescriptionMsg.
///
/// This is needed because the wire format uses generated ROS message types,
/// while the internal schema uses hiroz-schema types.
fn wire_to_schema_type_description(
    wire: &type_description_interfaces::TypeDescription,
) -> TypeDescriptionMsg {
    TypeDescriptionMsg {
        type_description: wire_to_schema_individual(&wire.type_description),
        referenced_type_descriptions: wire
            .referenced_type_descriptions
            .iter()
            .map(wire_to_schema_individual)
            .collect(),
    }
}

fn wire_to_schema_individual(
    wire: &type_description_interfaces::IndividualTypeDescription,
) -> TypeDescription {
    TypeDescription {
        type_name: wire.type_name.clone(),
        fields: wire.fields.iter().map(wire_to_schema_field).collect(),
    }
}

fn wire_to_schema_field(wire: &type_description_interfaces::Field) -> FieldDescription {
    FieldDescription {
        name: wire.name.clone(),
        field_type: FieldTypeDescription {
            type_id: wire.r#type.type_id,
            capacity: wire.r#type.capacity,
            string_capacity: wire.r#type.string_capacity,
            nested_type_name: wire.r#type.nested_type_name.clone(),
        },
        default_value: wire.default_value.clone(),
    }
}

/// Test calling GetTypeDescription service on a ROS 2 talker node.
///
/// This test:
/// 1. Starts a ROS 2 demo_nodes_cpp talker
/// 2. Queries its /talker/get_type_description service
/// 3. Verifies the response contains correct type description for std_msgs/msg/String
#[test]
fn test_get_type_description_from_ros2_talker() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    if !check_demo_nodes_cpp_available() {
        panic!(
            "demo_nodes_cpp package not found!\n\
             Please install it with: apt install ros-$ROS_DISTRO-demo-nodes-cpp\n\
             Or ensure ROS environment is sourced: source /opt/ros/$ROS_DISTRO/setup.bash"
        );
    }

    let router = TestRouter::new();

    println!("\n=== Test: GetTypeDescription from ROS2 talker ===");

    // Start ROS2 talker
    let talker = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "talker"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start ROS2 talker");

    let _guard = ProcessGuard::new(talker, "ros2 talker");

    // Wait for talker to be ready and its services to be available
    wait_for_ready(Duration::from_secs(5));

    // Call GetTypeDescription service from hiroz
    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("type_desc_client")
                .build()
                .expect("Failed to create node");

            // The service name follows ROS 2 convention: /{node_name}/get_type_description
            // NOTE: ROS 2 Jazzy and newer nodes expose ~/get_type_description automatically
            let zcli = node
                .create_client::<GetTypeDescription>("/talker/get_type_description")
                .build()
                .expect("Failed to create client");

            println!("Client ready, waiting for service discovery...");
            // Give more time for ROS 2 node to fully initialize
            tokio::time::sleep(Duration::from_secs(3)).await;

            // Request type description for std_msgs/msg/String
            // Use the type hash that matches our computation
            let req = GetTypeDescriptionRequest {
                type_name: "std_msgs/msg/String".to_string(),
                type_hash:
                    "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18"
                        .to_string(),
                include_type_sources: true,
            };

            let resp = zcli
                .call_with_timeout(&req, Duration::from_secs(15))
                .await
                .expect("Failed to receive response");

            resp
        })
    })
    .join()
    .expect("Client thread panicked");

    // Verify response
    println!("Response received:");
    println!("  successful: {}", result.successful);
    println!("  failure_reason: {}", result.failure_reason);
    println!(
        "  type_description.type_name: {}",
        result.type_description.type_description.type_name
    );
    println!(
        "  type_description.fields count: {}",
        result.type_description.type_description.fields.len()
    );
    println!("  type_sources count: {}", result.type_sources.len());

    assert!(
        result.successful,
        "GetTypeDescription failed: {}",
        result.failure_reason
    );
    assert_eq!(
        result.type_description.type_description.type_name,
        "std_msgs/msg/String"
    );
    assert_eq!(result.type_description.type_description.fields.len(), 1);
    assert_eq!(
        result.type_description.type_description.fields[0].name,
        "data"
    );

    // Verify type_id is STRING (17)
    assert_eq!(
        result.type_description.type_description.fields[0]
            .r#type
            .type_id,
        17,
        "Expected type_id 17 (STRING)"
    );

    println!("Test passed: GetTypeDescription returned correct type info");
}

/// Test GetTypeDescription with valid hash but without requesting sources.
#[test]
fn test_get_type_description_without_sources() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    if !check_demo_nodes_cpp_available() {
        panic!("demo_nodes_cpp package not found!");
    }

    let router = TestRouter::new();

    println!("\n=== Test: GetTypeDescription without hash validation ===");

    // Start ROS2 talker
    let talker = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "talker"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start ROS2 talker");

    let _guard = ProcessGuard::new(talker, "ros2 talker");

    wait_for_ready(Duration::from_secs(5));

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("type_desc_client_no_hash")
                .build()
                .expect("Failed to create node");

            let zcli = node
                .create_client::<GetTypeDescription>("/talker/get_type_description")
                .build()
                .expect("Failed to create client");

            tokio::time::sleep(Duration::from_secs(3)).await;

            // Request with valid hash (ROS 2 Jazzy requires hash to be provided)
            let req = GetTypeDescriptionRequest {
                type_name: "std_msgs/msg/String".to_string(),
                type_hash:
                    "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18"
                        .to_string(),
                include_type_sources: false,
            };

            let resp = zcli
                .call_with_timeout(&req, Duration::from_secs(15))
                .await
                .expect("Failed to receive response");

            resp
        })
    })
    .join()
    .expect("Client thread panicked");

    assert!(
        result.successful,
        "GetTypeDescription failed: {}",
        result.failure_reason
    );
    assert_eq!(
        result.type_description.type_description.type_name,
        "std_msgs/msg/String"
    );

    // Without include_type_sources, should have empty sources
    assert!(
        result.type_sources.is_empty(),
        "Expected no type sources when include_type_sources=false"
    );

    println!("Test passed: GetTypeDescription works without hash validation");
}

/// Test that our computed hash matches what ROS 2 expects.
#[test]
fn test_type_hash_matches_ros2() {
    use hiroz::dynamic::{FieldType, MessageSchema, MessageSchemaTypeDescription};

    println!("\n=== Test: Type hash computation matches ROS 2 ===");

    // Build std_msgs/msg/String schema
    let schema = MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()
        .expect("Failed to build schema");

    let hash = schema.compute_type_hash().expect("Failed to compute hash");
    let rihs = hash.to_rihs_string();

    println!("Computed hash: {}", rihs);

    // This is the hash that ROS 2 Jazzy uses for std_msgs/msg/String
    // Verified by running: ros2 interface show std_msgs/msg/String --show-hash
    assert_eq!(
        rihs, "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18",
        "Hash does not match ROS 2 Jazzy expected value"
    );

    println!("Test passed: Type hash matches ROS 2");
}

/// Test creating a dynamic subscriber using type description from GetTypeDescription service.
///
/// This is the full workflow test:
/// 1. Start a ROS 2 talker that publishes std_msgs/msg/String
/// 2. Query its GetTypeDescription service to get the type schema
/// 3. Convert the response to a MessageSchema
/// 4. Create a dynamic subscriber using that schema
/// 5. Receive and deserialize messages dynamically
///
#[test]
fn test_dynamic_subscriber_from_type_description() {
    if !check_ros2_available() {
        panic!("ros2 CLI not available");
    }

    if !check_demo_nodes_cpp_available() {
        panic!(
            "demo_nodes_cpp package not found!\n\
             Please install it with: apt install ros-$ROS_DISTRO-demo-nodes-cpp\n\
             Or ensure ROS environment is sourced: source /opt/ros/$ROS_DISTRO/setup.bash"
        );
    }

    let router = TestRouter::new();

    println!("\n=== Test: Dynamic subscriber from TypeDescription ===");

    // Start ROS2 talker
    let talker = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "talker"])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", router.rmw_zenoh_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to start ROS2 talker");

    let _guard = ProcessGuard::new(talker, "ros2 talker");

    // Wait for talker to be ready
    wait_for_ready(Duration::from_secs(5));

    let received_messages = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("Failed to create context");

            let node = ctx
                .create_node("dynamic_sub_client")
                .build()
                .expect("Failed to create node");

            // Step 1: Query GetTypeDescription service
            println!("Step 1: Querying GetTypeDescription service...");
            let zcli = node
                .create_client::<GetTypeDescription>("/talker/get_type_description")
                .build()
                .expect("Failed to create client");

            tokio::time::sleep(Duration::from_secs(3)).await;

            let req = GetTypeDescriptionRequest {
                type_name: "std_msgs/msg/String".to_string(),
                type_hash:
                    "RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18"
                        .to_string(),
                include_type_sources: false,
            };

            let resp = zcli
                .call_with_timeout(&req, Duration::from_secs(15))
                .await
                .expect("Failed to receive GetTypeDescription response");

            assert!(
                resp.successful,
                "GetTypeDescription failed: {}",
                resp.failure_reason
            );

            println!(
                "  Got type description for: {}",
                resp.type_description.type_description.type_name
            );

            // Step 2: Convert wire format to hiroz-schema types
            println!("Step 2: Converting to MessageSchema...");
            let type_desc_msg = wire_to_schema_type_description(&resp.type_description);
            let schema: Arc<MessageSchema> = type_description_msg_to_schema(&type_desc_msg)
                .expect("Failed to convert TypeDescription to MessageSchema");

            println!("  Schema type: {}", schema.type_name);
            println!(
                "  Schema fields: {:?}",
                schema.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
            );

            // Step 3: Create dynamic subscriber using the schema
            println!("Step 3: Creating dynamic subscriber...");
            let zsub = node
                .create_sub_impl::<DynamicMessage>("chatter", None)
                .with_serdes::<DynamicSerdeCdrSerdes>()
                .with_dyn_schema(schema.clone())
                .build()
                .expect("Failed to create dynamic subscriber");

            // Step 4: Receive messages
            println!("Step 4: Receiving messages...");
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let target_count = 3;

            while received.len() < target_count && tokio::time::Instant::now() < deadline {
                match zsub.recv_timeout(Duration::from_secs(2)) {
                    Ok(msg) => {
                        let data: String = msg.get("data").expect("Failed to get 'data' field");
                        println!("  Received: {}", data);
                        received.push(data);
                    }
                    Err(e) => {
                        println!("  Waiting for message... ({})", e);
                    }
                }
            }

            received
        })
    })
    .join()
    .expect("Subscriber thread panicked");

    // Verify we received messages
    assert!(
        !received_messages.is_empty(),
        "Expected to receive at least one message"
    );

    // The talker publishes "Hello World: N" messages
    for msg in &received_messages {
        assert!(
            msg.starts_with("Hello World:"),
            "Expected message to start with 'Hello World:', got: {}",
            msg
        );
    }

    println!(
        "Test passed: Received {} messages dynamically",
        received_messages.len()
    );
}
