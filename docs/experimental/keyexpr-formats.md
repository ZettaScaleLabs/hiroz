# Key Expression Formats

**hiroz uses key expression formats to map ROS 2 entities (topics, services, actions) to Eclipse Zenoh key expressions.** The independent `hiroz-protocol` crate provides the format and determines how hiroz translates ROS 2 names for Zenoh routing and discovery.

!!! note
    Key expression format is a runtime choice that affects how hiroz maps ROS 2 entities to Zenoh key expressions. Choose the format that matches your infrastructure for proper message routing.

## Available Formats

hiroz supports multiple key expression formats for interoperability with different Zenoh-ROS bridges:

| Format | Compatibility | Use Case |
|--------|--------------|----------|
| **RmwZenoh** (default) | [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) | Standard ROS 2 Zenoh middleware |
| **Ros2Dds** | `zenoh-bridge-ros2dds` | DDS bridge compatibility |

### RmwZenoh Format (Default)

The RmwZenoh format targets compatibility with ROS 2's official Zenoh middleware implementation ([`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh)).

**Key Expression Patterns:**

```text
Topic keys:      <domain_id>/<topic>/<type>/<hash>
Liveliness:      @ros2_lv/<domain_id>/<entity_kind>/<namespace>/<name>/...
```

**Example Topic Keys:**

```text
0/chatter/std_msgs::msg::dds_::String_/RIHS01_...
5/robot/sensors/camera/sensor_msgs::msg::dds_::Image_/RIHS01_...
```

**Use this format when:**

- Using [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) as your ROS 2 middleware
- Running pure hiroz deployments
- Requiring domain isolation via Zenoh

### Ros2Dds Format

The Ros2Dds format targets compatibility with `zenoh-bridge-ros2dds`, which bridges standard DDS-based ROS 2 nodes to Zenoh.

**Key Expression Format:**

```text
<topic>/**
```

**Example:**

```text
chatter/**                    # Topic /chatter (no domain prefix)
robot/sensors/camera/**       # Topic /robot/sensors/camera
```

**Use this format when:**

- Bridging existing DDS-based ROS 2 systems to Zenoh
- Using `zenoh-bridge-ros2dds`
- Integrating with CycloneDDS or FastDDS nodes via Zenoh

**Discovery consequences:**

- `Ros2Dds` graph discovery can identify publishers/subscribers/services by topic or service name, but its liveliness data does not provide publisher node identity.
- Topic-based graph helpers such as publisher/subscriber matching work with `Ros2Dds`.
- Node-based discovery and automatic schema discovery via `create_dyn_sub_auto()` are not supported with `Ros2Dds`, because the publishing node cannot be identified from discovery data.

## Key Expression Behavior (IMPORTANT)

Understanding how hiroz converts topic names to key expressions is critical for debugging:

### Topic Key Expressions (For Data Routing)

**ALL entity types** (publishers, subscriptions, services, clients, actions) use `strip_slashes()` behavior:

- Removes **leading** and **trailing** slashes only
- **Preserves internal** slashes for hierarchical routing
- Enables multi-segment topic names

**Examples:**

| ROS 2 Topic Name | Topic Key Expression | ✓/✗ |
|------------------|---------------------|-----|
| `/chatter` | `0/chatter/...` | ✅ Correct |
| `/robot/sensors` | `0/robot/sensors/...` | ✅ Correct |
| `/a/b/c` | `0/a/b/c/...` | ✅ Correct |
| `/talker/service` | `0/talker/service/...` | ✅ Correct |

**Why preserve slashes?**

- Zenoh uses `/` for hierarchical routing
- Enables wildcard subscriptions: `0/robot/**`
- Human-readable key expressions

### Liveliness Tokens (For Discovery)

**ALL fields** in liveliness tokens use `mangle_name()` behavior:

- Replaces **all** `/` with `%`
- Ensures unambiguous parsing of entity metadata
- Machine-parsable format for discovery protocol

**Examples:**

| ROS 2 Name | Liveliness Field | ✓/✗ |
|-----------|-----------------|-----|
| `/chatter` | `%chatter` | ✅ Correct |
| `/robot/sensors` | `%robot%sensors` | ✅ Correct |
| `/my_node` | `%my_node` | ✅ Correct |

**Why mangle slashes?**

- Liveliness tokens have fixed structure: `@ros2_lv/<domain>/<kind>/<ns>/<name>/...`
- Prevents ambiguity when parsing fields
- Ensures reliable entity discovery

### Why Two Different Behaviors?

This is **intentional design** in [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh), not an inconsistency:

- **Topic keys**: Human-readable, hierarchical (optimized for Zenoh routing)
- **Liveliness**: Machine-parsable, unambiguous (optimized for discovery protocol)

!!! tip
    If multi-segment topics like `/robot/sensors/camera` don't receive messages, check your hiroz version. Versions before 0.1.0 had a bug where publishers incorrectly mangled topic key expressions.

## API Usage

### Specifying Format at Context Creation

```rust
use hiroz::context::ZContextBuilder;
use hiroz_protocol::KeyExprFormat;

// Default (RmwZenoh)
let ctx = ZContextBuilder::default().build()?;

// Explicit format selection
let ctx = ZContextBuilder::default()
    .keyexpr_format(KeyExprFormat::RmwZenoh)
    .build()?;

// Ros2Dds format for DDS bridge compatibility
let ctx = ZContextBuilder::default()
    .keyexpr_format(KeyExprFormat::Ros2Dds)
    .build()?;
```

**Key points:**

- Format is set at context creation time
- All nodes and entities created from the context use the same format
- Default format is `KeyExprFormat::RmwZenoh`
- Format choice is type-safe and explicit

### Creating Entities

Once you create the context with a format, all entities inherit it:

```rust
use hiroz_msgs::std_msgs::String as RosString;
use hiroz::Builder;

// Create context with RmwZenoh format (default)
let ctx = ZContextBuilder::default().build()?;
let node = ctx.create_node("my_node").build()?;

// Publisher uses context's format
let pub_rmw = node
    .create_pub::<RosString>("chatter")
    .build()?;

// Subscriber uses same format
let sub_rmw = node
    .create_sub::<RosString>("chatter")
    .build()?;
```

### Mixing Formats (Advanced)

To communicate with both RmwZenoh and Ros2Dds systems, create separate contexts:

```rust
// Context for rmw_zenoh_cpp nodes
let ctx_rmw = ZContextBuilder::default()
    .keyexpr_format(KeyExprFormat::RmwZenoh)
    .build()?;

// Context for zenoh-bridge-ros2dds nodes
let ctx_dds = ZContextBuilder::default()
    .keyexpr_format(KeyExprFormat::Ros2Dds)
    .build()?;

// Create nodes from each context
let node_rmw = ctx_rmw.create_node("rmw_node").build()?;
let node_dds = ctx_dds.create_node("dds_node").build()?;
```

## Architecture Diagrams

### RmwZenoh Format Architecture

```mermaid
graph LR
accTitle: RmwZenoh key expression format connecting hiroz and ROS 2 nodes
accDescr: A hiroz node in RmwZenoh format communicates through a Zenoh router using domain-prefixed key expressions to reach a standard ROS 2 node running rmw_zenoh_cpp.
    A[hiroz Node<br/>RmwZenoh Format] -->|"0/chatter/**"| B[Zenoh Router<br/>rmw_zenoh]
    B -->|"0/chatter/**"| C[ROS 2 Node<br/>rmw_zenoh_cpp]
```

**Use case:** Native Zenoh-based ROS 2 deployment

- All nodes use rmw_zenoh or hiroz
- Direct Zenoh communication
- Domain isolation via key expression prefix

### Ros2Dds Format Architecture

```mermaid
graph LR
accTitle: Ros2Dds key expression format bridging Zenoh and DDS networks
accDescr: A hiroz node in Ros2Dds format sends messages via Zenoh to the zenoh-bridge-ros2dds, which translates them to DDS for standard ROS 2 nodes running CycloneDDS or FastDDS.
    A[hiroz Node<br/>Ros2Dds Format] -->|"chatter/**"| B[zenoh-bridge-ros2dds<br/>Router + Bridge]
    B -->|DDS| C[ROS 2 Node<br/>CycloneDDS/FastDDS]
```

**Use case:** Bridge existing DDS systems to Zenoh

- ROS 2 nodes use standard DDS middleware
- `zenoh-bridge-ros2dds` translates DDS ↔ Zenoh
- hiroz communicates via Zenoh side of bridge

## Key Expression Generation Details

Understanding how `hiroz-protocol` generates key expressions helps with debugging and monitoring.

### Topic Key Expression Structure

```text
<domain_id>/<topic_stripped>/<type>/<hash>
```

**Components:**

1. **Domain ID**: ROS 2 domain (e.g., `0`, `5`)
2. **Topic (stripped)**: Topic name with leading/trailing slashes removed, internal slashes preserved
3. **Type**: Mangled message type (e.g., `std_msgs::msg::dds_::String_`)
4. **Hash**: Type hash for compatibility (e.g., `RIHS01_...`)

**Example:**

```text
Topic: /robot/sensors/camera
Type:  sensor_msgs/msg/Image
Hash:  RIHS01_abc123...

Key Expression:
0/robot/sensors/camera/sensor_msgs::msg::dds_::Image_/RIHS01_abc123...
```

### Liveliness Token Structure

```text
@ros2_lv/<domain>/<entity_kind>/<zid>/<id>/<namespace>/<name>/<type>/<hash>/<qos>
```

**hiroz mangles all name fields** (/ → %):

```text
Namespace: /robot/arm  →  %robot%arm
Name:      /gripper    →  %gripper
```

**Example:**

```text
@ros2_lv/0/MP/01234567890abcdef/1/%robot%arm/%gripper/std_msgs::msg::String_/RIHS01_.../qos_string
```

## hiroz-protocol Crate

The independent `hiroz-protocol` crate provides the key expression logic:

**Features:**

- `no_std` compatible (with `alloc`)
- Language-agnostic protocol layer (FFI-ready)
- Feature-gated format implementations
- Comprehensive unit tests
- Type-safe API

**Cargo features:**

```toml
[dependencies]
hiroz-protocol = { version = "0.1", features = ["rmw-zenoh", "ros2dds"] }
```

**Using hiroz-protocol directly:**

```rust
use hiroz_protocol::{KeyExprFormat, entity::*};

let format = KeyExprFormat::default(); // RmwZenoh

// Generate topic key expression
let topic_ke = format.topic_key_expr(&entity)?;

// Generate liveliness token
let lv_ke = format.liveliness_key_expr(&entity, &zid)?;

// Parse liveliness token back to entity
let parsed_entity = format.parse_liveliness(&lv_ke)?;
```

See the [hiroz-protocol crate](https://github.com/ZettaScaleLabs/hiroz/tree/main/crates/hiroz-protocol) for details.

## Troubleshooting

### Multi-Segment Topics Not Working?

**Symptom:** Publisher publishes to `/robot/sensors/camera` but subscriber never receives messages.

**Cause:** Old versions of hiroz (before 0.1.0) incorrectly mangled slashes in topic key expressions.

**Fix:** Update to hiroz 0.1.0+ which correctly uses `strip_slashes()` for all topic key expressions.

**Verify:** Enable debug logging to check key expressions:

```bash
RUST_LOG=hiroz=debug cargo run --example z_pubsub
```

Look for key expressions like:

```text
✅ Correct:  0/robot/sensors/camera/sensor_msgs::msg::Image_/...
❌ Wrong:    0/robot%sensors%camera/sensor_msgs::msg::Image_/...
```

### No Messages Between hiroz and rmw_zenoh_cpp?

**Check format:** Ensure hiroz uses `KeyExprFormat::RmwZenoh` (the default).

**Check type hash:** Enable debug logging and compare type hashes:

```bash
RUST_LOG=hiroz=debug cargo run
```

Type hashes must match between hiroz and rmw_zenoh_cpp. If they don't, you may have:

- Different message definitions
- Different ROS 2 distros
- Outdated generated messages

### No Messages Through zenoh-bridge-ros2dds?

**Check format:** Ensure hiroz uses `KeyExprFormat::Ros2Dds`:

```rust
let ctx = ZContextBuilder::default()
    .keyexpr_format(KeyExprFormat::Ros2Dds)
    .build()?;
```

**Check bridge configuration:** Verify `zenoh-bridge-ros2dds` is running and connected to the same Zenoh router as hiroz.

## Format Comparison

### When to Use RmwZenoh Format

✅ **Use RmwZenoh when:**

- Building pure Zenoh-based ROS 2 systems
- Using [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) middleware
- Requiring domain isolation
- Deploying new systems with native Zenoh support
- Maximizing Zenoh performance benefits

### When to Use Ros2Dds Format

✅ **Use Ros2Dds when:**

- Bridging existing DDS-based ROS 2 systems
- Using `zenoh-bridge-ros2dds`
- Integrating with legacy ROS 2 infrastructure
- Gradual migration from DDS to Zenoh
- Heterogeneous deployments (DDS + Zenoh)
