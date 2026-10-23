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

A buffer-aware ROS 2 Lyrical publisher treats an unbounded `uint8[]` or `char[]` field as a buffer, directly or inside a nested message. An example is `sensor_msgs/PointCloud2`. For a matched subscriber that announces buffer support, the publisher writes the message on `<topic key>/_buf_cpu` and never on `<topic key>`. The `humble`, `jazzy` and `kilted` branches of `rmw_zenoh` have no buffer support: their liveliness key-expression code has no `backends` field.

| field | buffer |
|---|---|
| unbounded `uint8[]`, unbounded `char[]` | yes |
| unbounded `byte[]`, unbounded `int8[]`, bounded `uint8[<=N]`, fixed `uint8[N]` | no |

The released `rmw_zenoh_cpp` 0.10.6 package gave this table. Each field ran alone and nested, with n=3 runs.

A hiroz subscriber joins this group when its liveliness token ends in `backends:cpu:`. A joined subscriber declares two exact subscriptions, on `<topic key>` and on `<topic key>/_buf_cpu`, both before the token. The base key stays subscribed because a publisher that is not buffer-aware writes only that key. The publisher also writes keys such as `<topic key>/_buf/<gid>` for subscribers outside the group. A joined subscriber has no subscription that matches them. This is read from the source and is not measured.

Set the choice with `with_buffer_group` on a subscriber builder or a cache builder:

| setting | the subscriber joins when |
|---|---|
| `BufferGroup::Auto` (default) | the message has an unbounded `uint8[]` or `char[]`, directly or nested, or hiroz has no message schema |
| `BufferGroup::Cpu` | always, except in the cases below |
| `BufferGroup::Off` | never |

- The typed code generator maps ROS `byte`, `uint8` and `char` to one type. A typed subscriber therefore also joins for an unbounded `byte[]`. This costs one extra join and loses no data.
- A subscriber with no message schema joins on every topic, not only on buffer topics. The feedback subscriber of an action client and the Python bindings have no message schema. The Python `create_subscriber` therefore joins on every topic under its default `buffer_group='auto'`, unless it uses transient-local QoS or a key format other than `rmw_zenoh`'s. `'auto'` and `'cpu'` do the same in Python. The argument accepts `'auto'`, `'cpu'` and `'off'`, and any other string raises `ValueError`. The status subscriber of an action client is always `Off`, because its message has no buffer.
- The feedback subscriber of an action client has no `Off` switch yet.
- A typed subscriber that stays out keeps its liveliness token, without the `backends:cpu:` part, and subscribes to the base key only. A raw subscriber that stays out declares no token.
- A raw subscriber has no message schema, so `Auto` joins it. `create_raw_subscriber` and `create_raw_subscriber_with_qos` join, and so do the C and Go bindings that call them. `create_raw_subscriber_with_group` takes `Auto`, `Cpu` or `Off`. It exists only with the `ffi` feature, and the Go bindings have no entry point for it yet. The feedback subscriber of a raw action client reads nothing and stays out.
- `RawSubscriber` has no public field. In release 0.2.0 the `inner` field is public, with the `ffi` feature. A caller of `raw.inner.undeclare().wait()` must call `raw.undeclare()` now. That call stops the token and both subscriptions. Dropping the wrapper stops the same handles.
- A transient-local subscriber never joins. A subscriber on a key format other than `rmw_zenoh`'s never joins. This holds under every setting.
- `rmw-zenoh-rs` subscriptions use `Off`.

A transient-local subscriber reads its history from `<topic key>`. The released 0.10.6 package gave this result in 75 cells with Image topics: five kinds of late joiner, three publish-time situations, n=5 per cell. The history arrived on `<topic key>` in every cell and never on `<topic key>/_buf_cpu`. An unreleased source revision serves it on the group key. No test covers history after [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078).

If you use router access control, allow `<topic key>` and `<topic key>/_buf_cpu`. A joined subscriber declares only these two keys, so a rule on `<topic key>/**` is not needed. A rule that allows `<topic key>` and not `<topic key>/_buf_cpu` stops a joined subscriber from receiving buffer messages. `build` returns `Ok` and no error appears. The base key still delivers. A subscriber receives a message that carries no buffer through the base key, so a rule on the group key does not affect that message. This holds for a subscriber with no message schema and for a subscriber set to `Cpu` on a topic that carries no buffer. `Off` stops a subscriber from joining, so `Off` needs no rule for the group key. A subscriber with no `backends:` field receives no buffer message from a buffer-aware publisher until a release carries [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078). `Off` therefore does not bring those messages back. hiroz logs each join at `debug` level and names both keys.

A subscriber with no `backends:` field still receives topics that carry no buffer through the base key. A buffer-aware publisher writes a message that carries a buffer on the group key only, so this subscriber receives nothing for it. The fix in [ros2/rmw_zenoh#1078](https://github.com/ros2/rmw_zenoh/pull/1078) is not released. This limit covers `Off` subscribers, transient-local subscribers, subscribers on a key format other than `rmw_zenoh`'s and `rmw-zenoh-rs` subscriptions. Measurements cover an `Off` subscriber only. No test runs the other three.

| Limit | Cause | Measured | Fix |
|---|---|---|---|
| A joined subscriber reports lost-message events for messages that arrived | The publisher advances one sequence number for each endpoint copy | The lost count was K times the delivered messages for K other buffer-aware subscribers. K was 1 and 4, n=5 per K, one machine, 100 messages per second for 20 s, released 0.10.6 package. An unreleased revision gave the same. | The open fix [ros2/rmw_zenoh#1079](https://github.com/ros2/rmw_zenoh/pull/1079) gave a count of 0 (n=5 per K) |

No hiroz test covers this limit, and hiroz code does not change for it.
