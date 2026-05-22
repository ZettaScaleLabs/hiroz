# rmw_zenoh_rs

[`rmw_zenoh_rs`](https://github.com/ros2/rmw_zenoh) is a Rust-based ROS Middleware (RMW) implementation that enables standard ROS 2 C++/Python nodes to communicate via Eclipse Zenoh. It allows you to use Zenoh as the transport layer for your existing ROS 2 applications without modifying your code. It is **fully interoperable** with [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh), allowing seamless communication between nodes using either implementation.

## What is an RMW?

ROS 2 uses a middleware abstraction layer called RMW (ROS Middleware) that allows you to choose different transport implementations. By default, ROS 2 uses DDS-based middleware like Fast-DDS or CycloneDDS. With rmw_zenoh_rs, you can use Zenoh instead.

```text
┌─────────────────────────────────────────────────┐
│              ROS 2 Application                  │
│         (C++ node using rclcpp)                 │
├─────────────────────────────────────────────────┤
│                    RCL                          │
│         (ROS Client Library - C)                │
├─────────────────────────────────────────────────┤
│                    RMW                          │
│      (ROS Middleware Interface - C)             │
├─────────────────────────────────────────────────┤
│              rmw_zenoh_rs                       │  ← This package
│         (Rust implementation)                   │
│  ┌──────────────────────────────────────────┐   │
│  │  FFI Layer (bindgen + cxx)               │   │
│  ├──────────────────────────────────────────┤   │
│  │  hiroz primitives                        │   │
│  │  (pubsub, service, guard_condition)      │   │
│  ├──────────────────────────────────────────┤   │
│  │  Zenoh                                   │   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
```

## Requirements

### ROS 2 Version Requirements

!!! warning "Humble Not Supported"
    rmw-zenoh-rs requires **ROS 2 Jazzy (2023) or later**.
    ROS 2 Humble (2022 LTS) is **not supported**.

**Supported ROS 2 Distributions:**

- ✅ **Jazzy** (2023)
- ✅ **Jazzy** (2024 LTS) - Recommended
- ✅ **Kilted** (2025)
- ✅ **Rolling**
- ❌ **Humble** (2022 LTS) - See [Why No Humble Support?](#why-no-humble-support)

### System Requirements

- **ROS 2 Jazzy or later** (Jazzy recommended)
- **Rust toolchain** (1.91 or later)
- **Cargo** (comes with Rust)
- **CMake** (3.16 or later)
- **Clang** (for bindgen)

### ROS 2 Dependencies

rmw-zenoh-rs requires the following ROS 2 packages:

- `rmw` - RMW interface definitions
- `rcutils` - ROS C utilities
- `rcpputils` - ROS C++ utilities
- `fastcdr` - Fast CDR serialization
- `rosidl_typesupport_fastrtps_c` - Type support for C
- `rosidl_typesupport_fastrtps_cpp` - Type support for C++

These are typically installed with ROS 2:

```bash
# Ubuntu/Debian with ROS 2 Jazzy
sudo apt install ros-jazzy-rmw ros-jazzy-rcutils ros-jazzy-rcpputils \
  ros-jazzy-fastcdr ros-jazzy-rosidl-typesupport-fastrtps-c \
  ros-jazzy-rosidl-typesupport-fastrtps-cpp
```

## Building rmw_zenoh_rs

```bash
# Source your ROS 2 installation
source /opt/ros/jazzy/setup.bash

# Create a workspace
mkdir -p ~/ros2_ws/src
cd ~/ros2_ws/src

# Clone hiroz repository
git clone https://github.com/ZettaScaleLabs/hiroz.git

# Build rmw_zenoh_rs
cd ~/ros2_ws
colcon build --packages-select rmw_zenoh_rs --symlink-install
```

### Build Output

After building, you'll have:

- **`librmw_zenoh_rs.so`** - The RMW library (Linux)
- **RMW plugin registration** - Allows ROS 2 to discover rmw_zenoh_rs

## Using rmw_zenoh_rs

### Setting the RMW Implementation

To use rmw_zenoh_rs, set the `RMW_IMPLEMENTATION` environment variable:

```bash
export RMW_IMPLEMENTATION=rmw_zenoh_rs
```

This tells ROS 2 to use rmw_zenoh_rs instead of the default DDS implementation.

### Example: Running ROS 2 Nodes

Start a Zenoh router first, then run your nodes with the RMW implementation set:

```bash
# Terminal 1: Start Zenoh router (required)
zenohd

# Terminal 2: Talker
source ~/ros2_ws/install/setup.bash
export RMW_IMPLEMENTATION=rmw_zenoh_rs
ros2 run demo_nodes_cpp talker

# Terminal 3: Listener
source ~/ros2_ws/install/setup.bash
export RMW_IMPLEMENTATION=rmw_zenoh_rs
ros2 run demo_nodes_cpp listener
```

Your nodes will now communicate via Zenoh instead of DDS!


!!! warning
    Unlike multicast-based discovery, router-based architecture is **required** by default. Both rmw_zenoh_rs and rmw_zenoh_cpp expect a router at `tcp/localhost:7447`. Without a router, nodes will not discover each other.

**Why a Zenoh router?**

hiroz, rmw_zenoh_rs, and rmw_zenoh_cpp all use router-based discovery by default, which provides:

- **Better scalability** - Handles large deployments with many nodes
- **Lower network overhead** - More efficient than multicast discovery
- **Cross-network communication** - Nodes can discover each other across network boundaries
- **Production-ready architecture** - Standard approach used in real ROS 2 systems
- **Interoperability** - Required for hiroz nodes to work with rmw_zenoh_cpp nodes

!!! info
    The router runs as a separate process and manages discovery and routing between all Zenoh-based nodes. You only need one router per network, regardless of how many nodes you run.

## Why No Humble Support?

ROS 2 Jazzy (May 2023) introduced **breaking changes to the RMW API** that are fundamental to how rmw-zenoh-rs works. These are not minor API additions - they are architectural changes defined in ROS Enhancement Proposals (REPs).

### Missing APIs in Humble

#### 1. Type Introspection & Hashing (REP 2011)

**What's Missing:**

- `rosidl_type_hash_t` type structure
- Type hash computation functions
- Type description support

**Why It Matters:**

rmw-zenoh-rs uses type hashes as part of Zenoh key expressions for topic matching and type safety validation. This is a core architectural feature, not an optional add-on.

**Example:**

```text
Jazzy+ Key: /topic/String/RIHS01_<64-char-hash>/...
Humble:    Cannot generate proper type hash
```

#### 2. Discovery Options (REP 2019)

**What's Missing:**

- `rmw_discovery_options_t` configuration
- Discovery control functions
- Participant filtering APIs

**Why It Matters:**

Used for network optimization and selective discovery in distributed systems.

#### 3. FastCDR API Changes

**What's Missing:**

- Humble uses FastCDR v1.x
- Jazzy+ uses FastCDR v2.x
- Constructor signatures incompatible

#### 4. Additional Missing Features

- Type hash fields in endpoint info structs
- QoS event types (`rmw_incompatible_type_status_t`, etc.)
- Multiple discovery and introspection functions

### Impact

Supporting Humble would require:

- 100+ conditional compilation guards throughout the codebase
- Maintaining two different RMW API implementations
- Stub implementations for core features
- Ongoing maintenance burden
- Compromised architecture

### Alternatives for Humble Users

If you need ROS 2 Humble support, you have two options:

#### Option 1: Use hiroz Core Library (Recommended)

The **hiroz core library** has full Humble support:

```rust
use hiroz::{Builder, ZContext};

let ctx = ZContext::default();
let node = ctx.create_node("my_node").build()?;

// Full pub/sub, services, actions support on Humble ✅
let publisher = node.create_publisher::<String>("topic").build()?;
```

This gives you pure Rust ROS 2 functionality on Humble without needing the RMW layer.

#### Option 2: Use rmw_zenoh_cpp

Use the C++ RMW implementation, which supports Humble:

```bash
# Install C++ RMW for Humble
apt install ros-humble-rmw-zenoh-cpp

# Set RMW implementation
export RMW_IMPLEMENTATION=rmw_zenoh_cpp

# Your C++/Python nodes will use Zenoh
ros2 run demo_nodes_cpp talker
```

**Interoperability**: hiroz nodes can communicate seamlessly with rmw_zenoh_cpp nodes, so you can mix Rust and C++/Python nodes.

## rmw_zenoh_rs vs rmw_zenoh_cpp

Both [`rmw_zenoh_rs`](https://github.com/ros2/rmw_zenoh) and [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) are RMW implementations using Zenoh, but with different design goals:

| Feature | rmw_zenoh_rs | rmw_zenoh_cpp |
|---------|--------------|---------------|
| **Implementation Language** | Rust (using hiroz) | C++ |
| **Primary Use Case** | Integration with hiroz ecosystem | Standalone Zenoh RMW |
| **ROS 2 Compatibility** | Jazzy, Jazzy, Kilted, Rolling | Humble, Jazzy, Jazzy, Kilted, Rolling |
| **Humble Support** | ❌ No | ✅ Yes |
| **Status** | Experimental | Production-ready |
| **Dependencies** | hiroz, Zenoh Rust | Zenoh C++ binding |
| **Performance** | Optimized for Rust stack | Optimized for C++ stack |
| **Interoperability** | ✅ Works with rmw_zenoh_cpp | ✅ Works with rmw_zenoh_rs |

## Configuration

rmw_zenoh_rs uses the same Zenoh configuration as rmw_zenoh_cpp. See [Configuration Options](../user-guide/config-options.md) for details.


## See Also

- [Zenoh Documentation](https://zenoh.io/docs/)
- [ROS 2 RMW Documentation](https://docs.ros.org/en/jazzy/Concepts/Intermediate/About-Different-Middleware-Vendors.html)
- [hiroz GitHub Repository](https://github.com/ZettaScaleLabs/hiroz)
