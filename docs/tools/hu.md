# hu — The hiroz Unified Tool

`hu` is the command-line companion to the hiroz stack. It replaces `ros2 topic`, `ros2 node`, `ros2 service`, `ros2 action`, and `ros2 param` with a single daemon-free binary that works directly over Zenoh — no DDS, no Python, no background process.

## Quick Start

```bash
# Connect to a local router and list all topics
hu meter list topics

# Measure publish rate
hu meter hz /camera/image_raw

# Show node introspection
hu meter info node /my_robot

# Monitor the live graph
hu monitor watch
```

By default `hu` connects to `tcp/127.0.0.1:7447`. Override with `--router`:

```bash
hu --router tcp/192.168.1.10:7447 meter list topics
```

Or set it once for the session:

```bash
export HU_ROUTER=tcp/192.168.1.10:7447
hu meter hz /lidar/scan
```

---

## Why hu instead of ros2cli?

`ros2cli` is the standard ROS 2 command-line tool. It works, but it carries a set of well-known pain points that `hu` was built to eliminate.

### No daemon

`ros2cli` spawns a background daemon process (`_ros2_daemon`) on first use and caches graph state there. This causes several failure modes that users hit regularly:

- **Stale domain ID**: the daemon snapshots your environment at startup. If you change `ROS_DOMAIN_ID` or `RMW_IMPLEMENTATION` in a new terminal, the daemon silently queries the wrong domain ([ros2cli#1238](https://github.com/ros2/ros2cli/issues/1238)).
- **Daemon crashes**: on enterprise networks with strict firewall rules, the daemon dies silently after 1–3 hours and all CLI commands return empty results until you kill it manually ([ros2cli#502](https://github.com/ros2/ros2cli/issues/502)).
- **WSL2 / container incompatibility**: the daemon health check fails in certain WSL2 configurations, making `ros2 topic list` unusable ([ros2cli#934](https://github.com/ros2/ros2cli/issues/934)).
- **Manual recovery**: the only fix is `pkill -f _ros2_daemon` then re-running your command ([ros2cli#702](https://github.com/ros2/ros2cli/issues/702)).

`hu` has no daemon. Every invocation connects directly to the Zenoh router, reads the current graph state, and exits. It always reflects the real current state.

### Accurate rate measurement

`ros2 topic hz` deserializes every message in Python before counting it. At high publish rates or with large messages (cameras, lidar, point clouds), deserialization can't keep up and the reported rate is lower than the real rate:

- A 30 fps camera reports 15–22 fps ([ros2cli#871](https://github.com/ros2/ros2cli/issues/871))
- At ~2 kHz the reported rate saturates — Python simply cannot process arrivals fast enough ([ros2cli#1043](https://github.com/ros2/ros2cli/issues/1043))
- Large messages (5 MB+) compound the problem regardless of `PYTHONOPTIMIZE` ([ros2cli#843](https://github.com/ros2/ros2cli/issues/843))

`hu meter hz` subscribes at the raw Zenoh byte level — no deserialization, no Python overhead. It counts message arrivals with nanosecond timestamps and reports the actual rate.

```bash
# ros2 topic hz /camera/image_raw  → "average rate: 17.3"  (real: 30 fps)
# hu meter hz /camera/image_raw    → "rate: 30.001 Hz"
```

### QoS visibility

`ros2 topic echo` silently drops messages when the subscriber's QoS is incompatible with the publisher's — there is no warning ([ros2cli#593](https://github.com/ros2/ros2cli/issues/593)). You see no output and assume the topic is empty.

`hu meter echo` uses hiroz's QoS event system and prints a warning when a mismatch is detected:

```text
[warn] QoS incompatible: publisher on /scan uses BEST_EFFORT, subscriber expects RELIABLE
```

### Service call timeout

`ros2 service call` blocks indefinitely when no server is available — there is no `--timeout` flag and no way to interrupt it cleanly without killing the process ([ros2cli#818](https://github.com/ros2/ros2cli/issues/818)).

`hu meter service call` has a `--timeout <secs>` flag (default: 10s) and returns a clear error:

```bash
hu meter service call /my_srv --payload "00 00 00 00" --timeout 5
# Error: Service call timed out after 5s
```

### Fast startup

`ros2 --help` takes 7+ seconds on Raspberry Pi 2 due to Python import overhead ([ros2cli#424](https://github.com/ros2/ros2cli/issues/424)). `hu --help` is a compiled binary — startup is under 10 ms on any hardware.

### Topic publish with nested messages

`ros2 topic pub` populates message fields from YAML, but it has no type information at encoding time. Nested array fields and non-primitive array elements fail silently or error out ([ros2cli#59](https://github.com/ros2/ros2cli/issues/59), [ros2cli#191](https://github.com/ros2/ros2cli/issues/191)).

`hu meter pub` accepts `--yaml` with `--msg-type` and encodes directly to CDR using the known message layout:

```bash
hu meter pub /cmd_vel \
  --msg-type geometry_msgs/msg/Vector3 \
  --yaml '{x: 0.5, y: 0.0, z: 0.0}'
```

For std_msgs primitive types, all fields are supported:

```bash
hu meter pub /enable --msg-type std_msgs/msg/Bool --yaml '{data: true}'
hu meter pub /count  --msg-type std_msgs/msg/Int32 --yaml '{data: 42}'
```

---

## Summary

| Pain point | ros2cli | hu |
|---|---|---|
| Daemon crashes / stale state | ❌ common | ✅ no daemon |
| Rate measurement accuracy | ❌ Python deserialization bottleneck | ✅ raw Zenoh bytes |
| QoS mismatch warning | ❌ silent drop | ✅ explicit warning |
| Service call timeout | ❌ hangs forever | ✅ `--timeout` flag |
| Startup time (embedded HW) | ❌ 7+ seconds | ✅ <10 ms |
| Nested YAML in topic pub | ❌ fails silently | ✅ CDR-aware encoding |
| Works without ROS 2 install | ❌ requires full ROS 2 | ✅ only needs a Zenoh router |

---

## Subcommands

### hu meter

Measurement and introspection:

| Command | Description |
|---|---|
| `hu meter hz <topic>` | Publish rate (sliding window) |
| `hu meter bw <topic>` | Bandwidth in bytes/sec |
| `hu meter echo <topic>` | Print arriving messages |
| `hu meter delay <topic>` | End-to-end latency |
| `hu meter pub <topic>` | Publish a message |
| `hu meter list topics\|nodes\|services\|actions` | Enumerate graph entities |
| `hu meter info topic\|node\|service <name>` | Full entity introspection |
| `hu meter service call <name>` | Call a service |
| `hu meter param get\|set\|list <node>` | Read/write node parameters |
| `hu meter action list\|info\|send-goal <name>` | Action introspection and goal dispatch |

### hu monitor

Observation and diagnostics:

| Command | Description |
|---|---|
| `hu monitor watch` | Stream live graph change events |
| `hu monitor graph` | Snapshot the current graph (with optional `--watch` refresh) |
| `hu monitor log` | Tail `/rosout` with level and node filters |
| `hu monitor log-level get\|set <node>` | Read or change a node's logger level |

### hu bridge

Cross-distro and cross-DDS bridging — see [Cross-Distro Bridge](../user-guide/bridge.md).

---

## Multi-topic Rate Dashboard

For continuous monitoring of multiple topics at once, use the `hu` TUI. It shows a live rate table for all active topics, updated every second, without spawning one process per topic:

```bash
hu
```

This is the primary advantage over `ros2 topic hz`, which requires a separate terminal per topic.

---

## JSON Output

Every `hu meter` subcommand accepts `--json` for scripting:

```bash
hu meter hz /scan --duration 5 --json | jq '.rate_hz'
hu meter list topics --json | jq '.[].name'
hu meter info node /talker --json | jq '.publishers[].name'
```
