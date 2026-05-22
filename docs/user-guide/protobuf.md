<!-- markdownlint-disable MD046 -->
# Protobuf Serialization

**Use Protocol Buffers as an alternative serialization format for hiroz messages.** While CDR is the default ROS 2-compatible format, protobuf offers schema evolution, cross-language compatibility, and familiar tooling for teams already using the protobuf ecosystem.

!!! info
    Protobuf support in hiroz enables two powerful use cases:
    1. **ROS messages with protobuf encoding** - Use standard ROS message types serialized via protobuf
    2. **Pure protobuf messages** - Send custom `.proto` messages directly through hiroz

## When to Use Protobuf

| Use Case | Recommendation |
|----------|----------------|
| **ROS 2 interoperability** | Use CDR (default) |
| **Schema evolution** | Use Protobuf |
| **Cross-language data exchange** | Use Protobuf |
| **Existing protobuf infrastructure** | Use Protobuf |
| **Performance critical** | Benchmark both (typically similar) |

!!! warning
    Protobuf-serialized messages are **not** compatible with standard ROS 2 nodes using CDR. Use protobuf when you control both ends of the communication or need its specific features.

## Enabling Protobuf Support

### Feature Flags

Enable protobuf in your `Cargo.toml`:

```toml
[dependencies]
hiroz = { version = "0.1", features = ["protobuf"] }
hiroz-msgs = { version = "0.1", features = ["geometry_msgs", "protobuf"] }
prost = "0.13"

[build-dependencies]
prost-build = "0.13"
```

### Build Configuration

For custom `.proto` files, add a `build.rs`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = prost_build::Config::new();
    // Enable serde support for hiroz compatibility
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");
    config.compile_protos(&["proto/sensor_data.proto"], &["proto/"])?;
    println!("cargo:rerun-if-changed=proto/sensor_data.proto");
    Ok(())
}
```

## Approach 1: ROS Messages with Protobuf

Use auto-generated ROS message types with protobuf serialization:

```rust
use hiroz::msg::ProtobufSerdes;
use hiroz_msgs::proto::geometry_msgs::Vector3 as Vector3Proto;

let ctx = hiroz::context::ZContextBuilder::default().build()?;
let node = ctx.create_node("protobuf_node").build()?;

// Create publisher with protobuf serialization
let pub = node
    .create_pub::<Vector3Proto>("/vector_proto")
    .with_serdes::<ProtobufSerdes<Vector3Proto>>()
    .build()?;

// Publish messages
let msg = Vector3Proto {
    x: 1.0,
    y: 2.0,
    z: 3.0,
};
pub.publish(&msg)?;
```

**Key points:**

- Import from `hiroz_msgs::proto::*` namespace (not `hiroz_msgs::ros::*`)
- Use `.with_serdes::<ProtobufSerdes<T>>()` to select protobuf encoding
- Message types automatically implement `MessageTypeInfo` trait
- Full type safety and compile-time checking

## Approach 2: Custom Protobuf Messages

Send arbitrary protobuf messages defined in `.proto` files:

### Step 1: Define Your Message

Create `proto/sensor_data.proto`:

```protobuf
syntax = "proto3";

package examples;

message SensorData {
    string sensor_id = 1;
    double temperature = 2;
    double humidity = 3;
    int64 timestamp = 4;
}
```

### Step 2: Generate Rust Code

Configure `build.rs` as shown above. The build script generates Rust structs at compile time.

### Step 3: Include Generated Code

```rust
pub mod sensor_data {
    include!(concat!(env!("OUT_DIR"), "/examples.rs"));
}

use sensor_data::SensorData;
```

### Step 4: Implement Required Traits

```rust
use hiroz::{MessageTypeInfo, WithTypeInfo, entity::TypeHash};

impl MessageTypeInfo for SensorData {
    fn type_name() -> &'static str {
        "examples::msg::dds_::SensorData_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()  // For custom protobuf messages
    }
}

impl WithTypeInfo for SensorData {}
```

### Step 5: Use in hiroz

```rust
let pub = node
    .create_pub::<SensorData>("/sensor_data")
    .with_serdes::<ProtobufSerdes<SensorData>>()
    .build()?;

let msg = SensorData {
    sensor_id: "sensor_01".to_string(),
    temperature: 23.5,
    humidity: 45.0,
    timestamp: 1234567890,
};

pub.publish(&msg)?;
```

## Complete Example

The `protobuf_demo` example demonstrates both approaches:

```rust
use clap::Parser;
use protobuf_demo::{run_pubsub_demo, run_service_client, run_service_server};
use hiroz::{Builder, Result, context::ZContextBuilder};

#[derive(Debug, Parser)]
#[command(
    name = "protobuf_demo",
    about = "Protobuf demonstration for hiroz - pub/sub and services"
)]
struct Args {
    /// Mode to run: pubsub, service-server, service-client, or combined
    #[arg(short, long, default_value = "pubsub")]
    mode: String,

    /// Service name (for service modes)
    #[arg(long, default_value = "/calculator")]
    service: String,

    /// Maximum number of messages/requests (0 for unlimited)
    #[arg(short = 'n', long, default_value = "3")]
    count: usize,

    /// Eclipse Zenoh session mode (peer, client, router)
    #[arg(long, default_value = "peer")]
    zenoh_mode: String,

    /// Zenoh router endpoint to connect to
    #[arg(short, long)]
    endpoint: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    zenoh::init_log_from_env_or("info");

    // Create the hiroz context
    let ctx = if let Some(ref e) = args.endpoint {
        ZContextBuilder::default()
            .with_mode(&args.zenoh_mode)
            .with_connect_endpoints([e.clone()])
            .build()?
    } else {
        ZContextBuilder::default()
            .with_mode(&args.zenoh_mode)
            .build()?
    };

    let max_count = if args.count == 0 {
        None
    } else {
        Some(args.count)
    };

    match args.mode.as_str() {
        "pubsub" => {
            run_pubsub_demo(ctx, max_count)?;
        }
        "service-server" => {
            run_service_server(ctx, &args.service, max_count)?;
        }
        "service-client" => {
            let operations = vec![
                ("add", 10.0, 5.0),
                ("subtract", 10.0, 5.0),
                ("multiply", 10.0, 5.0),
                ("divide", 10.0, 5.0),
                ("divide", 10.0, 0.0), // This will fail
            ];
            run_service_client(ctx, &args.service, operations)?;
        }
        "combined" => {
            println!("\n=== Running Combined Demo ===\n");

            // Create separate contexts for server and client
            let server_ctx = if let Some(e) = args.endpoint.clone() {
                ZContextBuilder::default()
                    .with_mode(&args.zenoh_mode)
                    .with_connect_endpoints([e])
                    .build()?
            } else {
                ZContextBuilder::default()
                    .with_mode(&args.zenoh_mode)
                    .build()?
            };

            let service_name = args.service.clone();

            // Run server in background thread
            let _server_handle =
                std::thread::spawn(move || run_service_server(server_ctx, &service_name, Some(5)));

            // Give server time to start
            std::thread::sleep(std::time::Duration::from_millis(500));

            // Run client
            let operations = vec![
                ("add", 10.0, 5.0),
                ("subtract", 10.0, 5.0),
                ("multiply", 10.0, 5.0),
                ("divide", 10.0, 5.0),
                ("divide", 10.0, 0.0),
            ];
            run_service_client(ctx, &args.service, operations)?;
        }
        _ => {
            eprintln!("Unknown mode: {}", args.mode);
            eprintln!("Valid modes: pubsub, service-server, service-client, combined");
            std::process::exit(1);
        }
    }

    Ok(())
}
```

## Subscribers with Protobuf

Receive protobuf-encoded messages:

```rust
use hiroz::msg::ProtobufSerdes;

let sub = node
    .create_sub::<Vector3Proto>("/vector_proto")
    .with_serdes::<ProtobufSerdes<Vector3Proto>>()
    .build()?;

loop {
    let msg = sub.recv()?;
    println!("Received: x={}, y={}, z={}", msg.x, msg.y, msg.z);
}
```

!!! important
    Publishers and subscribers must use the **same serialization format**. A protobuf publisher requires a protobuf subscriber.

## Services with Protobuf

Both request and response use protobuf encoding:

### Server

```rust
let mut service = node
    .create_service::<MyService>("/my_service")
    .with_serdes::<ProtobufSerdes<MyServiceRequest>, ProtobufSerdes<MyServiceResponse>>()
    .build()?;

loop {
    let req = service.take_request()?;
    let response = process_request(req.message());
    req.reply_blocking(&response)?;
}
```

### Client

```rust
let client = node
    .create_client::<MyService>("/my_service")
    .with_serdes::<ProtobufSerdes<MyServiceRequest>, ProtobufSerdes<MyServiceResponse>>()
    .build()?;

let response = client.call_with_timeout(&request, Duration::from_secs(5)).await?;
```

## Available ROS Messages

When hiroz-msgs is built with `protobuf` feature, it generates protobuf versions of ROS messages:

```rust
// Import from proto namespace
use hiroz_msgs::proto::std_msgs::String as StringProto;
use hiroz_msgs::proto::geometry_msgs::{Point, Pose, Twist};
use hiroz_msgs::proto::sensor_msgs::{LaserScan, Image};
```

**Namespace mapping:**

| Format | Namespace | Use |
|--------|-----------|-----|
| **CDR** | `hiroz_msgs::ros::*` | ROS 2 interop |
| **Protobuf** | `hiroz_msgs::proto::*` | Protobuf encoding |

## Type Information

### ROS Messages

Auto-generated messages from hiroz-msgs include `MessageTypeInfo`:

```rust
// No manual implementation needed
use hiroz_msgs::proto::geometry_msgs::Vector3;
// Vector3 already implements MessageTypeInfo
```

### Custom Protobuf Messages

Manual implementation required:

```rust
impl MessageTypeInfo for MyProtoMessage {
    fn type_name() -> &'static str {
        // Follow ROS naming convention
        "my_package::msg::dds_::MyProtoMessage_"
    }



fn type_hash() -> TypeHash {
    // Use zero for custom protobuf messages
    TypeHash::zero()
}
}

impl WithTypeInfo for MyProtoMessage {}
```

!!! note
    `TypeHash::zero()` indicates the message doesn't have ROS 2 type compatibility. This is fine for hiroz-to-hiroz communication.

## Protobuf vs CDR Comparison

| Aspect | CDR | Protobuf |
|--------|-----|----------|
| **ROS 2 Compatibility** | ✅ Full | ❌ None |
| **Schema Evolution** | ❌ Limited | ✅ Excellent |
| **Cross-language** | ROS 2 only | ✅ Universal |
| **Tooling** | ROS ecosystem | ✅ Protobuf ecosystem |
| **Message Size** | Efficient | Efficient |
| **Setup Complexity** | Simple | Moderate |
| **hiroz Support** | Default | Requires feature flag |

## Common Patterns

### Mixed Serialization

Different topics can use different formats:

```rust
// CDR for ROS 2 compatibility
let ros_pub = node
    .create_pub::<RosString>("/ros_topic")
    .build()?;  // CDR is default

// Protobuf for schema evolution
let proto_pub = node
    .create_pub::<ProtoString>("/proto_topic")
    .with_serdes::<ProtobufSerdes<ProtoString>>()
    .build()?;
```

### Migration Strategy

Gradual migration from CDR to protobuf:

1. Add protobuf feature to dependencies
2. Create protobuf topics with new names
3. Run both CDR and protobuf publishers temporarily
4. Migrate subscribers to protobuf
5. Deprecate CDR topics

## Build Integration

### Project Structure

```text
my_ros_project/
├── proto/
│   ├── sensor_data.proto
│   └── robot_status.proto
├── src/
│   └── main.rs
├── build.rs
└── Cargo.toml
```

### Cargo.toml

```toml
[package]
name = "my_ros_project"
version = "0.1.0"
edition = "2021"

[dependencies]
hiroz = { version = "0.1", features = ["protobuf"] }
hiroz-msgs = { version = "0.1", features = ["geometry_msgs", "protobuf"] }
prost = "0.13"
serde = { version = "1.0", features = ["derive"] }

[build-dependencies]
prost-build = "0.13"
```

### build.rs

```rust
use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();

    // Enable serde for hiroz compatibility
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");

    // Compile all proto files
    config.compile_protos(
        &[
            "proto/sensor_data.proto",
            "proto/robot_status.proto",
        ],
        &["proto/"]
    )?;

    // Rebuild if proto files change
    println!("cargo:rerun-if-changed=proto/");

    Ok(())
}
```

## Troubleshooting

??? question "Error: protobuf feature not enabled"
    This error occurs when you try to use protobuf serialization without enabling the feature flag.
    **Solution:** Enable the protobuf feature in your `Cargo.toml`:

    ```toml
    [dependencies]
    hiroz = { version = "0.1", features = ["protobuf"] }
    hiroz-msgs = { version = "0.1", features = ["geometry_msgs", "protobuf"] }

    ```

??? question "Error: MessageTypeInfo not implemented"
    Custom protobuf messages need to implement required hiroz traits.
    **Solution:** Implement the required traits for your custom message:

    ```rust
    use hiroz::{MessageTypeInfo, WithTypeInfo, entity::TypeHash};

    impl MessageTypeInfo for MyMessage {
        fn type_name() -> &'static str {
            "package::msg::dds_::MyMessage_"
        }

        fn type_hash() -> TypeHash {
            TypeHash::zero()
        }
    }

    impl WithTypeInfo for MyMessage {}

    ```

??? question "Build fails with prost errors"
    Version mismatches between prost dependencies can cause build failures.
    **Solution:** Ensure prost versions match in your `Cargo.toml`:

    ```toml
    [dependencies]
    prost = "0.13"

    [build-dependencies]
    prost-build = "0.13"

    ```

    If issues persist, try:

    ```bash
    cargo clean
    cargo build

    ```

??? question "Messages not receiving"
    Publisher and subscriber must use the same serialization format.
    **Solution:** Verify both sides use protobuf serialization:

    ```rust
    // Publisher
    let pub_handle = node
        .create_pub::<MyMessage>("/topic")
        .with_serdes::<ProtobufSerdes<MyMessage>>()
        .build()?;

    // Subscriber
    let sub = node
        .create_sub::<MyMessage>("/topic")
        .with_serdes::<ProtobufSerdes<MyMessage>>()
        .build()?;

    ```

    **Note:** A protobuf publisher cannot communicate with a CDR subscriber and vice versa.

??? question "Proto file not found during build"
    The build script cannot locate your `.proto` files.
    **Solution:** Verify the path in your `build.rs`:

    ```rust
    config.compile_protos(
        &["proto/sensor_data.proto"],  // Check this path
        &["proto/"]                     // Check include directory
    )?;

    ```

    Ensure the proto directory exists:

    ```bash
    ls proto/sensor_data.proto

    ```

??? question "Generated code not found"
    The build script generated code but you can't import it.
    **Solution:** Ensure you're including from the correct location:

    ```rust
    pub mod sensor_data {
        include!(concat!(env!("OUT_DIR"), "/examples.rs"));
    }

    ```

    The filename after `OUT_DIR` should match your package name in the `.proto` file:

    ```protobuf
    package examples;  // Generates examples.rs

    ```

## Resources

- **[Message Generation](./message-generation.md)** - Understanding message architecture
- **[Custom Messages](./custom-messages.md)** - Manual message implementation
- **[Protobuf Documentation](https://protobuf.dev/)** - Official protobuf guide
- **[prost Crate](https://docs.rs/prost/)** - Rust protobuf library

**Use protobuf when you need schema evolution or cross-language compatibility beyond the ROS 2 ecosystem. Stick with CDR for standard ROS 2 interoperability.**
