# Building hiroz

**hiroz works without ROS 2 dependencies by default, enabling pure Rust development while optionally integrating with existing ROS 2 installations.** This flexible approach lets you choose your dependency level based on project requirements.

## Philosophy

hiroz follows a **dependency-optional** design:

- Build pure Rust applications without ROS 2 installed
- Use bundled message definitions for common types
- Opt-in to ROS 2 integration when needed
- Pay only for what you use

## Adding hiroz to Your Project

Get started by adding hiroz to your `Cargo.toml`. Choose the dependency setup that matches your needs:

### Scenario 1: Pure Rust with Custom Messages

**Use when:** You want to define your own message types without ROS 2 dependencies

**Add to your `Cargo.toml`:**

```toml
[dependencies]
hiroz = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
tokio = { version = "1", features = ["full"] }  # Async runtime required
```

**What you get:**

- Full hiroz functionality
- Custom message support via derive macros
- Zero external dependencies
- Fast build times

### Scenario 2: Using Bundled ROS Messages

**Use when:** You need standard ROS 2 message types (no ROS 2 installation required)

**Add to your `Cargo.toml`:**

```toml
[dependencies]
hiroz = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
hiroz-msgs = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }  # Includes core_msgs by default
tokio = { version = "1", features = ["full"] }
```

**Default message packages (core_msgs):**

- `std_msgs` - Primitive types (String, Int32, Float64, etc.)
- `geometry_msgs` - Spatial data (Point, Pose, Transform, Twist)
- `sensor_msgs` - Sensor data (LaserScan, Image, Imu, PointCloud2)
- `nav_msgs` - Navigation (Path, OccupancyGrid, Odometry)
- `example_interfaces` - Tutorial services (AddTwoInts)
- `action_tutorials_interfaces` - Tutorial actions (Fibonacci)

### Scenario 3: All Message Packages

**Use when:** You need all available message types including test messages

**Requirements:** None (all messages are vendored)

**Add to your `Cargo.toml`:**

```toml
[dependencies]
hiroz = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
hiroz-msgs = { git = "https://github.com/ZettaScaleLabs/hiroz.git", features = ["all_msgs"] }
tokio = { version = "1", features = ["full"] }
```

**Build your project:**

```bash
cargo build
```

**All available packages:**

- `std_msgs` - Basic types
- `geometry_msgs` - Spatial data
- `sensor_msgs` - Sensor data
- `nav_msgs` - Navigation
- `example_interfaces` - Tutorial services (AddTwoInts)
- `action_tutorials_interfaces` - Tutorial actions (Fibonacci)
- `test_msgs` - Test types

!!! tip
    The default `core_msgs` feature includes everything except `test_msgs`. Use `all_msgs` only if you need test message types.

## ROS 2 Distribution Compatibility

**hiroz defaults to ROS 2 Jazzy compatibility**, which is the recommended distribution for new projects. If you need to target a different distribution like Humble, see the [ROS 2 Distribution Compatibility](./distro-compatibility.md) chapter for detailed instructions.

**Quick reference:**

```bash
# Default (Jazzy) - works out of the box
cargo build

# For Humble - use --no-default-features
cargo build --no-default-features --features humble

# For Rolling/Kilted/Lyrical - just add the feature
cargo build --features rolling
```

The distribution choice affects type hash support and interoperability with ROS 2 nodes. See the [Distribution Compatibility chapter](./distro-compatibility.md) for full details.

## Development

This section is for contributors working on hiroz itself. If you're using hiroz in your project, you can skip this section.

### Package Organization

The hiroz repository uses a Cargo workspace with multiple packages:

| Package | Default Build | Purpose | Dependencies |
|---------|---------------|---------|--------------|
| **hiroz** | Yes | Core Eclipse Zenoh-native ROS 2 library | None |
| **hiroz-codegen** | Yes | Message generation utilities | None |
| **hiroz-msgs** | No | Pre-generated message types | None (all vendored) |
| **hiroz-tests** | No | Integration tests | hiroz-msgs |
| **rcl-z** | No | RCL C bindings | ROS 2 required |

!!! note
    Only `hiroz` and `hiroz-codegen` build by default. Other packages are optional for development, testing, and running examples.

### Building the Repository

When contributing to hiroz, you can build different parts of the workspace:

```bash
# Build core library
cargo build

# Run tests
cargo test

# Build with bundled messages for examples
cargo build -p hiroz-msgs

# Build all packages (requires ROS 2)
source /opt/ros/jazzy/setup.bash
cargo build --all
```

### Message Package Resolution

The build system automatically locates ROS message definitions:

**Search order:**

1. System ROS installation (`AMENT_PREFIX_PATH`, `CMAKE_PREFIX_PATH`)
2. Common ROS paths (`/opt/ros/{rolling,jazzy,kilted,lyrical,humble}`)
3. Bundled assets (built-in message definitions in hiroz-codegen)

This fallback mechanism enables builds without ROS 2 installed.

### Common Development Commands

```bash
# Fast iterative development
cargo check                # Quick compile check
cargo build                # Debug build
cargo build --release      # Optimized build
cargo test                 # Run tests
cargo clippy              # Lint checks

# Clean builds
cargo clean                # Remove all build artifacts
cargo clean -p hiroz-msgs  # Clean specific package
```

!!! warning
    After changing feature flags or updating ROS 2, run `cargo clean -p hiroz-msgs` to force message regeneration.

## Next Steps

- **[ROS 2 Distribution Compatibility](./distro-compatibility.md)** - Target Jazzy, Humble, or other distributions
- **[Running Examples](../user-guide/examples.md)** - Try out the included examples
- **[Networking](../user-guide/networking.md)** - Set up Zenoh router and session config
- **[Message Generation](../user-guide/message-generation.md)** - Understand how messages work
- **[Troubleshooting](../reference/troubleshooting.md)** - Solutions to common build issues

**Start with the simplest build and add dependencies incrementally as your project grows.**
