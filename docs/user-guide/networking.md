# Networking

**Configure hiroz's Eclipse Zenoh transport layer for optimal performance in your deployment environment.** hiroz uses router-based architecture by default, matching ROS 2's official [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) middleware for production-ready scalability.

```mermaid
graph TB
accTitle: Zenoh router-based node topology
accDescr: Talker and Listener nodes both connect to a central zenohd router for discovery, with optional peer-to-peer communication between them.
    Router["zenohd (router)"]

    Talker["Talker node (peer)"]
    Listener["Listener node (peer)"]

    Router <-->|Discovery| Talker
    Router <-->|Discovery| Listener
    Talker <-.->|P2P Communication| Listener
```

## How ROS 2 Discovery Works

ROS 2 nodes find each other automatically through a discovery process — no central registry or manual configuration needed. Discovery scope depends on **ROS domain**: only nodes with the same `ROS_DOMAIN_ID` can see each other (default: 0). The standard ROS 2 middleware (DDS) uses **multicast UDP** to announce node presence on the local network. Once discovered, nodes check QoS compatibility — connections only form when publisher and subscriber have compatible settings.

**The discovery cycle:**

1. **Announce**: a new node broadcasts its presence and advertises its publishers, subscribers, services, and actions
2. **Respond**: existing nodes reply with their own information; endpoints with matching topics negotiate connections
3. **Periodic heartbeat**: nodes re-broadcast periodically so nodes that join later can discover the system
4. **Departure**: nodes announce when going offline (best-effort — the system may not detect crashes immediately)

**Why hiroz replaces DDS discovery with Zenoh:**

Standard DDS multicast discovery has known limitations:

- Cloud networks and containers often block multicast/container networks and across subnets
- Discovery traffic grows quadratically with node count in large systems
- Configuration (tuning QoS, domain separation) is complex

hiroz and [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) use **router-based discovery** instead. All nodes connect to a `zenohd` router via TCP. The router acts as the rendezvous point — nodes announce themselves to the router, which propagates the information to interested peers. This works across subnets, containers, and cloud environments without multicast, and scales linearly: adding more nodes does not increase per-node discovery traffic.

```mermaid
sequenceDiagram
accTitle: Zenoh router discovery sequence for publisher and subscriber
accDescr: Node A declares a publisher and Node B declares a subscriber through the zenohd router, which notifies each side so messages can flow directly between them.
    participant R as zenohd (router)
    participant A as Node A (Publisher)
    participant B as Node B (Subscriber)

    A->>R: Connect + declare publisher on /chatter
    B->>R: Connect + declare subscriber on /chatter
    R->>A: Subscriber joined /chatter
    R->>B: Publisher available on /chatter
    Note over A,B: Connection negotiated — data flows
    A->>B: Messages (Eclipse Zenoh transport)
```

**QoS and discovery:** even after discovery, a publisher and subscriber will not exchange messages if their QoS policies are incompatible (e.g., a best-effort publisher and a reliable subscriber). hiroz checks compatibility at connection time.

The following sections describe how to configure hiroz's router connection for your deployment environment.

## Router-Based Architecture

hiroz uses a centralized Zenoh router for node discovery and communication, providing:

| Benefit | Description |
|---------|-------------|
| **Scalability** | Centralized discovery handles large deployments efficiently |
| **Lower Network Overhead** | TCP-based discovery instead of multicast broadcasts |
| **ROS 2 Compatibility** | Matches [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) behavior for seamless interoperability |
| **Production Ready** | Battle-tested configuration used in real robotics systems |

## Quick Start

The default hiroz configuration connects to a Zenoh router on `tcp/localhost:7447`:

```rust
use hiroz::context::ZContextBuilder;
use hiroz::Builder;

let ctx = ZContextBuilder::default()
    .with_connect_endpoints(["tcp/127.0.0.1:7447"])
    .build()?;
let node = ctx.create_node("my_node").build()?;
```

!!! success
    That's it! Connect to the router address and you're ready. Now you need to run one.

## Running the Zenoh Router

hiroz applications require a Zenoh router to be running. There are several ways to get one - choose based on your environment and requirements.

### Quick Comparison

| Method | Best For | Requires | Setup Speed |
|--------|----------|----------|-------------|
| [Cargo Install](#method-1-cargo-install) | Rust developers | Rust toolchain | Slower (build from source) |
| [Pre-built Binary](#method-2-pre-built-binary) | Quick setup, no Rust | None | Fast |
| [Docker](#method-3-docker) | Containers, CI/CD | Docker | Fast |
| [Package Manager](#method-4-package-manager-apt-brew) | System-wide install | apt/brew/etc | Fast |
| [hiroz Example](#method-5-hiroz-example-router) | hiroz repo developers | hiroz repository | Fast |
| [ROS 2 rmw_zenoh](#method-6-ros-2-rmw_zenoh) | ROS 2 interop testing | ROS 2 installed | Already installed |

---

### Method 1: Cargo Install

**Recommended for Rust developers building standalone projects.**

Install the official Zenoh router using Cargo:

```bash
cargo install zenohd
```

Run the router:

```bash
zenohd
```

Requires Rust toolchain; takes 2–5 min to compile but always builds the latest version.

---

### Method 2: Pre-built Binary

**Fastest way to get started without Rust installed.**

Download the archive for your platform from the [Zenoh Releases page](https://github.com/eclipse-zenoh/zenoh/releases):

| Platform | File |
|----------|------|
| Linux x86_64 | `zenoh-*-x86_64-unknown-linux-gnu-standalone.zip` |
| macOS Apple Silicon | `zenoh-*-aarch64-apple-darwin-standalone.zip` |
| macOS Intel | `zenoh-*-x86_64-apple-darwin-standalone.zip` |
| Windows | `zenoh-*-x86_64-pc-windows-msvc-standalone.zip` |

Then extract and run:

```bash
# Linux/macOS
unzip zenoh-*.zip && chmod +x zenohd && ./zenohd

# Windows (PowerShell)
Expand-Archive zenoh-*.zip; .\zenoh\zenohd.exe
```

No build tools required. More info: <https://zenoh.io/docs/getting-started/installation/>

---

### Method 3: Docker

**Perfect for containerized deployments and CI/CD pipelines.**

```bash
docker run --init --net host eclipse/zenoh:latest
```

For production with a config file:

```bash
docker run -d --name zenoh-router --net host \
  -v /path/to/config:/zenoh/config \
  eclipse/zenoh:latest --config /zenoh/config/zenoh.json5
```

Use `--net host` to avoid Docker network isolation blocking port 7447. Docker Hub: <https://hub.docker.com/r/eclipse/zenoh/tags>

---

### Method 4: Package Manager (apt, brew)

**Best for system-wide installation on Linux/macOS.**

```bash
# Ubuntu/Debian
echo "deb [trusted=yes] https://download.eclipse.org/zenoh/debian-repo/ /" | sudo tee /etc/apt/sources.list.d/zenoh.list
sudo apt update && sudo apt install zenoh
zenohd          # or: sudo systemctl start zenoh

# macOS
brew tap eclipse-zenoh/homebrew-zenoh && brew install zenoh && zenohd

# Arch Linux
yay -S zenoh && zenohd
```

---

### Method 5: hiroz Example Router

**Only available when working in the hiroz repository.**

```bash
cd /path/to/hiroz
cargo run --example zenoh_router
```

Pre-configured for hiroz defaults. Not suitable for standalone projects.

!!! warning
    This method is for hiroz repository development only. If you're building your own project with hiroz as a dependency, use one of the other methods instead.

---

### Method 6: ROS 2 rmw_zenoh

**Use this if you have ROS 2 installed and want to test interoperability with ROS 2 nodes.**

If you have ROS 2 Jazzy or newer with the Zenoh middleware:

```bash
ros2 run rmw_zenoh_cpp rmw_zenohd
```

Already installed with ROS 2 Jazzy+. Guaranteed compatibility with C++/Python ROS 2 nodes. Requires a full ROS 2 installation.

!!! tip
    Use this when testing hiroz interoperability with standard ROS 2 nodes — they share the same router.

---

## Verifying the Router is Running

After starting a router with any method above, verify it's working:

**Check the router is listening on port 7447:**

```bash
# Linux/macOS
netstat -an | grep 7447

# Or use lsof
lsof -i :7447
```

**Test with a hiroz application:**

```bash
# In another terminal, try running a hiroz node
# If it connects successfully, the router is working
```

You should see log output from the router showing connections when your hiroz nodes start.

## Next Steps

Choose the configuration approach that fits your needs:

- **[Configuration Options](./config-options.md)** - Six ways to configure Zenoh (from simple to complex)
- **[Advanced Configuration](./config-advanced.md)** - Generate config files, run routers, configuration reference
- **[Troubleshooting](../reference/troubleshooting.md)** - Solutions to connectivity issues

**Ready to optimize your deployment? Experiment with different configurations and measure performance impact.**
