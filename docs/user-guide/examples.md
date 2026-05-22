# Running Examples

!!! note
    The examples described here are part of the hiroz repository. To run them, you must first clone the repository.

```bash
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz
```

## Start the Eclipse Zenoh Router

All examples require a Zenoh router to be running first (see [Networking](./networking.md) for why hiroz uses router-based architecture by default).

## From the hiroz Repository

If you're working in the hiroz repository, use the included router example:

```bash
cargo run --example zenoh_router
```

## From Your Own Project

If you're working on your own project, you need to install a Zenoh router. Quick options:

```bash
# Using cargo
cargo install zenohd

# Using Docker
docker run --init --net host eclipse/zenoh:latest

# Using apt (Ubuntu/Debian)
echo "deb [trusted=yes] https://download.eclipse.org/zenoh/debian-repo/ /" | sudo tee /etc/apt/sources.list.d/zenoh.list
sudo apt update && sudo apt install zenoh
```

Then run:

```bash
zenohd
```

!!! tip
    See the comprehensive [Zenoh Router Installation Guide](./networking.md#running-the-zenoh-router) for all installation methods including pre-built binaries, package managers, and more.

---

## Available Examples

Leave the router running in a separate terminal, then run any example from the hiroz repository root:

| Example | What it demonstrates | Command |
|---------|---------------------|---------|
| `z_pubsub` | Basic publisher + subscriber exchanging `std_msgs/String` | `cargo run --example z_pubsub` |
| `z_srvcli` | Service server + client using `example_interfaces/AddTwoInts` | `cargo run --example z_srvcli` |
| `z_custom_message` | Custom `.msg` types without `hiroz-msgs`; shows the codegen workflow | `cargo run --example z_custom_message -- --mode status-pub` |
| `twist_pub` | Publishing `geometry_msgs/Twist` (common for robot velocity commands) | `cargo run --example twist_pub` |
| `battery_state_sub` | Subscribing to `sensor_msgs/BatteryState` | `cargo run --example battery_state_sub` |
| `z_parameter_declare` | Local parameter declare/get/set/undeclare flow | `cargo run --example z_parameter_declare` |
| `z_parameter_callback` | Validation callback flow for parameter changes | `cargo run --example z_parameter_callback` |
| `z_parameter_yaml` | YAML parameter loading plus programmatic overrides | `cargo run --example z_parameter_yaml` |
| `z_parameter_client` | Remote `ParameterClient` calls against a parameter server | `cargo run --example z_parameter_client` |
| `zenoh_router` | Embedded Zenoh router for development without installing `zenohd` | `cargo run --example zenoh_router` |

!!! tip
    For a detailed walkthrough of creating your own project with hiroz (not using the repository examples), see the [Quick Start](../getting-started/quick-start.md#option-2-create-your-own-project) guide.

---

## Demo Nodes

The `demo_nodes` examples are basic ROS 2 patterns referenced from [ROS 2 demo_nodes_cpp](https://github.com/ros2/demos/tree/rolling/demo_nodes_cpp). These demonstrate fundamental pub/sub and service patterns.

### Talker (Publisher)

A simple publisher node that publishes "Hello World" messages to the `/chatter` topic.

**Features:**

- Publishes `std_msgs/String` messages
- Uses async API for non-blocking publishing
- Publishes at 1 Hz (configurable)
- Uses QoS history depth of 7 (KeepLast)

**Usage:**

```bash
# Default settings (topic: chatter, period: 1.0s)
cargo run --example demo_nodes_talker

# Custom topic and faster publishing
cargo run --example demo_nodes_talker -- --topic /my_topic --period 0.5

# Connect to specific Zenoh router
cargo run --example demo_nodes_talker -- --endpoint tcp/localhost:7447
```

**Command-line Options:**

| Option | Description | Default |
|--------|-------------|---------|
| `-t, --topic <TOPIC>` | Topic name to publish to | `chatter` |
| `-p, --period <PERIOD>` | Publishing period in seconds | `1.0` |
| `-m, --mode <MODE>` | Zenoh session mode (peer/client/router) | `peer` |
| `-e, --endpoint <ENDPOINT>` | Zenoh router endpoint to connect to | - |

### Listener (Subscriber)

A simple subscriber node that listens to messages on a topic.

**Features:**

- Subscribes to `std_msgs/String` messages
- Uses async API for receiving messages
- Uses QoS history depth of 10 (KeepLast)
- Prints received messages to console

**Usage:**

```bash
# Default settings (topic: chatter)
cargo run --example demo_nodes_listener

# Custom topic
cargo run --example demo_nodes_listener -- --topic /my_topic

# Connect to specific Zenoh router
cargo run --example demo_nodes_listener -- --endpoint tcp/localhost:7447
```

**Command-line Options:**

| Option | Description | Default |
|--------|-------------|---------|
| `-t, --topic <TOPIC>` | Topic name to subscribe to | `chatter` |
| `-m, --mode <MODE>` | Zenoh session mode (peer/client/router) | `peer` |
| `-e, --endpoint <ENDPOINT>` | Zenoh router endpoint to connect to | - |

**Test the pub/sub pattern:**

Terminal 1:

```bash
cargo run --example demo_nodes_talker
```

Terminal 2:

```bash
cargo run --example demo_nodes_listener
```

Expected output in Terminal 2:

```text
I heard: [Hello World: 1]
I heard: [Hello World: 2]
I heard: [Hello World: 3]
...
```

### Add Two Ints Server

A simple service server that adds two integers and returns the result.

**Features:**

- Provides `example_interfaces/AddTwoInts` service
- Handles addition requests synchronously
- Uses async API for service handling
- Prints received requests and sent responses

**Usage:**

```bash
# Default settings (handles unlimited requests)
cargo run --example demo_nodes_add_two_ints_server

# Handle only one request and exit
cargo run --example demo_nodes_add_two_ints_server -- --count 1

# Connect to specific Zenoh router
cargo run --example demo_nodes_add_two_ints_server -- --endpoint tcp/localhost:7447
```

**Command-line Options:**

| Option | Description | Default |
|--------|-------------|---------|
| `-c, --count <COUNT>` | Number of requests to handle before exiting (0 for unlimited) | `0` |
| `-m, --mode <MODE>` | Zenoh session mode (peer/client/router) | `peer` |
| `-e, --endpoint <ENDPOINT>` | Zenoh router endpoint to connect to | - |

### Add Two Ints Client

A simple service client that sends addition requests to the server.

**Features:**

- Calls `example_interfaces/AddTwoInts` service
- Supports both synchronous and asynchronous response waiting
- Uses async API for client operations
- Configurable numbers to add

**Usage:**

```bash
# Default settings (sync mode, adds 2 + 3)
cargo run --example demo_nodes_add_two_ints_client

# Async mode
cargo run --example demo_nodes_add_two_ints_client -- --async-mode

# Custom numbers
cargo run --example demo_nodes_add_two_ints_client -- --a 10 --b 20

# Async mode with custom numbers
cargo run --example demo_nodes_add_two_ints_client -- --a 10 --b 20 --async-mode

# Connect to specific Zenoh router
cargo run --example demo_nodes_add_two_ints_client -- --endpoint tcp/localhost:7447
```

**Command-line Options:**

| Option | Description | Default |
|--------|-------------|---------|
| `-a, --a <A>` | First number to add | `2` |
| `-b, --b <B>` | Second number to add | `3` |
| `--async-mode` | Use asynchronous response waiting | `false` |
| `-m, --mode <MODE>` | Zenoh session mode (peer/client/router) | `peer` |
| `-e, --endpoint <ENDPOINT>` | Zenoh router endpoint to connect to | - |

**Test the service pattern:**

Terminal 1 (Server):

```bash
cargo run --example demo_nodes_add_two_ints_server -- --count 1
```

Terminal 2 (Client - sync mode):

```bash
cargo run --example demo_nodes_add_two_ints_client
```

Or Terminal 2 (Client - async mode):

```bash
cargo run --example demo_nodes_add_two_ints_client -- --async-mode
```

**Expected output (Client):**

```text
AddTwoInts service client started (mode: sync/async)
Sending request: 2 + 3
Received response: 5
Result: 5
```

**Expected output (Server):**

```text
AddTwoInts service server started
Received request: 2 + 3
Sending response: 5
```

---

## Custom Messages Demo

This example demonstrates how to generate Rust types from user-defined ROS 2 message definitions. See the [Custom Messages](./custom-messages.md) chapter for comprehensive documentation.

**Quick start:**

```bash
cd crates/hiroz/examples/custom_msgs_demo
HIROZ_MSG_PATH="./my_robot_msgs" cargo build
```

---

## Protobuf Demo

This example demonstrates using protobuf serialization with hiroz, both for ROS messages and custom protobuf messages. See the [Protobuf Serialization](./protobuf.md) chapter for comprehensive documentation.

**Quick start:**

```bash
cd crates/hiroz/examples/protobuf_demo
cargo run
```

---

## Parameter Examples

These focused examples demonstrate the hiroz parameter subsystem without a mode-switching wrapper. See the [Parameters](../core-concepts/parameters.md) chapter for comprehensive documentation.

**Quick start:**

```bash
cargo run --example z_parameter_declare
cargo run --example z_parameter_callback
cargo run --example z_parameter_yaml
cargo run --example z_parameter_client
```
