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

Measured on the released `rmw_zenoh_cpp` 0.10.6 package, a ROS 2 Lyrical publisher treats an unbounded `uint8[]` or an unbounded `char[]` field as a buffer, directly or inside a nested message. An example is `sensor_msgs/PointCloud2`. An unbounded `byte[]`, an unbounded `int8[]`, a bounded `uint8[<=N]` and a fixed `uint8[N]` are not buffers. The rosidl type support generator makes this choice: it tests for an unbounded sequence of IDL `uint8`, and the adapter maps ROS `char` to IDL `uint8` and ROS `byte` to IDL `octet`. A matched subscriber can announce buffer support. For such a subscriber the publisher writes the message on `<topic key>/_buf_cpu` and never on `<topic key>`. For each matched subscriber that does not announce it, the publisher also writes `<topic key>/_buf/<gid>`. This comes from reading the `rmw_zenoh_cpp` source and is not measured. The `humble`, `jazzy` and `kilted` branches of `rmw_zenoh` carry no buffer support: they have no `backends` field in the liveliness key-expression code.

A hiroz subscriber joins this group when the end of its liveliness token is `backends:cpu:`. It then declares two exact subscriptions, one on the base key and one on the group key, both before the token. It receives no other key under the topic key. The payload is plain CDR. The publisher also writes `<topic key>/_buf/<gid>` for each matched subscriber outside the group. The joined subscriber has no subscription that matches those keys, so those writes do not reach its process. The base key stays subscribed because a publisher that is not buffer-aware writes only there.

`BufferGroup::Auto` is the default. A typed subscriber, a dynamic subscriber or a cache joins when its message has an unbounded `uint8[]` or `char[]`, directly or nested. A typed subscriber also joins for an unbounded `byte[]`, although `rmw_zenoh_cpp` does not: the typed code generator maps ROS `byte`, `uint8` and `char` to one type. This over-approximation costs one extra join and loses no data. A dynamic subscriber follows its schema, and a schema that `load_schema` reads from a `.msg` file keeps `byte` apart from `uint8`. A subscriber also joins when hiroz has no message definition to ask. The feedback and status subscribers of an action client have no message definition to ask, so they join under `Auto`. The feedback subscriber stays out when its QoS is transient-local. The Python bindings build a typed subscriber with no message definition, so they join under `Auto`. A subscriber with no definition joins on every topic it subscribes to, not only on buffer topics. It declares the second subscription and a token with the group field each time. To declare a token with no `backends:` field and one subscription on `<topic key>`, call `with_buffer_group(BufferGroup::Off)` on the subscriber builder or the cache builder. `BufferGroup::Cpu` joins whatever the message is, except for a transient-local subscriber or a custom key expression format. The action client and the Python bindings have no `Off` switch yet.

Raw subscribers do not join. The Go bindings create raw subscribers, so they do not join. A transient-local subscriber and a subscriber with a custom key expression format never join, on the default as well as under `Cpu`. A transient-local subscriber stays out because the released `rmw_zenoh_cpp` 0.10.6 package keeps it off the buffer endpoints and serves its history on the base key only. This is measured: five runs per cell, late joiners of three kinds, Image topics. The history arrived on the base key in every cell and never on the group key. A newer source revision that is not released serves history on the group key. A transient-local subscriber therefore reads its history from the base key. Delivery of history after the fix for legacy subscribers in [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078) is not tested. `rmw-zenoh-rs` subscriptions use `Off`: they do not join yet.

If you use access control, allow `<topic key>` and `<topic key>/_buf_cpu`. A joined subscriber declares only these two keys, so a rule for them is enough. A router that allows `<topic key>` and not `<topic key>/_buf_cpu` starves the group key of every joined subscriber of that topic. A buffer-aware publisher writes a buffer message on the group key only, so the subscriber receives nothing. No error is raised and `build` returns `Ok`. The base key still delivers. A subscriber set to `Off` declares `<topic key>` only and keeps receiving. A subscriber that stays out of the group declares `<topic key>` only and receives under the same rule. An example is a subscriber on a topic with no buffer. Two cases join on such a topic and the rule starves their group key: a subscriber with no message definition, and a subscriber set to `Cpu`. A Python or action-client user has no `Off` switch, so the only remedy for them is the router rule. hiroz logs each join at `debug` level and names both keys.

A subscriber with no `backends:` field still receives topics that carry no buffer through the base key. From a buffer-aware publisher it receives nothing for messages that carry a buffer, until [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078) is released. This covers `Off` subscribers, transient-local subscribers, subscribers with a custom key expression format and `rmw-zenoh-rs` subscriptions. This is measured for an `Off` subscriber only. The other three have no `backends:` field either, and no test runs them.

A joined subscriber can report lost-message events for messages that arrived, when other buffer-aware subscribers share the topic. The publisher advances one sequence number for each endpoint copy. The interop tests do not cover this limit, and hiroz code does not change for it.

| Limit | Cause | Measured | Fix |
|---|---|---|---|
| A joined subscriber reports lost-message events for messages that arrived | The publisher advances one sequence number for each endpoint copy | The lost-message count was K times the number of delivered messages for K other buffer-aware subscribers. This holds for the released 0.10.6 package and for an unreleased revision. K was 1 and 4, on one machine, with five runs per K and a 20 second window at 100 messages per second | The open fix [ros2/rmw_zenoh#1079](https://github.com/ros2/rmw_zenoh/pull/1079) gave a count of 0 |
