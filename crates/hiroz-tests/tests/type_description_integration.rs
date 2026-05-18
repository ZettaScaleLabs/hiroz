//! Type Description Integration Tests
//!
//! Tests for the full type description workflow:
//! 1. Publisher with TypeDescriptionService -> Subscriber discovers schema dynamically
//! 2. Dynamic publisher/subscriber communication using discovered schemas

#![cfg(feature = "ros-msgs")]
#![cfg(not(ros_humble))]

mod common;

use std::{thread, time::Duration};

use common::*;
use hiroz::{
    Builder,
    dynamic::{DynamicMessage, FieldType, MessageSchema},
};
use hiroz_msgs::std_msgs::String as RosString;
use serial_test::serial;

/// Test: Static publisher with TypeDescriptionService, dynamic subscriber discovers schema.
///
/// Workflow:
/// 1. Create publisher node with TypeDescriptionService enabled
/// 2. Create static publisher for std_msgs/msg/String
/// 3. Register the schema with the type description service
/// 4. Create subscriber node
/// 5. Use TypeDescriptionClient to discover schema from publisher
/// 6. Create dynamic subscriber with discovered schema
/// 7. Receive and verify messages
#[test]
fn test_static_pub_dynamic_sub_with_type_discovery() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hiroz=debug,rmw_zenoh_rs=debug".parse().unwrap()),
        )
        .try_init();

    let router = TestRouter::new();

    println!("\n=== Test: Static Publisher -> Dynamic Subscriber with Type Discovery ===");

    // Run the test in a separate thread to allow tokio runtime
    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Create publisher context and node with type description service
            let pub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub context");
            let pub_node = pub_ctx
                .create_node("static_talker")
                .with_type_description_service()
                .build()
                .expect("Failed to create publisher node");

            // Create static publisher using hiroz-msgs type
            let publisher = pub_node
                .create_pub::<RosString>("/test_topic")
                .build()
                .expect("Failed to create publisher");

            println!("Publisher created on /test_topic");

            // Verify schema was auto-registered by create_pub::<RosString>()
            if let Some(service) = pub_node.type_description_service() {
                let registered = service
                    .get_schema("std_msgs/msg/String")
                    .expect("Failed to query schema");
                assert!(
                    registered.is_some(),
                    "Schema should be auto-registered for static publisher"
                );
                println!("Schema auto-registration verified");
            } else {
                panic!("TypeDescriptionService not available");
            }

            // Publish continuously until cancelled — schema discovery can be slow
            // under CI load, so we keep messages flowing until the subscriber has
            // received what it needs.
            let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
            let pub_handle = tokio::spawn(async move {
                let mut i = 0u32;
                loop {
                    if cancel_rx.try_recv().is_ok() {
                        break;
                    }
                    let msg = RosString {
                        data: format!("Hello from static publisher: {}", i),
                    };
                    if let Err(e) = publisher.publish(&msg) {
                        eprintln!("Publish error: {}", e);
                    }
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            // Give publisher time to start and be discoverable
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Create subscriber context and node
            let sub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create sub context");
            let sub_node = sub_ctx
                .create_node("dynamic_listener")
                .build()
                .expect("Failed to create subscriber node");

            // Discover schema and create dynamic subscriber
            println!("Discovering schema from publisher...");
            let subscriber = sub_node
                .create_dyn_sub_auto("/test_topic", Duration::from_secs(15))
                .await
                .expect("Failed to create dynamic subscriber with auto-discovery");
            let discovered_schema = subscriber.schema().expect("discovered schema");

            println!("Discovered schema: {}", discovered_schema.type_name);
            assert_eq!(discovered_schema.type_name, "std_msgs/msg/String");
            assert_eq!(discovered_schema.fields.len(), 1);
            assert_eq!(discovered_schema.fields[0].name, "data");

            // Receive messages
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while received.len() < 3 && tokio::time::Instant::now() < deadline {
                match subscriber.recv_timeout(Duration::from_secs(1)) {
                    Ok(msg) => {
                        let data: String = msg.get("data").expect("Failed to get data field");
                        println!("Received: {}", data);
                        received.push(data);
                    }
                    Err(_) => {
                        println!("Waiting for messages...");
                    }
                }
            }

            // Stop publisher now that we have enough messages
            let _ = cancel_tx.send(());
            let _ = pub_handle.await;

            received
        })
    })
    .join()
    .expect("Test thread panicked");

    // Verify results
    assert!(
        !result.is_empty(),
        "Expected to receive at least one message"
    );
    for msg in &result {
        assert!(
            msg.starts_with("Hello from static publisher:"),
            "Unexpected message: {}",
            msg
        );
    }
    println!(
        "Test passed: Received {} messages with discovered schema",
        result.len()
    );
}

/// Test: Dynamic publisher with TypeDescriptionService, dynamic subscriber discovers schema.
///
/// Workflow:
/// 1. Create publisher node with TypeDescriptionService enabled
/// 2. Use create_dyn_pub() to create publisher (auto-registers schema)
/// 3. Publish dynamic messages
/// 4. Create subscriber node
/// 5. Use create_dyn_sub_auto() to discover schema and create subscriber
/// 6. Receive and verify messages
#[test]
fn test_dynamic_pub_dynamic_sub_with_type_discovery() {
    let router = TestRouter::new();

    println!("\n=== Test: Dynamic Publisher -> Dynamic Subscriber with Type Discovery ===");

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Create publisher context and node with type description service
            let pub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub context");
            let pub_node = pub_ctx
                .create_node("dyn_talker")
                .with_type_description_service()
                .build()
                .expect("Failed to create publisher node");

            // Create schema for geometry_msgs/msg/Point
            let schema = MessageSchema::builder("geometry_msgs/msg/Point")
                .field("x", FieldType::Float64)
                .field("y", FieldType::Float64)
                .field("z", FieldType::Float64)
                .build()
                .expect("Failed to build schema");

            // Create dynamic publisher (auto-registers schema)
            let publisher = pub_node
                .create_dyn_pub("/point_topic", schema.clone())
                .build()
                .expect("Failed to create dynamic publisher");

            println!(
                "Dynamic publisher created on /point_topic with schema: {}",
                schema.type_name
            );

            // Verify schema is registered
            if let Some(service) = pub_node.type_description_service() {
                let registered = service
                    .get_schema("geometry_msgs/msg/Point")
                    .expect("Failed to query schema");
                assert!(registered.is_some(), "Schema should be registered");
                println!("Schema registration verified");
            }

            // Start publishing in background - publish continuously to ensure
            // publisher stays alive during schema discovery and message reception
            let pub_schema = schema.clone();
            let pub_handle = tokio::spawn(async move {
                // Publish enough messages to cover schema discovery + message reception time
                // At 100ms per message, 100 messages = 10 seconds, well beyond test duration
                for i in 0..100 {
                    let mut msg = DynamicMessage::new(&pub_schema);
                    msg.set("x", (i as f64) * 1.0).unwrap();
                    msg.set("y", (i as f64) * 2.0).unwrap();
                    msg.set("z", (i as f64) * 3.0).unwrap();

                    if let Err(e) = publisher.publish(&msg) {
                        eprintln!("Publish error: {}", e);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            // Give publisher time to start and advertise
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Create subscriber context and node
            let sub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create sub context");
            let sub_node = sub_ctx
                .create_node("dyn_listener")
                .build()
                .expect("Failed to create subscriber node");

            // Discover schema and create dynamic subscriber
            println!("Discovering schema from dynamic publisher...");
            let subscriber = sub_node
                .create_dyn_sub_auto("/point_topic", Duration::from_secs(15))
                .await
                .expect("Failed to create dynamic subscriber with auto-discovery");
            let discovered_schema = subscriber.schema().expect("discovered schema");

            println!("Discovered schema: {}", discovered_schema.type_name);
            assert_eq!(discovered_schema.type_name, "geometry_msgs/msg/Point");
            assert_eq!(discovered_schema.fields.len(), 3);

            // Receive messages
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while received.len() < 3 && tokio::time::Instant::now() < deadline {
                match subscriber.recv_timeout(Duration::from_secs(1)) {
                    Ok(msg) => {
                        let x: f64 = msg.get("x").expect("Failed to get x");
                        let y: f64 = msg.get("y").expect("Failed to get y");
                        let z: f64 = msg.get("z").expect("Failed to get z");
                        println!("Received Point: ({}, {}, {})", x, y, z);
                        received.push((x, y, z));
                    }
                    Err(_) => {
                        println!("Waiting for messages...");
                    }
                }
            }

            // Abort publisher task now that we've received messages
            pub_handle.abort();

            received
        })
    })
    .join()
    .expect("Test thread panicked");

    // Verify results
    assert!(
        !result.is_empty(),
        "Expected to receive at least one message"
    );
    for (i, (x, y, z)) in result.iter().enumerate() {
        // The values should follow the pattern: x=i*1, y=i*2, z=i*3
        // But we might not receive from index 0
        assert!(
            *y == *x * 2.0 && *z == *x * 3.0,
            "Point {} has unexpected values: ({}, {}, {})",
            i,
            x,
            y,
            z
        );
    }
    println!(
        "Test passed: Received {} Point messages with discovered schema",
        result.len()
    );
}

/// Test: Dynamic publisher, static subscriber (schema known at compile time).
///
/// This tests that dynamic messages are wire-compatible with static messages.
#[test]
fn test_dynamic_pub_static_sub() {
    let router = TestRouter::new();

    println!("\n=== Test: Dynamic Publisher -> Static Subscriber ===");

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Create publisher context and node
            let pub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub context");
            let pub_node = pub_ctx
                .create_node("dyn_pub")
                .build()
                .expect("Failed to create publisher node");

            // Create schema matching std_msgs/msg/String
            let schema = MessageSchema::builder("std_msgs/msg/String")
                .field("data", FieldType::String)
                .build()
                .expect("Failed to build schema");

            // Create dynamic publisher
            let publisher = pub_node
                .create_dyn_pub("/dyn_to_static", schema.clone())
                .build()
                .expect("Failed to create dynamic publisher");

            println!("Dynamic publisher created");

            // Start publishing in background
            let pub_schema = schema.clone();
            let pub_handle = tokio::spawn(async move {
                for i in 0..10 {
                    let mut msg = DynamicMessage::new(&pub_schema);
                    msg.set("data", format!("Dynamic message {}", i)).unwrap();

                    if let Err(e) = publisher.publish(&msg) {
                        eprintln!("Publish error: {}", e);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            // Give publisher time to start
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Create subscriber with static type
            let sub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create sub context");
            let sub_node = sub_ctx
                .create_node("static_sub")
                .build()
                .expect("Failed to create subscriber node");

            let subscriber = sub_node
                .create_sub::<RosString>("/dyn_to_static")
                .build()
                .expect("Failed to create static subscriber");

            println!("Static subscriber created");

            // Receive messages
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while received.len() < 3 && tokio::time::Instant::now() < deadline {
                match subscriber.recv_timeout(Duration::from_secs(1)) {
                    Ok(msg) => {
                        println!("Received (static): {}", msg.data);
                        received.push(msg.data);
                    }
                    Err(_) => {
                        println!("Waiting for messages...");
                    }
                }
            }

            // Wait for publisher to finish
            let _ = pub_handle.await;

            received
        })
    })
    .join()
    .expect("Test thread panicked");

    // Verify results
    assert!(
        !result.is_empty(),
        "Expected to receive at least one message"
    );
    for msg in &result {
        assert!(
            msg.starts_with("Dynamic message"),
            "Unexpected message: {}",
            msg
        );
    }
    println!(
        "Test passed: Static subscriber received {} dynamic messages",
        result.len()
    );
}

/// Test: Static publisher, dynamic subscriber with known schema (no discovery).
#[test]
fn test_static_pub_dynamic_sub_known_schema() {
    let router = TestRouter::new();

    println!("\n=== Test: Static Publisher -> Dynamic Subscriber (Known Schema) ===");

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Create publisher context and node
            let pub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub context");
            let pub_node = pub_ctx
                .create_node("static_pub")
                .build()
                .expect("Failed to create publisher node");

            // Create static publisher
            let publisher = pub_node
                .create_pub::<RosString>("/static_to_dyn")
                .build()
                .expect("Failed to create publisher");

            println!("Static publisher created");

            // Create subscriber before publishing so no messages are missed.
            let sub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create sub context");
            let sub_node = sub_ctx
                .create_node("dyn_sub")
                .build()
                .expect("Failed to create subscriber node");

            // Build schema manually (known at development time)
            let schema = MessageSchema::builder("std_msgs/msg/String")
                .field("data", FieldType::String)
                .build()
                .expect("Failed to build schema");

            let subscriber = sub_node
                .create_dyn_sub("/static_to_dyn", schema)
                .build()
                .expect("Failed to create dynamic subscriber");

            println!("Dynamic subscriber created with known schema");

            // Wait until the subscriber has matched the publisher at the middleware
            // level before starting to publish — eliminates the race where all
            // messages are sent before the subscriber is ready.
            publisher
                .wait_for_subscription(1, Duration::from_secs(10))
                .await;

            // Publish in the background so we can receive concurrently.
            let pub_handle = tokio::task::spawn(async move {
                for i in 0..10 {
                    let msg = RosString {
                        data: format!("Static message {}", i),
                    };
                    publisher.publish(&msg).expect("Failed to publish");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            // Receive messages
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while received.len() < 3 && tokio::time::Instant::now() < deadline {
                match subscriber.recv_timeout(Duration::from_secs(1)) {
                    Ok(msg) => {
                        let data: String = msg.get("data").expect("Failed to get data");
                        println!("Received (dynamic): {}", data);
                        received.push(data);
                    }
                    Err(_) => {
                        println!("Waiting for messages...");
                    }
                }
            }

            // Wait for publisher to finish
            let _ = pub_handle.await;

            received
        })
    })
    .join()
    .expect("Test thread panicked");

    // Verify results
    assert!(
        !result.is_empty(),
        "Expected to receive at least one message"
    );
    for msg in &result {
        assert!(
            msg.starts_with("Static message"),
            "Unexpected message: {}",
            msg
        );
    }
    println!(
        "Test passed: Dynamic subscriber received {} static messages",
        result.len()
    );
}

/// Test: Multiple publishers, subscriber discovers from any.
#[test]
#[serial]
fn test_multiple_publishers_schema_discovery() {
    let router = TestRouter::new();

    println!("\n=== Test: Multiple Publishers, Subscriber Discovers Schema ===");

    let result = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Create first publisher with type description service
            let pub1_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub1 context");
            let pub1_node = pub1_ctx
                .create_node("talker1")
                .with_type_description_service()
                .build()
                .expect("Failed to create publisher 1 node");

            let schema = MessageSchema::builder("std_msgs/msg/String")
                .field("data", FieldType::String)
                .build()
                .expect("Failed to build schema");

            let publisher1 = pub1_node
                .create_dyn_pub("/multi_pub_topic", schema.clone())
                .build()
                .expect("Failed to create publisher 1");

            // Create second publisher with type description service
            let pub2_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create pub2 context");
            let pub2_node = pub2_ctx
                .create_node("talker2")
                .with_type_description_service()
                .build()
                .expect("Failed to create publisher 2 node");

            let publisher2 = pub2_node
                .create_dyn_pub("/multi_pub_topic", schema.clone())
                .build()
                .expect("Failed to create publisher 2");

            println!("Two publishers created on /multi_pub_topic");

            // Create subscriber BEFORE starting to publish.  Publishers are
            // already in the graph (announced at creation), so discovery works.
            // Deferring the publish loops ensures messages arrive while the
            // subscriber is actually ready to receive them.
            let sub_ctx =
                create_hiroz_context_with_router(&router).expect("Failed to create sub context");
            let sub_node = sub_ctx
                .create_node("listener")
                .build()
                .expect("Failed to create subscriber node");

            println!("Discovering schema from publishers...");
            let subscriber = sub_node
                .create_dyn_sub_auto("/multi_pub_topic", Duration::from_secs(15))
                .await
                .expect("Failed to create dynamic subscriber with auto-discovery");
            let discovered_schema = subscriber.schema().expect("discovered schema");

            println!("Discovered schema: {}", discovered_schema.type_name);

            // Wait until pub1 and pub2 each see the subscriber before publishing.
            // This is the hiroz equivalent of rclcpp's rcl_wait_for_subscribers().
            let matched1 = publisher1
                .wait_for_subscription(1, Duration::from_secs(10))
                .await;
            let matched2 = publisher2
                .wait_for_subscription(1, Duration::from_secs(10))
                .await;
            assert!(matched1, "pub1 timed out waiting for subscriber");
            assert!(matched2, "pub2 timed out waiting for subscriber");
            println!("Both publishers matched subscriber — starting to publish");

            // Now start both publisher tasks.
            let schema1 = schema.clone();
            let schema2 = schema.clone();

            let pub1_handle = tokio::spawn(async move {
                for i in 0..5 {
                    let mut msg = DynamicMessage::new(&schema1);
                    msg.set("data", format!("From pub1: {}", i)).unwrap();
                    let _ = publisher1.publish(&msg);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            });

            let pub2_handle = tokio::spawn(async move {
                for i in 0..5 {
                    let mut msg = DynamicMessage::new(&schema2);
                    msg.set("data", format!("From pub2: {}", i)).unwrap();
                    let _ = publisher2.publish(&msg);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            });

            // Receive messages from both publishers
            let mut received = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while received.len() < 5 && tokio::time::Instant::now() < deadline {
                match subscriber.recv_timeout(Duration::from_millis(500)) {
                    Ok(msg) => {
                        let data: String = msg.get("data").expect("Failed to get data");
                        println!("Received: {}", data);
                        received.push(data);
                    }
                    Err(_) => {}
                }
            }

            let _ = pub1_handle.await;
            let _ = pub2_handle.await;

            received
        })
    })
    .join()
    .expect("Test thread panicked");

    // Verify we received messages from at least one publisher
    assert!(
        !result.is_empty(),
        "Expected to receive at least one message"
    );

    // Check that we received from at least one of the publishers
    let from_pub1 = result.iter().any(|m| m.starts_with("From pub1:"));
    let from_pub2 = result.iter().any(|m| m.starts_with("From pub2:"));
    println!(
        "Received from pub1: {}, from pub2: {}",
        from_pub1, from_pub2
    );

    // At least one should have worked
    assert!(
        from_pub1 || from_pub2,
        "Should have received from at least one publisher"
    );
    println!(
        "Test passed: Received {} messages from multiple publishers",
        result.len()
    );
}
