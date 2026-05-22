# ROS 2 Distribution Compatibility

hiroz supports multiple ROS 2 distributions through compile-time feature flags. This chapter explains the differences between distributions and how to target specific ROS 2 versions.

> **Important**: Different hiroz components have different ROS 2 version requirements:
>
> - **hiroz core library**: Supports Humble, Jazzy, Kilted, Rolling
> - **rmw-zenoh-rs**: Requires Jazzy or later (see [rmw-zenoh-rs chapter](../experimental/rmw-zenoh-rs.md#ros-2-version-requirements))

## Supported Distributions

| Distribution | Core Library | rmw-zenoh-rs | Type Hash Support | Default |
|--------------|--------------|--------------|-------------------|---------|
| **Kilted Kaiju** | ✅ Supported | ✅ Supported | ✅ Yes | No |
| **Jazzy Jalisco** | ✅ Supported | ✅ Supported | ✅ Yes | **Yes** |
| **Humble Hawksbill** | ✅ Supported | ❌ **Not Supported** | ❌ Placeholder | No |
| Rolling Ridley | ✅ Supported | ✅ Supported | ✅ Yes | No |
| Iron Irwini | ⚠️ EOL (Dec 2024) | ❌ Not Supported | ✅ Yes | No |

**Default**: hiroz defaults to **Jazzy** compatibility, which is the recommended distribution for new projects.

## Distribution Differences

### Type Hash Support

The most significant difference between distributions is **type hash support**:

**Jazzy/Kilted/Rolling** (Modern):

- Supports real type hashes computed from message definitions
- Format: `RIHS01_<64-hex-chars>` (ROS IDL Hash Standard version 1)
- Enables type safety checks during pub/sub matching
- hiroz embeds type hashes in Eclipse Zenoh key expressions for discovery

**Humble** (Legacy):

- Does not support real type hashes
- Uses constant placeholder: `"TypeHashNotSupported"`
- No type safety validation during discovery
- Compatible with rmw_zenoh_cpp v0.1.8

### Example Key Expressions

**Jazzy:**

```text
@ros2_lv/0/<zid>/<nid>/<eid>/MP/%/<namespace>/<node>/chatter/std_msgs%msg%String/RIHS01_1234567890abcdef.../...
```

**Humble:**

```text
@ros2_lv/0/<zid>/<nid>/<eid>/MP/%/<namespace>/<node>/chatter/std_msgs%msg%String/TypeHashNotSupported/...
```

## Building for Different Distributions

### Using Jazzy (Default)

By default, hiroz builds with Jazzy compatibility. No special flags needed:

```bash
# Build with default (Jazzy)
cargo build

# Run examples
cargo run --example demo_nodes_talker

# Run tests
cargo nextest run
```

### Using Humble

To build for Humble, use `--no-default-features --features humble`:

```bash
# Build for Humble
cargo build --no-default-features --features humble

# Run examples for Humble
cargo run --no-default-features --features humble --example demo_nodes_talker

# Run tests for Humble
cargo nextest run --no-default-features --features humble
```

### Using Other Distributions

For Rolling or Kilted, specify the distro feature:

```bash
# Build for Rolling
cargo build --features rolling

# Build for Kilted
cargo build --features kilted
```
