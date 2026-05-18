# Custom Messages

hiroz supports two approaches for defining custom message types:

| Approach             | Definition                               | Best For                        |
| -------------------- | ---------------------------------------- | ------------------------------- |
| **Rust-Native**      | Write Rust structs directly              | Prototyping, hiroz-only systems |
| **Schema-Generated** | Write `.msg`/`.srv` files, generate Rust | Production, ROS 2 interop       |

```mermaid
flowchart TD
accTitle: Decision tree for choosing Rust-native or schema-generated messages
accDescr: If custom messages are needed and ROS 2 interop is required use schema-generated; for quick prototypes without interop use Rust-native; otherwise use standard messages.
    A[Need Custom Messages?] -->|Yes| B{ROS 2 Interop Needed?}
    B -->|Yes| C[Schema-Generated]
    B -->|No| D{Quick Prototype?}
    D -->|Yes| E[Rust-Native]
    D -->|No| C
    A -->|No| F[Use Standard Messages]
```

---

## Rust-Native Messages

**Define messages directly in Rust and derive their schema metadata.** This approach is fast for prototyping but only works between hiroz nodes.

!!! warning
    Rust-Native messages use `TypeHash::zero()` and won't interoperate with ROS 2 C++/Python nodes.

### Workflow of Rust-Native Messages

```mermaid
graph LR
accTitle: Rust-native message definition workflow
accDescr: Starting from defining the struct, the workflow proceeds through implementing MessageTypeInfo, adding Serde traits, implementing WithTypeInfo, and finally using the type in pub/sub.
    A[Define Struct] --> B[Impl MessageTypeInfo]
    B --> C[Add Serde Traits]
    C --> D[Impl ZMessage]
    D --> E[Use in Pub/Sub]
```

### Required Traits

| Trait                     | Purpose                              | Key Method                                       |
| ------------------------- | ------------------------------------ | ------------------------------------------------ |
| **MessageTypeInfo**       | Type identification + runtime schema | `type_name()`, `type_hash()`, `message_schema()` |
| **Serialize/Deserialize** | Data encoding                        | From `serde`                                     |
| **ZMessage**              | hiroz serialization path             | `type Serdes = SerdeCdrSerdes<Self>`             |

### Message Example

```rust,ignore
use hiroz::MessageTypeInfo;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize, MessageTypeInfo)]
#[ros_msg(type_name = "my_msgs/msg/RobotStatus")]
struct RobotStatus {
    battery_level: f32,
    position_x: f32,
    position_y: f32,
    is_moving: bool,
}

impl hiroz::msg::ZMessage for RobotStatus {
    type Serdes = hiroz::msg::SerdeCdrSerdes<Self>;
}
```

`MessageTypeInfo` derive currently supports named structs with deterministic ROS field mappings:
primitive numeric/bool types, `String`, `Vec<T>`, fixed arrays `[T; N]`, and nested message structs.
Tuple structs, unit structs, enums, `Option`, maps, and other richer Rust-only shapes are not yet supported.

### Service Example

```rust
use hiroz::{ServiceTypeInfo, TypeInfo, TypeHash, msg::ZService};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NavigateToRequest {
    target_x: f32,
    target_y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NavigateToResponse {
    success: bool,
}

struct NavigateTo;

impl ServiceTypeInfo for NavigateTo {
    fn service_type_info() -> TypeInfo {
        TypeInfo::new("my_msgs::srv::dds_::NavigateTo_", TypeHash::zero())
    }
}

impl ZService for NavigateTo {
    type Request = NavigateToRequest;
    type Response = NavigateToResponse;
}
```

See the `z_custom_message` example (clone the repo first: `git clone https://github.com/ZettaScaleLabs/hiroz.git && cd hiroz`):

When a publisher node enables the type description service, derived custom message types
automatically register their runtime schema so dynamic subscribers can discover them.

```bash
# Terminal 1: Router
cargo run --example zenoh_router

# Terminal 2: Subscriber
cargo run --example z_custom_message -- --mode status-sub

# Terminal 3: Publisher
cargo run --example z_custom_message -- --mode status-pub
```

---

## Schema-Generated Messages

**Define messages in `.msg`/`.srv` files and generate Rust code using `hiroz-codegen`.** This approach provides proper type hashes and can reference standard ROS 2 types.

!!! tip
    Schema-Generated messages get proper RIHS01 type hashes and can reference types from `hiroz_msgs` like `geometry_msgs/Point`.

### Workflow of Schema-Generated Messages

```mermaid
graph LR
accTitle: Schema-generated message workflow from msg files to Rust types
accDescr: The developer writes dot-msg files, creates a Rust crate with a build.rs, sets HIROZ_MSG_PATH, runs cargo build, and then uses the generated Rust types.
    A[Write .msg files] --> B[Create Rust crate]
    B --> C[Add build.rs]
    C --> D[Set HIROZ_MSG_PATH]
    D --> E[cargo build]
    E --> F[Use generated types]
```

### Step 1: Create Message Package

Create a ROS 2 style directory structure:

```text
my_robot_msgs/
├── msg/
│   ├── RobotStatus.msg
│   └── SensorReading.msg
└── srv/
    └── NavigateTo.srv
```

### Step 2: Define Messages

Messages can reference standard ROS 2 types:

```text
# RobotStatus.msg
string robot_id
geometry_msgs/Point position
bool is_moving
```

```text
# SensorReading.msg
builtin_interfaces/Time timestamp
float64[] values
string sensor_id
```

```text
# NavigateTo.srv
geometry_msgs/Point target
float64 max_speed
---
bool success
string message
```

### Step 3: Create Rust Crate

**Cargo.toml:**

```toml
[package]
name = "my-robot-msgs"
version = "0.1.0"
edition = "2021"

# Standalone package (not part of parent workspace)
[workspace]

[dependencies]
hiroz-msgs = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
hiroz = { git = "https://github.com/ZettaScaleLabs/hiroz.git", default-features = false }
serde = { version = "1", features = ["derive"] }
smart-default = "0.7"
zenoh-buffers = "1"

[build-dependencies]
hiroz-codegen = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
anyhow = "1"
```

**build.rs:**

```rust
use std::path::PathBuf;
use std::env;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    hiroz_codegen::generate_user_messages(&out_dir, false)?; // false = Jazzy-compatible (use true for Humble)
    println!("cargo:rerun-if-env-changed=HIROZ_MSG_PATH");
    Ok(())
}
```

**src/lib.rs:**

```rust
// Re-export standard types from hiroz-msgs
pub use hiroz_msgs::*;

// Include generated user messages
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
```

### Step 4: Build

Set `HIROZ_MSG_PATH` and build:

```bash
HIROZ_MSG_PATH="./my_robot_msgs" cargo build
```

For multiple packages, use colon-separated paths:

```bash
HIROZ_MSG_PATH="./my_msgs:./other_msgs" cargo build
```

### Step 5: Use Generated Types

```rust
use my_robot_msgs::ros::my_robot_msgs::{RobotStatus, SensorReading};
use my_robot_msgs::ros::my_robot_msgs::srv::NavigateTo;
use hiroz_msgs::ros::geometry_msgs::Point;
use hiroz_msgs::ros::builtin_interfaces::Time;

let status = RobotStatus {
    robot_id: "robot_1".to_string(),
    position: Point { x: 1.0, y: 2.0, z: 0.0 },
    is_moving: true,
};

let reading = SensorReading {
    timestamp: Time { sec: 1234, nanosec: 0 },
    values: vec![1.0, 2.0, 3.0],
    sensor_id: "lidar_1".to_string(),
};
```

See `crates/hiroz/examples/custom_msgs_demo/` for a working example:

```bash
cd crates/hiroz/examples/custom_msgs_demo
HIROZ_MSG_PATH="./my_robot_msgs" cargo build
```

---

## Comparison

| Feature                | Rust-Native        | Schema-Generated                         |
| ---------------------- | ------------------ | ---------------------------------------- |
| **Definition**         | Rust structs       | `.msg`/`.srv` files                      |
| **Type Hashes**        | `TypeHash::zero()` | Proper RIHS01 hashes                     |
| **Standard Type Refs** | Manual             | Automatic (`geometry_msgs`, etc.)        |
| **ROS 2 Interop**      | No                 | Partial (messages yes, services limited) |
| **Setup Complexity**   | Low                | Medium (build.rs required)               |
| **Best For**           | Prototyping        | Production                               |

---

## Type Naming Convention

Both approaches should follow ROS 2 DDS naming:

```text
# Messages
package::msg::dds_::MessageName_

# Services
package::srv::dds_::ServiceName_
```

The trailing underscore and `dds_` infix match ROS 2's internal naming scheme.

---

## Resources

- **[Message Generation](./message-generation.md)** - How hiroz-msgs generates standard types
- **[Protobuf Serialization](./protobuf.md)** - Alternative serialization format
- **[Publishers & Subscribers](../core-concepts/pubsub.md)** - Using messages in pub-sub
- **[Services](../core-concepts/services.md)** - Using messages in services
