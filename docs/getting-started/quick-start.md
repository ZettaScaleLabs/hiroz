<!-- markdownlint-disable MD046 -->
# Quick Start

**Get hiroz running in under 5 minutes.** Pick your language below — no ROS 2 installation required.

## Pre-built Releases

Pre-built artifacts are available on the [Releases page](https://github.com/ZettaScaleLabs/hiroz/releases) — no Rust toolchain required:

| Artifact | Use case | Install |
|---|---|---|
| `hiroz-console` binary | Monitor any ROS 2 / hiroz system | [Console docs](../tools/console.md#installation) |
| `hiroz-bridge` binary | Bridge Humble ↔ Jazzy/Kilted/Lyrical | [Bridge docs](../user-guide/bridge.md#installation) |
| `hiroz_py` Python wheel | Python pub/sub & services | [Python quick start](../bindings/python-quick-start.md) |
| `libhiroz` Go library | Go pub/sub & services | [Go quick start](../bindings/go-quick-start.md) |

---

## Embed hiroz in a Rust Application

The rest of this page covers using hiroz from Rust. If you're using Python or Go, follow the quick start links in the table above instead.

**Prerequisites:**

- **Rust 1.85+** (requires edition 2024) — install via [rustup](https://rustup.rs/)
- **Tokio 1.x** — hiroz requires an async runtime (added to `Cargo.toml` in Option 2 below)
- No ROS 2 installation needed

There are two ways to get started:

1. **Option 1: Try the Examples** — Clone the hiroz repository and run pre-built examples (fastest)
2. **Option 2: Create Your Own Project** — Start a new Rust project with hiroz as a dependency

---

## Option 1: Try the Examples

The quickest way to experience hiroz is to run the included examples from the repository.

### Clone the Repository

```bash
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz
```

### Start the Eclipse Zenoh Router

The router is a lightweight process that lets nodes find each other — think of it as the matchmaker between publishers and subscribers. Keep it running in its own terminal for the entire session.

**Terminal 1 — Start the Router:**

```bash
cargo run --example zenoh_router
```

### Run the Pub/Sub Example

Open two more terminals and navigate to the same `hiroz` directory:

**Terminal 2 — Start the Listener:**

```bash
cargo run --example z_pubsub -- --role listener
```

**Terminal 3 — Start the Talker:**

```bash
cargo run --example z_pubsub -- --role talker
```

!!! success
    You should see the listener receiving messages published by the talker in real-time. Press Ctrl+C to stop any process.

### Understanding the Code

The example you just ran does four things:

1. **Creates a context** — `ZContextBuilder` connects to the Zenoh router and sets up the ROS 2 domain.
2. **Creates a node** — `ctx.create_node("name")` is the logical unit your publishers and subscribers attach to.
3. **Publishes** — `node.create_pub::<RosString>("/chatter")` sends CDR-serialized ROS 2 messages on a topic.
4. **Subscribes** — `node.create_sub::<RosString>("/chatter")` receives and deserializes them asynchronously.

The full example source (with CLI flags and multi-backend support) is at [`examples/z_pubsub.rs`](https://github.com/ZettaScaleLabs/hiroz/blob/main/crates/hiroz/examples/z_pubsub.rs) in the repository.

---

## Option 2: Create Your Own Project

Ready to build your own hiroz application? Follow these steps to create a new project from scratch.

### 1. Install the Zenoh Router

Since you won't have access to the `zenoh_router` example outside the hiroz repository, you'll need to install a Zenoh router. Here are the quickest options:

**Option A: Using cargo (if you have Rust):**

```bash
cargo install zenohd
```

**Option B: Using pre-built binary (no Rust needed):**

Download the latest release for your platform from:
**<https://github.com/eclipse-zenoh/zenoh/releases>**

Then extract and run:

```bash
unzip zenoh-*.zip
chmod +x zenohd
./zenohd
```

**Option C: Using Docker:**

```bash
docker run --init --net host eclipse/zenoh:latest
```

**Start the router:**

```bash
zenohd
```

!!! tip
    For more installation options (apt, brew, Windows, etc.), see the comprehensive [Zenoh Router Installation Guide](../user-guide/networking.md#running-the-zenoh-router).

!!! note
    Keep the router running in a separate terminal. All hiroz applications will connect to it.

### 2. Create a New Rust Project

```bash
cargo new my_hiroz_project
cd my_hiroz_project
```

### 3. Add Dependencies

Add hiroz to your `Cargo.toml`:

```toml
[dependencies]
hiroz = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }
hiroz-msgs = { git = "https://github.com/ZettaScaleLabs/hiroz.git" }  # Standard ROS 2 message types
tokio = { version = "1", features = ["full"] }  # Async runtime
```

!!! note
    hiroz requires an async runtime. This example uses Tokio, the most popular choice in the Rust ecosystem.

### 4. Write Your First Application

Replace the contents of `src/main.rs` with this simple publisher example:

!!! tip "Simpler imports with prelude"
    Instead of importing `Builder`, `Result`, and `ZContextBuilder` separately, use `use hiroz::prelude::*;` to bring all common hiroz types into scope at once.

```rust
use std::time::Duration;
use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize hiroz context (connects to router on localhost:7447)
    let ctx = ZContextBuilder::default()
        .with_connect_endpoints(["tcp/127.0.0.1:7447"])
        .build()?;

    // Create a ROS 2 node
    let node = ctx.create_node("my_talker").build()?;

    // Create a publisher for the /chatter topic
    let pub_handle = node.create_pub::<RosString>("/chatter").build()?;

    // Publish messages every second
    let mut count = 0;
    loop {
        let msg = RosString {
            data: format!("Hello from hiroz #{}", count),
        };
        println!("Publishing: {}", msg.data);
        pub_handle.async_publish(&msg).await?;
        count += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
```

### 5. Run Your Application

Make sure the Zenoh router (`zenohd`) is running in another terminal, then:

```bash
cargo run
```

!!! success
    You should see the application publishing messages every second. It will continue until you press Ctrl+C.

### 6. Test with Multiple Nodes

Open another terminal and create a simple listener to verify communication:

**Create `src/bin/listener.rs`:**

```rust
use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ZContextBuilder::default()
        .with_connect_endpoints(["tcp/127.0.0.1:7447"])
        .build()?;
    let node = ctx.create_node("my_listener").build()?;
    let sub = node.create_sub::<RosString>("/chatter").build()?;

    println!("Listening on /chatter...");
    while let Ok(msg) = sub.async_recv().await {
        println!("Received: {}", msg.data);
    }
    Ok(())
}
```

**Run both:**

```bash
# Terminal 1: Router
zenohd

# Terminal 2: Publisher
cargo run

# Terminal 3: Listener
cargo run --bin listener
```

---

## Key Components

| Component | Purpose | Usage |
|-----------|---------|-------|
| **ZContextBuilder** | Initialize hiroz environment | Entry point, configure settings |
| **ZContext** | Manages ROS 2 connections | Create nodes from this |
| **Node** | Logical unit of computation | Publishers/subscribers attach here |
| **Publisher** | Sends messages to topics | `node.create_pub::<Type>("topic")` |
| **Subscriber** | Receives messages from topics | `node.create_sub::<Type>("topic")` |

!!! tip "Why a Zenoh router?"
    hiroz uses router-based discovery by default, aligning with ROS 2's official Zenoh middleware ([`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh)). This provides:
    - **Better scalability** for large deployments with many nodes
    - **Lower network overhead** compared to multicast discovery
    - **Production-ready** architecture used in real ROS 2 systems

    See the [Networking](../user-guide/networking.md) chapter for customization options.

## What's Happening?

```mermaid
sequenceDiagram
accTitle: Talker publishing messages through Zenoh to a Listener
accDescr: The Talker publishes Hello messages to the Zenoh network each second, which delivers each message to the Listener who prints it to the console.
    participant T as Talker
    participant Z as Zenoh Network
    participant L as Listener

    T->>Z: Publish "Hello 0"
    Z->>L: Deliver message
    L->>L: Print to console
    Note over T: Wait 1 second
    T->>Z: Publish "Hello 1"
    Z->>L: Deliver message
    L->>L: Print to console
```

The talker publishes messages every second to the `/chatter` topic. The listener subscribes to the same topic and prints each received message. Zenoh handles the network transport transparently.

!!! info
    Both nodes run independently. You can start/stop them in any order, and multiple listeners can receive from one talker simultaneously.

## Next Steps

Now that you understand the basics:

**Core Concepts:**

- **[Pub/Sub](../core-concepts/pubsub.md)** - Deep dive into pub-sub patterns and QoS
- **[Services](../core-concepts/services.md)** - Request-response communication
- **[Actions](../core-concepts/actions.md)** - Long-running tasks with feedback
- **[Message Generation](../user-guide/message-generation.md)** - How message types work
- **[Custom Messages](../user-guide/custom-messages.md)** - Define your own message types

**Development:**

- **[Building](./building.md)** - Build configurations and dependencies
- **[Networking](../user-guide/networking.md)** - Zenoh router setup and options
