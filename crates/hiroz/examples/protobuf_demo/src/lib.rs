//! Protobuf demo module
//!
//! This module demonstrates using protobuf messages with hiroz for both
//! pub/sub and service communication.
//!
//! Re-exports functions from example modules, allowing tests to import
//! and use the exact same code that users run.

use std::time::Duration;

use hiroz::{Builder, Result, context::ZContext, msg::ProtobufSerdes};
use hiroz_msgs::proto::geometry_msgs::Vector3 as Vector3Proto;

pub mod types;

// Re-export types for tests
pub use types::{Calculate, CalculateRequest, CalculateResponse, SensorData};

/// Run protobuf pub/sub demo
///
/// Demonstrates two approaches:
/// 1. ROS messages with protobuf serialization (Vector3Proto)
/// 2. Custom protobuf messages (SensorData)
pub fn run_pubsub_demo(ctx: ZContext, max_count: Option<usize>) -> Result<()> {
    let node = ctx.create_node("protobuf_pubsub_demo").build()?;

    println!("\n=== Protobuf Serialization Demo ===");
    println!("This demonstrates two ways to use protobuf with hiroz:");
    println!("1. ROS messages with protobuf serialization (from hiroz-msgs)");
    println!("2. Custom protobuf messages (from .proto files)");
    println!("=====================================================\n");

    // Part 1: ROS message with protobuf serialization
    println!("--- Part 1: ROS geometry_msgs/Vector3 with Protobuf ---");
    let ros_pub = node
        .create_pub::<Vector3Proto>("/vector_proto")
        .with_serdes::<ProtobufSerdes<Vector3Proto>>()
        .build()?;

    println!("Publishing ROS Vector3 messages...\n");
    let count = max_count.unwrap_or(3);
    for i in 0..count {
        let msg = Vector3Proto {
            x: i as f64,
            y: (i as f64) * 2.0,
            z: (i as f64) * 3.0,
        };

        ros_pub.publish(&msg)?;
        println!("  Published Vector3: x={}, y={}, z={}", msg.x, msg.y, msg.z);
        std::thread::sleep(Duration::from_millis(500));
    }

    // Part 2: Custom protobuf message
    println!("\n--- Part 2: Custom SensorData message (pure protobuf) ---");
    let custom_pub = node
        .create_pub::<SensorData>("/sensor_data")
        .with_serdes::<ProtobufSerdes<SensorData>>()
        .build()?;

    println!("Publishing custom SensorData messages...\n");
    for i in 0..count {
        let msg = SensorData {
            sensor_id: format!("sensor_{}", i),
            temperature: 20.0 + (i as f64) * 0.5,
            humidity: 45.0 + (i as f64) * 2.0,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };

        custom_pub.publish(&msg)?;
        println!(
            "  Published SensorData: id={}, temp={:.1}°C, humidity={:.1}%, ts={}",
            msg.sensor_id, msg.temperature, msg.humidity, msg.timestamp
        );
        std::thread::sleep(Duration::from_millis(500));
    }

    println!("\nSuccessfully demonstrated both protobuf approaches!");
    println!("\nKey points:");
    println!("1. ROS messages (Vector3Proto): Auto-generated from hiroz-msgs with MessageTypeInfo");
    println!("2. Custom messages (SensorData): Generated from your own .proto files");
    println!("3. Both use .with_serdes::<ProtobufSerdes<T>>() for protobuf serialization");
    println!("4. hiroz is transport-agnostic - works with ANY protobuf message!");

    Ok(())
}

/// Run the calculator service client
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `service_name` - Name of the service to call
/// * `operations` - List of (operation, a, b) tuples to execute
pub fn run_service_client(
    ctx: ZContext,
    service_name: &str,
    operations: Vec<(&str, f64, f64)>,
) -> Result<()> {
    let node = ctx.create_node("calculator_client").build()?;

    let client = node.create_client::<Calculate>(service_name).build()?;

    println!("Calculator service client started");
    println!("Calling service '{}'...\n", service_name);

    // Give server time to start if needed
    std::thread::sleep(Duration::from_millis(500));

    // Create runtime once for all requests
    let rt = tokio::runtime::Runtime::new().unwrap();

    for (op, a, b) in operations {
        println!("[Client] Sending: {} {} {}", a, op, b);

        let request = CalculateRequest {
            a,
            b,
            operation: op.to_string(),
        };

        // Send request and wait for response using the shared runtime
        let response = rt.block_on(async {
            client
                .call_with_timeout(&request, Duration::from_secs(5))
                .await
        })?;

        if response.success {
            println!("[Client] ✓ Success: {}", response.message);
        } else {
            println!("[Client] ✗ Failed: {}", response.message);
        }
        println!();

        std::thread::sleep(Duration::from_millis(200));
    }

    println!("Successfully demonstrated protobuf service calls!");
    println!("\nKey points:");
    println!("1. Request/Response: Protobuf messages generated from .proto files");
    println!("2. Both implement MessageTypeInfo, WithTypeInfo, and ZMessage traits");
    println!("3. Service type implements ZService and ServiceTypeInfo");
    println!("4. Works seamlessly with node.create_service() and node.create_client()");
    println!("5. Uses ProtobufSerdes for pure protobuf serialization (not CDR!)");

    Ok(())
}

/// Run the calculator service server
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `service_name` - Name of the service
/// * `max_requests` - Optional maximum number of requests to handle. If None, handles indefinitely.
pub fn run_service_server(
    ctx: ZContext,
    service_name: &str,
    max_requests: Option<usize>,
) -> Result<()> {
    let node = ctx.create_node("calculator_server").build()?;

    let mut server = node.create_service::<Calculate>(service_name).build()?;

    println!("Calculator service server started on '{}'", service_name);
    println!("Waiting for requests...\n");

    let mut count = 0;
    let mut consecutive_errors = 0;
    const MAX_CONSECUTIVE_ERRORS: u32 = 10;

    loop {
        match server.take_request() {
            Ok(request) => {
                consecutive_errors = 0; // Reset error counter on success
                count += 1;
                println!(
                    "[Server] Request {}: {} {} {}",
                    count,
                    request.message().a,
                    request.message().operation,
                    request.message().b
                );

                let response = match request.message().operation.as_str() {
                    "add" => types::CalculateResponse {
                        success: true,
                        result: request.message().a + request.message().b,
                        message: format!(
                            "{} + {} = {}",
                            request.message().a,
                            request.message().b,
                            request.message().a + request.message().b
                        ),
                    },
                    "subtract" => types::CalculateResponse {
                        success: true,
                        result: request.message().a - request.message().b,
                        message: format!(
                            "{} - {} = {}",
                            request.message().a,
                            request.message().b,
                            request.message().a - request.message().b
                        ),
                    },
                    "multiply" => types::CalculateResponse {
                        success: true,
                        result: request.message().a * request.message().b,
                        message: format!(
                            "{} * {} = {}",
                            request.message().a,
                            request.message().b,
                            request.message().a * request.message().b
                        ),
                    },
                    "divide" => {
                        if request.message().b == 0.0 {
                            types::CalculateResponse {
                                success: false,
                                result: 0.0,
                                message: "Error: Division by zero".to_string(),
                            }
                        } else {
                            types::CalculateResponse {
                                success: true,
                                result: request.message().a / request.message().b,
                                message: format!(
                                    "{} / {} = {}",
                                    request.message().a,
                                    request.message().b,
                                    request.message().a / request.message().b
                                ),
                            }
                        }
                    }
                    _ => types::CalculateResponse {
                        success: false,
                        result: 0.0,
                        message: format!(
                            "Error: Unknown operation '{}'",
                            request.message().operation
                        ),
                    },
                };

                if let Err(e) = request.reply_blocking(&response) {
                    eprintln!("[Server] Failed to send response: {}", e);
                    consecutive_errors += 1;
                }

                // Check if we've reached max_requests
                if let Some(max) = max_requests
                    && count >= max
                {
                    println!("[Server] Processed {} requests, exiting", count);
                    break;
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                eprintln!(
                    "[Server] Error taking request ({} consecutive): {}",
                    consecutive_errors, e
                );

                // If we have too many consecutive errors, exit to avoid infinite loops
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    eprintln!(
                        "[Server] Too many consecutive errors ({}), exiting",
                        consecutive_errors
                    );
                    break;
                }

                // Sleep briefly before retrying, but with exponential backoff
                let sleep_ms = 100 * (consecutive_errors as u64).min(10);
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        }
    }

    Ok(())
}
