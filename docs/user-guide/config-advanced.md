# Advanced Configuration

## Generating Config Files

hiroz can generate JSON5 config files matching [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) defaults. This is opt-in via the `generate-configs` feature flag.

### Basic Generation

```bash
cargo build --features generate-configs
```

**Output location:**

```console
target/debug/build/hiroz-*/out/hiroz_config/
  ├── DEFAULT_HIROZ_ROUTER_CONFIG.json5
  └── DEFAULT_HIROZ_SESSION_CONFIG.json5
```

### Custom Output Directory

Specify a custom directory using the `HIROZ_CONFIG_OUTPUT_DIR` environment variable:

**Absolute path:**

```bash
HIROZ_CONFIG_OUTPUT_DIR=/etc/zenoh cargo build --features generate-configs
```

**Relative path (from package root):**

```bash
HIROZ_CONFIG_OUTPUT_DIR=./config cargo build --features generate-configs
```

**From workspace root:**

```bash
HIROZ_CONFIG_OUTPUT_DIR=$PWD/config cargo build -p hiroz --features generate-configs
```

!!! tip
    Generated files include inline comments explaining each setting, making them perfect documentation references.

### Using Generated Files

```rust
let ctx = ZContextBuilder::default()
    .with_config_file("./config/DEFAULT_HIROZ_SESSION_CONFIG.json5")
    .build()?;
```

## Configuration Reference

### Key Settings Explained

| Setting | Router | Session | Purpose |
|---------|--------|---------|---------|
| **Mode** | `router` | `peer` | Router relays messages, peers connect directly |
| **Listen Endpoint** | `tcp/[::]:7447` | - | Router accepts connections |
| **Connect Endpoint** | - | `tcp/localhost:7447` | Session connects to router |
| **Multicast** | Disabled | Disabled | Uses TCP gossip for discovery |
| **Unicast Timeout** | 60s | 60s | Handles slow networks/large deployments |
| **Query Timeout** | 10min | 10min | Long-running service calls |
| **Max Sessions** | 10,000 | - | Supports concurrent node startup |
| **Keep-Alive** | 2s | 2s | Optimized for loopback |

!!! note
    These defaults target ROS 2 deployments and match [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) exactly. Only modify them if you have specific performance requirements.

## Example: Full Session Config

A typical generated `DEFAULT_HIROZ_SESSION_CONFIG.json5` looks like this. Copy it, edit
the fields you need, and pass it to `.with_config_file()`:

```json5
{
  // Session mode: "peer" connects to a router, "router" acts as one
  mode: "peer",

  connect: {
    endpoints: [
      // Connect to the local router by default
      "tcp/localhost:7447",
    ],
    // Retry failed connections every 5 seconds
    retry: {
      period_init_ms: 5000,
      period_max_ms: 5000,
    },
  },

  listen: {
    endpoints: [],
  },

  scouting: {
    multicast: {
      // Multicast disabled — hiroz uses router-based discovery
      enabled: false,
    },
    gossip: {
      enabled: true,
      multihop: false,
    },
  },

  transport: {
    unicast: {
      // Timeout for initial connection handshake
      open_timeout: 60000,
      // Keep-alive interval (optimised for loopback traffic)
      keepalive: 2,
    },
    qos: {
      enabled: true,
    },
  },

  // Timeout for service calls (set high for long-running operations)
  queries_default_timeout: 600000,
}
```

## Common Customisations

### Connect to a Remote Router

Replace `localhost` with the router's IP address or hostname:

```json5
connect: {
  endpoints: ["tcp/10.0.0.1:7447"],
},
```

Or set it in code without editing a file:

```rust
let ctx = ZContextBuilder::default()
    .with_connect_endpoints(["tcp/10.0.0.1:7447"])
    .build()?;
```

### Connect to Multiple Routers

For resilient deployments with redundant routers:

```json5
connect: {
  endpoints: [
    "tcp/10.0.0.1:7447",
    "tcp/10.0.0.2:7447",
  ],
},
```

### Reduce Service Call Timeout

The default 10-minute query timeout is conservative. For real-time applications:

```json5
queries_default_timeout: 5000,  // 5 seconds
```

### Enable TLS

For encrypted inter-robot communication:

```json5
connect: {
  endpoints: ["tls/10.0.0.1:7448"],
},
transport: {
  unicast: {
    lowlatency: false,
    qos: { enabled: true },
    tls: {
      server_name: "robot-router",
      root_ca_certificate: "/etc/zenoh/ca.pem",
    },
  },
},
```

## Validating a Config File

Before deploying a modified config, verify it parses correctly:

```bash
# Run a short-lived session — if it connects cleanly, the config is valid
RUST_LOG=zenoh=info cargo run --example z_pubsub -- --config ./my_config.json5
```

A valid config produces:

```text
[INFO] Opening session...
[INFO] Declaring Publisher on 'hiroz/example'...
```

An invalid config produces a parse error before the session opens.
