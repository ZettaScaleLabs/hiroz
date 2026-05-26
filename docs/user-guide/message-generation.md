# Message Generation

**Automatic Rust type generation from ROS 2 message definitions at build time.** The code generation system converts `.msg`, `.srv`, and `.action` files into type-safe Rust structs with full serialization support and ROS 2 compatibility.

!!! success
    Message generation happens automatically during builds. You write ROS 2 message definitions, hiroz generates idiomatic Rust code.

## System Architecture

```mermaid
graph LR
accTitle: Message generation system architecture from msg files to hiroz-msgs
accDescr: ROS message and service files are parsed and resolved by hiroz-codegen, hashed for type safety, then fed to Rust and Protobuf generators whose output is collected into the hiroz-msgs crate.
    A[.msg/.srv files] --> B[hiroz-codegen]
    B --> C[Parse & Resolve]
    C --> D[Type Hashing]
    D --> E[Code Generation]
    E --> F[Rust Generator]
    E --> G[Protobuf Generator]
    F --> H[Rust Structs + Traits]
    G --> I[Proto Files + Rust]
    H --> J[hiroz-msgs]
    I --> J
```

## Key Features

| Feature | Description | Benefit |
|---------|-------------|---------|
| **Build-time generation** | Runs during `cargo build` | No manual steps |
| **Bundled definitions** | Includes common ROS types | Works without ROS 2 |
| **Type safety** | Full Rust type system | Compile-time validation |
| **CDR compatible** | ROS 2 DDS serialization | Full interoperability |
| **Optional protobuf** | Additional serialization | Cross-language support |

## Component Stack

### hiroz-codegen

Internal message generation library for hiroz:

- Parses `.msg`, `.srv`, and `.action` file syntax
- Resolves message dependencies across packages
- Calculates ROS 2 type hashes (RIHS algorithm)
- Generates Rust structs with serde
- Bundles common message definitions

!!! info
    hiroz-codegen provides bundled messages for `std_msgs`, `geometry_msgs`, `sensor_msgs`, and `nav_msgs`. These work without ROS 2 installation.

### Orchestration Layer

hiroz-codegen's orchestration capabilities:

- Coordinates message discovery across sources
- Manages build-time code generation
- Provides code generators for different serialization formats
- Generates hiroz-specific traits

**Discovery workflow:**

```mermaid
sequenceDiagram
accTitle: Package discovery sequence checking system then bundled message sources
accDescr: build.rs asks the discovery layer to find packages, which checks AMENT_PREFIX_PATH then standard opt/ros paths and finally falls back to bundled assets before returning paths for Rust code generation.
    participant B as build.rs
    participant D as Discovery
    participant S as Sources

    B->>D: Find packages
    D->>S: Check AMENT_PREFIX_PATH
    alt Found in system
        S-->>D: System messages
    else Not found
        D->>S: Check /opt/ros/*
        alt Found in standard path
            S-->>D: System messages
        else Not found
            D->>S: Check bundled assets
            S-->>D: Bundled messages
        end
    end
    D-->>B: Package paths
    B->>B: Generate Rust code
```

### Code Generators

**Rust Generator (default):**

- Generates structs with serde
- CDR-compatible serialization via hiroz-cdr
- Full ROS 2 DDS interoperability
- No additional dependencies

**Protobuf Generator (optional):**

- Generates `.proto` files
- Protobuf-compatible types
- Cross-language data exchange
- Requires protobuf feature

## Generated Code

For each ROS 2 message, hiroz generates:

### Message Struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct String {
    pub data: std::string::String,
}
```

### Type Information Traits

```rust
impl MessageTypeInfo for std_msgs::String {
    fn type_name() -> &'static str {
        "std_msgs::msg::dds_::String_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::from_rihs_string("RIHS01_abc123...")
            .expect("Invalid hash")
    }
}

impl WithTypeInfo for std_msgs::String {
    fn type_info() -> TypeInfo {
        TypeInfo::new(Self::type_name(), Self::type_hash())
    }
}
```

**These traits enable:**

- Runtime type identification
- ROS 2 compatibility validation
- Proper DDS topic naming
- Type-safe message passing

!!! note
    Type hashes are critical for ROS 2 interoperability. They ensure nodes agree on message structure before exchanging data.

## Build Process

### hiroz-msgs Build Script

The generation happens in `build.rs`:

```mermaid
flowchart TD
accTitle: hiroz-msgs build script flow from feature flags to compiled output
accDescr: The build script reads enabled features, discovers package paths, parses and resolves message definitions if found, generates Rust code written to OUT_DIR, then compilation completes.
    A[Start build.rs] --> B[Read enabled features]
    B --> C[Discover package paths]
    C --> D{Messages found?}
    D -->|Yes| E[Parse message definitions]
    D -->|No| F[Build error]
    E --> G[Resolve dependencies]
    G --> H[Generate Rust code]
    H --> I[Write to OUT_DIR]
    I --> J[Compile completes]
```

**Configuration:**

```rust
let config = GeneratorConfig {
    generate_cdr: true,        // CDR-compatible types
    generate_protobuf: false,  // Optional protobuf
    generate_type_info: true,  // Trait implementations
    output_dir: out_dir,
};
```

### Package Discovery Order

```mermaid
flowchart LR
accTitle: Package discovery order by feature flags and ROS installation presence
accDescr: Feature flags trigger discovery that checks for a system ROS installation via AMENT_PREFIX_PATH or standard distro paths, falling back to bundled assets when no ROS is installed.
    A[Feature Flags] --> B{System ROS?}
    B -->|Found| C[AMENT_PREFIX_PATH]
    B -->|Not Found| D{/opt/ros/distro?}
    D -->|Found| E[Standard paths]
    D -->|Not Found| F[Bundled assets]

    C --> G[Generate from system]
    E --> G
    F --> H[Generate from bundled]
```

1. **System ROS:** `$AMENT_PREFIX_PATH`, `$CMAKE_PREFIX_PATH`
2. **Standard paths:** `/opt/ros/{rolling,jazzy,kilted,lyrical,humble}`
3. **Bundled assets:** Built-in message definitions in hiroz-codegen

This fallback enables development without ROS 2 installation.

## Using Generated Messages

### Import Pattern

```rust
use hiroz_msgs::ros::std_msgs::String as RosString;
use hiroz_msgs::ros::geometry_msgs::Twist;
use hiroz_msgs::ros::sensor_msgs::LaserScan;
```

### Namespace Structure

```text
hiroz_msgs::ros::{package}::{MessageName}
```

**Examples:**

- `hiroz_msgs::ros::std_msgs::String`
- `hiroz_msgs::ros::geometry_msgs::Point`
- `hiroz_msgs::ros::sensor_msgs::Image`

### Service Types

Services generate three types:

```rust
// Service definition
use hiroz_msgs::ros::example_interfaces::AddTwoInts;

// Request type
use hiroz_msgs::ros::example_interfaces::AddTwoIntsRequest;

// Response type
use hiroz_msgs::ros::example_interfaces::AddTwoIntsResponse;
```

!!! tip
    Import the service type for creation, then use the request/response types when handling calls.

## Message Packages

### Bundled Packages

Available without ROS 2:

| Package | Messages | Use Cases |
|---------|----------|-----------|
| **std_msgs** | String, Int32, Float64, etc. | Basic data types |
| **geometry_msgs** | Point, Pose, Twist, Transform | Spatial data |
| **sensor_msgs** | LaserScan, Image, Imu, PointCloud2 | Sensor readings |
| **nav_msgs** | Path, Odometry, OccupancyGrid | Navigation |

```bash
# Build with bundled messages
cargo build -p hiroz-msgs --features bundled_msgs
```

### Additional Packages

hiroz bundles these packages so they are available without ROS 2 installation:

| Package | Messages | Use Cases |
|---------|----------|-----------|
| **example_interfaces** | AddTwoInts, Fibonacci | Tutorials |
| **action_tutorials_interfaces** | Fibonacci action | Action tutorials |
| **test_msgs** | Test types | Testing |

```bash
# All packages are bundled by default
cargo build -p hiroz-msgs --features all_msgs
```

## Manual Custom Messages

For rapid prototyping without `.msg` files:

### Define the Struct

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RobotStatus {
    pub robot_id: String,
    pub battery_percentage: f64,
    pub position: [f64; 2],
    pub is_moving: bool,
}
```

### Implement Required Traits

```rust
use hiroz::{MessageTypeInfo, WithTypeInfo, entity::TypeHash};

impl MessageTypeInfo for RobotStatus {
    fn type_name() -> &'static str {
        "custom_msgs::msg::dds_::RobotStatus_"
    }

    fn type_hash() -> TypeHash {
        // For hiroz-to-hiroz only
        TypeHash::zero()
    }
}

impl WithTypeInfo for RobotStatus {}
```

!!! warning
    Manual messages with `TypeHash::zero()` work only between hiroz nodes. For ROS 2 interoperability, use generated messages with proper type hashes.

### When to Use Each Approach

```mermaid
flowchart TD
accTitle: Decision flowchart for manual versus generated custom messages
accDescr: Prototyping without ROS 2 interop or type safety requirements leads to manual implementation, while production use or ROS 2 interop requirements lead to generating Rust types from dot-msg files.
    A[Need Custom Message?] --> B{Prototyping?}
    B -->|Yes| C[Manual Implementation]
    B -->|No| D{ROS 2 Interop?}
    D -->|Required| E[Generate from .msg]
    D -->|Not Required| F{Want Type Safety?}
    F -->|Yes| E
    F -->|No| C
```

| Approach | Pros | Cons | Use When |
|----------|------|------|----------|
| **Manual** | Fast, flexible | No ROS 2 interop | Prototyping, internal only |
| **Generated** | Type hashes, portable | Requires .msg files | Production, ROS 2 systems |

## Serialization Formats

### CDR (Default)

Common Data Representation - ROS 2 standard:

- Full DDS compatibility
- Efficient binary encoding
- Used by all ROS 2 implementations
- Automatic via serde

```rust
// Generated with CDR support
#[derive(Serialize, Deserialize)]
pub struct String {
    pub data: std::string::String,
}
```

### Protobuf (Optional)

Protocol Buffers alternative:

```bash
cargo build -p hiroz-msgs --features protobuf
cargo build -p hiroz --features protobuf
```

**Benefits:**

- Schema evolution
- Cross-language compatibility
- Familiar ecosystem
- Efficient encoding

**Tradeoffs:**

- Not ROS 2 standard format
- Additional dependencies
- Requires feature flag

!!! info
    Use protobuf when you need schema evolution or cross-language data exchange beyond ROS 2 ecosystem. See [Protobuf Serialization](./protobuf.md) for detailed usage guide.

## Extending Message Packages

Add new packages to hiroz-msgs:

### 1. Add Feature Flag

Edit `hiroz-msgs/Cargo.toml`:

```toml
[features]
bundled_msgs = ["std_msgs", "geometry_msgs", "your_package"]
your_package = []
```

### 2. Update Build Script

Edit `hiroz-msgs/build.rs`:

```rust
fn get_bundled_packages() -> Vec<&'static str> {
    let mut names = vec!["builtin_interfaces"];

    #[cfg(feature = "your_package")]
    names.push("your_package");

    names
}
```

### 3. Rebuild

```bash
cargo build -p hiroz-msgs --features your_package
```

The build system automatically:

- Searches for the package
- Parses all message definitions
- Generates Rust types with traits
- Outputs to generated module

## Advanced Topics

### Message Filtering

The generator automatically filters:

- **Deprecated actionlib messages** - Old ROS 1 format
- **wstring fields** - Poor Rust support
- **Duplicate definitions** - Keeps first occurrence

### Type Hash Calculation

hiroz uses the RIHS (ROS IDL Hash) algorithm:

```mermaid
flowchart LR
accTitle: RIHS type hash calculation pipeline for ROS message definitions
accDescr: A message definition is parsed, its dependencies are included, a hash is calculated over the full structure, and the result is encoded as an RIHS string wrapped in a TypeHash object.
    A[Message Definition] --> B[Parse Structure]
    B --> C[Include Dependencies]
    C --> D[Calculate Hash]
    D --> E[RIHS String]
    E --> F[TypeHash Object]
```

**Properties:**

- Includes message structure and field types
- Incorporates dependency hashes
- Changes when definition changes
- Ensures type safety across network

**In generated code:**

```rust
TypeHash::from_rihs_string("RIHS01_1234567890abcdef...")
    .expect("Invalid RIHS hash string")
```

### Custom Code Generation

For custom build scripts:

```rust
use hiroz_codegen::{MessageGenerator, GeneratorConfig};

let config = GeneratorConfig {
    generate_cdr: true,
    generate_protobuf: false,
    generate_type_info: true,
    output_dir: out_dir.clone(),
};

let generator = MessageGenerator::new(config);
generator.generate_from_msg_files(&package_paths)?;
```

## Troubleshooting

### Package Not Found

```bash
# Check ROS 2 is sourced
echo $AMENT_PREFIX_PATH

# Verify package exists
ros2 pkg list | grep your_package

# Install if missing
sudo apt install ros-jazzy-your-package

# Bundled packages are built into hiroz-codegen
cargo build -p hiroz-msgs --features bundled_msgs
```

### Build Failures

| Error | Cause | Solution |
|-------|-------|----------|
| "Cannot find package" | Missing dependency | Enable feature or install ROS 2 package |
| "Type conflict" | Duplicate definition | Remove manual implementation |
| "Hash error" | Version mismatch | Update hiroz-codegen dependency |

See [Troubleshooting Guide](../reference/troubleshooting.md) for detailed solutions.

## Resources

- **[Feature Flags](../reference/feature-flags.md)** - Available message packages
- **[Building](../getting-started/building.md)** - Build configuration
- **[Custom Messages](./custom-messages.md)** - Manual implementation
- **[Protobuf Serialization](./protobuf.md)** - Alternative serialization format

**Message generation is transparent. Focus on writing ROS 2 message definitions and let hiroz handle the Rust code generation.**
