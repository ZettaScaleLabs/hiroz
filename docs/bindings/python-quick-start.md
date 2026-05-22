<!-- markdownlint-disable MD046 -->
# Python Quick Start

Get a Python publisher and subscriber running in five minutes. No Rust or ROS 2 installation required.

## Prerequisites

- Python 3.11+
- A Zenoh router running locally (covered in step 3)

## 1. Install hiroz-py

### Option A — Pre-built wheel (recommended)

Go to the [Releases page](https://github.com/ZettaScaleLabs/hiroz/releases), find the latest release, and substitute `<version>` below. Pick the tab for your ROS 2 distro — if you are new to hiroz and have no ROS 2 install, choose **Jazzy**.

=== "Jazzy / Kilted / Rolling"

    ```bash
    pip install https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/hiroz_msgs_py-<version>-py3-none-any.whl
    pip install https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/hiroz_py-<version>-0jazzy-cp311-abi3-linux_x86_64.whl
    ```

=== "Humble"

    ```bash
    pip install https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/hiroz_msgs_py-<version>-py3-none-any.whl
    pip install https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/hiroz_py-<version>-0humble-cp311-abi3-linux_x86_64.whl
    ```

!!! tip
    On **aarch64 Linux** (e.g. Raspberry Pi) replace `linux_x86_64` with `linux_aarch64`. On **macOS Apple Silicon** use `macosx_11_0_arm64`.

### Option B — Build from source

Requires Rust 1.85+ and `maturin`:

```bash
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz/crates/hiroz-py
python -m venv .venv
source .venv/bin/activate

# Install standard message types
pip install -e ../hiroz-msgs/python/

# Build and install the Python bindings
maturin develop
```

## 2. Write a Publisher

Create `talker.py`:

```python
import time
import hiroz_py
from hiroz_py import std_msgs

ctx = hiroz_py.ZContextBuilder().with_connect_endpoints(["tcp/127.0.0.1:7447"]).build()
node = ctx.create_node("py_talker").build()
pub = node.create_publisher("/chatter", std_msgs.String)

i = 0
while True:
    msg = std_msgs.String(data=f"Hello from Python #{i}")
    pub.publish(msg)
    print(f"Published: {msg.data}")
    i += 1
    time.sleep(1.0)
```

## 3. Write a Subscriber

Create `listener.py`:

```python
import hiroz_py
from hiroz_py import std_msgs

ctx = hiroz_py.ZContextBuilder().with_connect_endpoints(["tcp/127.0.0.1:7447"]).build()
node = ctx.create_node("py_listener").build()
sub = node.create_subscriber("/chatter", std_msgs.String)

print("Listening on /chatter...")
while True:
    msg = sub.recv(timeout=5.0)
    if msg is not None:
        print(f"Received: {msg.data}")
```

## 4. Run

### Start the Zenoh router

hiroz nodes communicate through a Zenoh router. Download `zenohd` from the [Eclipse Zenoh releases](https://github.com/eclipse-zenoh/zenoh/releases) or install it via cargo:

```bash
cargo install zenohd
```

Then start it:

```bash
# Terminal 1
zenohd
```

!!! tip
    For full router setup options (apt, brew, Docker, Windows), see [Networking](../user-guide/networking.md).

### Run the publisher and subscriber

```bash
# Terminal 2 — subscriber
python listener.py

# Terminal 3 — publisher
python talker.py
```

You should see the subscriber printing messages from the publisher.

## 5. Try the Built-in Examples

!!! note
    These examples live inside the cloned repository. If you installed via pip (Option A), clone the repo first:
    ```bash
    git clone https://github.com/ZettaScaleLabs/hiroz.git
    ```

```bash
cd hiroz
source crates/hiroz-py/.venv/bin/activate  # or your own venv if using Option A

# Pub/sub demo (pass --role talker or --role listener)
python crates/hiroz-py/examples/topic_demo.py --role talker
python crates/hiroz-py/examples/topic_demo.py --role listener

# Service demo
python crates/hiroz-py/examples/service_demo.py --role server
python crates/hiroz-py/examples/service_demo.py --role client

# Action demo
python crates/hiroz-py/examples/action_demo.py --role server
python crates/hiroz-py/examples/action_demo.py --role client
```

## What's Next

- **[Python Bindings](./python.md)** — full API: services, actions, SHM, QoS, context config
- **[Message Generation](../user-guide/message-generation.md)** — generate types from a full ROS 2 install
- **[ROS 2 Interoperability](../user-guide/interop.md)** — connect Python hiroz nodes to live ROS 2 nodes
