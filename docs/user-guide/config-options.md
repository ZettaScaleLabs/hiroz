# Configuration Options

hiroz provides multiple ways to configure Eclipse Zenoh, from simple to advanced.

## Option 1: Default Configuration (Recommended)

Use the built-in ROS session config for standard deployments:

```rust
let ctx = ZContextBuilder::default().build()?;
```

**What it does:**

- Connects to router at `tcp/localhost:7447`
- Uses ROS-compatible timeouts and buffer sizes
- Disables multicast discovery (uses router instead)

## Option 2: Custom Router Endpoint

Connect to a router on a different host or port:

```rust
let ctx = ZContextBuilder::default()
    .with_router_endpoint("tcp/192.168.1.100:7447")
    .build()?;
```

**Use cases:**

- Distributed systems with remote router
- Custom port configurations
- Multiple isolated networks

## Option 3: Config File via Environment Variable

Point to a JSON5 config file without changing code, using the same variable as [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh):

```bash
export ZENOH_SESSION_CONFIG_URI=/path/to/session_config.json5
```

```rust
// No code changes needed - the config file is loaded automatically
let ctx = ZContextBuilder::default().build()?;
```

## Option 4: Environment Variable Overrides

Override individual Zenoh configuration settings using the `ZENOH_CONFIG_OVERRIDE` environment variable, using the same variable as [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh):

```bash
# Override mode and endpoint
export ZENOH_CONFIG_OVERRIDE='mode="client";connect/endpoints=["tcp/192.168.1.100:7447"]'

# Run any hiroz application — overrides apply automatically
cargo run --example z_pubsub
```

```rust
// No code changes needed - overrides are applied automatically
let ctx = ZContextBuilder::default().build()?;
```

**Format:**

- Semicolon-separated `key=value` pairs
- Values use JSON5 syntax
- Keys use slash-separated paths (e.g., `connect/endpoints`, `scouting/multicast/enabled`)

**Common examples:**

```bash
# Connect to remote router
export ZENOH_CONFIG_OVERRIDE='connect/endpoints=["tcp/10.0.0.5:7447"]'

# Enable multicast scouting explicitly
export ZENOH_CONFIG_OVERRIDE='scouting/multicast/enabled=true'

# Multiple overrides
export ZENOH_CONFIG_OVERRIDE='mode="client";connect/timeout_ms=5000;scouting/multicast/enabled=false'
```

!!! tip
    Environment variable overrides have the highest priority and will override any programmatic configuration or config file settings.

## Option 5: Advanced Configuration Builders

Fine-tune session or router settings programmatically:

```rust
use hiroz::config::{SessionConfigBuilder, RouterConfigBuilder};

// Customize session config
let session_config = SessionConfigBuilder::new()
    .with_router_endpoint("tcp/192.168.1.100:7447")
    .build()?;

let ctx = ZContextBuilder::default()
    .with_zenoh_config(session_config)
    .build()?;

// Or build a custom router config
let router_config = RouterConfigBuilder::new()
    .with_listen_port(7448)  // Custom port
    .build()?;

zenoh::open(router_config).await?;
```

**Key builder methods:**

| Builder | Methods | Purpose |
|---------|---------|---------|
| `SessionConfigBuilder` | `with_router_endpoint(endpoint)` | Connect to custom router |
| `RouterConfigBuilder` | `with_listen_port(port)` | Set custom router port |

## Option 6: Peer Mode Using Multicast Discovery (No Router Required)

Revert to multicast peer discovery for simple setups:

```rust
// Use vanilla Zenoh config (peer mode with multicast)
let ctx = ZContextBuilder::default()
    .with_zenoh_config(zenoh::Config::default())
    .build()?;
```

!!! warning
    Multicast scouting discovery is convenient for quick testing but doesn't scale well and won't work with ROS 2 nodes using [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) (which expects a zenoh router).

## Option 7: Load from Config File (Programmatic)

Use JSON5 config files for complex deployments:

```rust
let ctx = ZContextBuilder::default()
    .with_config_file("/etc/zenoh/session_config.json5")
    .build()?;
```

**When to use:**

- Deploying across multiple machines
- Environment-specific configurations
- Version-controlled infrastructure
