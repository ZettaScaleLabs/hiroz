# ROS 2 Interoperability

hiroz nodes — whether written in Rust, Python, or Go — speak the same Eclipse Zenoh wire protocol as [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh), the official ROS 2 middleware plugin for Zenoh. This means they interoperate transparently: a Go subscriber can receive messages from a ROS 2 C++ talker, a Python publisher can send to a Rust listener, and so on.

!!! tip "Mixing Humble with Jazzy, Kilted, or Lyrical?"
    If you need to bridge a **Humble** (legacy) network and a **Jazzy / Kilted / Lyrical** (modern) network, use `hiroz-bridge` from [hiroz-toolkit](https://github.com/ZettaScaleLabs/hiroz-toolkit). See the [Cross-Distro Bridge](./bridge.md) chapter for details.

## Prerequisites

- A ROS 2 installation with [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) and `demo_nodes_cpp`
- A Zenoh router running on `localhost:7447` (see [Networking](./networking.md))

## How It Works

All participants connect to the same Zenoh router on the local machine.

```mermaid
graph LR
accTitle: ROS 2 interop via shared Zenoh router with Rust, Python, and Go
accDescr: A ROS 2 C++ talker connects through rmw_zenoh_cpp to a local Zenoh router, which also serves hiroz nodes written in Rust, Python, and Go.
    talker["demo_nodes_cpp<br>talker"] --> rmw["rmw_zenoh_cpp"] --> router(["Zenoh router<br>localhost:7447"])
    router <-->|Zenoh| rust["Rust<br>(hiroz)"]
    router <-->|Zenoh| python["Python<br>(hiroz-py)"]
    router <-->|Zenoh| go["Go<br>(hiroz-go)"]
```

!!! note
    The Zenoh router can be [`rmw_zenohd`](https://github.com/ros2/rmw_zenoh), `zenohd`, `hu router`, or Docker.
    See [Networking](./networking.md) for all options.

**Requirements for successful message exchange:**

- Both sides must use the same message type with matching RIHS01 type hashes
- ROS 2 nodes must use [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) (`export RMW_IMPLEMENTATION=rmw_zenoh_cpp`)

!!! warning
    If type hashes differ (e.g. mismatched message definitions), nodes silently drop messages.
    Enable `RUST_LOG=hiroz=debug` to inspect the hash in the key expression and compare with the ROS 2 side.
    See [Troubleshooting](../reference/troubleshooting.md) for diagnosis steps.

## Setup

```bash
# Terminal 1 — router
ros2 run rmw_zenoh_cpp rmw_zenohd

# Terminal 2 — ROS 2 talker
export RMW_IMPLEMENTATION=rmw_zenoh_cpp
ros2 run demo_nodes_cpp talker

# Terminal 3 — hiroz listener (see Rust / Python / Go below)
```

---

## Rust

```bash
cargo run --example demo_nodes_listener
```

For more detail on publisher/subscriber patterns see [Pub/Sub](../core-concepts/pubsub.md).

---

## Python

If you installed via **pip** (pre-built wheel), write your own `listener.py` using the code in [Python Quick Start](../bindings/python-quick-start.md) and run it with `python listener.py`.

If you cloned the repository (build from source), use the bundled example:

```bash
cd crates/hiroz-py
source .venv/bin/activate
python examples/topic_demo.py -r listener
```

Or go the other way — Python publishes, ROS 2 listens:

```bash
python examples/topic_demo.py -r talker
# On ROS 2 side:
ros2 topic echo /chatter std_msgs/msg/String
```

For more detail see [Python Bindings](../bindings/python.md).

---

## Go

```bash
just -f crates/hiroz-go/justfile run-example subscriber
```

For more detail see [Go Quick Start](../bindings/go-quick-start.md).

---

## Buffer messages from Lyrical publishers

A ROS 2 Lyrical `rmw_zenoh_cpp` publisher treats an unbounded `uint8[]` or `byte[]` field as a buffer, also inside a nested message. Examples are `sensor_msgs/Image` and `sensor_msgs/PointCloud2`. If a matched subscriber announces buffer support, the publisher writes the message on `<topic key>/_buf_cpu` and never on `<topic key>`. For each matched subscriber that does not announce it, the publisher also writes `<topic key>/_buf/<gid>`. The `humble`, `jazzy` and `kilted` branches of `rmw_zenoh` carry no buffer support (they have no `backends` field in the liveliness key-expression code), so the interop tests that need one skip on those distros.

A hiroz subscriber joins this group when the end of its liveliness token is `backends:cpu:`. It then reads `<topic key>/**`, which includes the base key and the group key, and drops every other key under the topic key in process. The payload is plain CDR. The publisher writes a full payload on `<topic key>/_buf/<gid>` for each matched subscriber outside the group. The wide subscription receives those writes and drops them, so with K such subscribers a payload reaches the joined process K extra times per message. This follows from the subscription pattern and is not measured.

`BufferGroup::Auto` is the default. A typed subscriber, a dynamic subscriber or a cache joins when its message has an unbounded `uint8[]` or `byte[]`, directly or nested, or when hiroz has no message definition to ask. The feedback and status subscribers of an action client have no message definition to ask, so they join under `Auto`, unless their QoS is transient-local. The Python bindings build a typed subscriber with no message definition, so they join under `Auto`. A subscriber with no definition joins on every topic it subscribes to, not only on buffer topics, and declares the wider subscription and the token each time. To keep the previous token and routing, call `with_buffer_group(BufferGroup::Off)` on the subscriber builder or the cache builder. `BufferGroup::Cpu` joins whatever the message is, except for a transient-local subscriber or a custom key expression format. The action client and the Python bindings have no `Off` switch yet.

Raw subscribers do not join. The Go bindings create raw subscribers, so they do not join. A transient-local subscriber and a subscriber with a custom key expression format never join. `rmw-zenoh-rs` subscriptions use `Off`: they do not join yet.

If you use access control, allow `<topic key>/**` wherever you allow `<topic key>`. A joined subscriber declares `<topic key>/**`. A router that allows `<topic key>` and not `<topic key>/**` therefore starves every joined subscriber of that topic, on the base key too: no error is raised and `build` returns `Ok`. A subscriber set to `Off` declares `<topic key>` and keeps receiving. This holds for a subscriber with no message definition on any topic, buffer topic or not. A router rule that denies only the group key starves a joined subscriber on a buffer topic, and the base key still delivers. A rule on `<topic key>/**` also starves a subscriber set to `Off`, because `<topic key>/**` includes `<topic key>`. A Python or action-client user has no `Off` switch, so the only remedy for them is the router rule. hiroz writes one `info` log line per group key and process when a subscriber joins.

A subscriber with no `backends:` field receives nothing from a buffer-aware publisher until [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078) is released. This covers `Off` subscribers, transient-local subscribers, subscribers with a custom key expression format and `rmw-zenoh-rs` subscriptions.
