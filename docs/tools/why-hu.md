# Why hu?

`hu` is the command-line tool for the hiroz stack. If you already use `ros2 topic`, `ros2 node`, `ros2 service`, or `rqt`, this page explains what problem `hu` solves and why you might want it.

---

## The problem with existing CLI tools

ROS 2 ships two standard toolsets: `ros2cli` for the terminal and `rqt` for the GUI. Both are implemented in Python and built on top of RCL (the ROS Client Library). This means they carry the full cost of the Python ROS 2 stack — a background daemon, DDS discovery, Python message deserialization — even for the simplest operations.

Four concrete pain points come up regularly:

**1. The daemon is fragile.** `ros2cli` starts a background process (`_ros2_daemon`) on first use and caches the graph there. It snapshots your environment (`ROS_DOMAIN_ID`, `RMW_IMPLEMENTATION`) at startup. Change either in a new terminal and the daemon silently queries the wrong domain ([ros2cli#1238](https://github.com/ros2/ros2cli/issues/1238)). On enterprise networks it crashes after idle periods and every subsequent command returns nothing — the fix is always `pkill -f _ros2_daemon` ([ros2cli#502](https://github.com/ros2/ros2cli/issues/502), [ros2cli#702](https://github.com/ros2/ros2cli/issues/702)). In containers and WSL2 the health check fails entirely ([ros2cli#934](https://github.com/ros2/ros2cli/issues/934)).

**2. Rate measurement is inaccurate at speed.** `ros2 topic hz` deserializes every message in Python before counting it. Python's GIL means it can process at most a few thousand messages per second regardless of CPU speed. At high publish rates — motors, IMUs, fusion outputs, cameras — the reported rate is lower than reality. The `hu` test suite measures a ~120 kHz publisher: `ros2 topic hz` reports 3–5 kHz, `hu meter hz` reports the actual rate. For camera (30 fps) and lidar (10–20 Hz) topics both tools agree; the gap opens above ~500 Hz ([ros2cli#871](https://github.com/ros2/ros2cli/issues/871), [ros2cli#1043](https://github.com/ros2/ros2cli/issues/1043)).

**3. Output is not machine-readable.** Every `ros2` command emits human-formatted text. Parsing `ros2 topic list` requires splitting on `/` and filtering blank lines; parsing `ros2 topic info` requires column counting. Both break across ROS 2 versions. There is no stable structured output format.

**4. It requires a full ROS 2 install.** Using `ros2 topic hz` in a CI pipeline or on a developer laptop that doesn't have a ROS 2 distro installed requires either a Docker image with the full distro or manually sourcing `setup.bash`. `hu` is a single statically-linked binary with no ROS 2 runtime dependency.

---

## What hu does differently

`hu` connects directly to the Zenoh router that underlies the hiroz stack. It does not use RCL, does not start a daemon, and does not deserialize messages in Python.

**No daemon, always fresh.** Every `hu` invocation opens a Zenoh session, reads the live liveliness index, and exits. The result is always a real-time snapshot of the current network, not cached state from whenever the daemon last started.

**Byte-level subscription for measurement.** `hu meter hz` and `hu meter bw` subscribe at the raw Zenoh byte layer. They timestamp message arrivals without deserializing any payload. A 100 MB point cloud costs the same to count as a 10-byte string.

**JSON output on every command.** Every `hu` subcommand accepts `--json` and emits newline-delimited JSON. This makes it composable with `jq`, shell scripts, CI harnesses, and log pipelines without fragile text parsing:

```bash
# Check camera rate in CI
rate=$(hu meter hz /camera/image_raw --duration 5 --json | jq '.rate_hz')
[ "$(echo "$rate > 28.0" | bc)" = "1" ] || exit 1

# Stream graph events to a log file
hu monitor watch --json >> /var/log/ros-graph-events.jsonl
```

**Plugin extensibility without packaging.** Any binary named `hu-<name>` on `PATH` becomes a `hu <name>` subcommand. No Python entry-points, no `setup.cfg`, no shared runtime state. A team-specific diagnostics tool or recorder is one executable away.

---

## What hu does not do

`hu` only works with the hiroz stack and `rmw_zenoh_cpp`. It cannot see nodes that use `rmw_fastrtps_cpp` or `rmw_cyclonedds_cpp`. If your system has a mix of RMW implementations, `ros2 topic hz` will see topics that `hu meter hz` cannot.

There is no `hu launch`, no `hu pkg`, and no `hu run`. Launch is orthogonal to inspection tooling; package management is a build-system concern. `hu` is scoped to graph introspection, measurement, and bridging — the operations that the Python-based tools do poorly at scale.

---

## When to switch

Use `hu` when:

- You run `rmw_zenoh_cpp` or pure hiroz nodes
- You need accurate rate measurement above 500 Hz
- You use CLI tools in CI scripts or automation
- You work in containers or environments without a ROS 2 install
- You want live graph events without polling

Keep `ros2cli` when:

- Your nodes use `rmw_fastrtps_cpp` or `rmw_cyclonedds_cpp`
- You need `ros2 launch`, `ros2 pkg`, or `ros2 run`
- You are working with a codebase that is not yet on the Zenoh stack

---

## Next steps

- [hu reference](hu.md) — full command reference
- [hu vs. ros2cli / rqt](hu-vs-ros2cli.md) — feature-by-feature comparison with benchmark data
