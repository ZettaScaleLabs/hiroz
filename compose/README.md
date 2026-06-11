# docker-compose interop harness

End-to-end RED/GREEN gates for hiroz ↔ ROS 2 interop across container
boundaries. Unlike the single-container interop tests in CI
(`.github/workflows/test.yml`), this harness runs hiroz in its **own**
container — a stand-in for a Raspberry Pi or similar edge device — talking
idiomatic zenoh over the network to a **separate** container with a full
ROS 2 Jazzy install.

## Topology

```text
┌─────────────────────┐      ┌──────────────────┐      ┌─────────────────────────┐
│  hiroz (testbed)    │      │  router          │      │  gates                  │
│  "Pi analogue"      │─────▶│  rmw_zenohd      │◀─────│  ROS 2 Jazzy +          │
│  compose_testbed    │ tcp/ │  (rmw_zenoh_cpp) │ tcp/ │  rmw_zenoh_cpp          │
│  zenoh client mode  │ 7447 │                  │ 7447 │  runs /gates/*.sh       │
└─────────────────────┘      └──────────────────┘      └─────────────────────────┘
```

- **router** — `ros2 run rmw_zenoh_cpp rmw_zenohd`, the documented rmw_zenoh
  deployment. Using rmw_zenoh's own router keeps the router version matched
  to the rmw side (jazzy currently bundles zenoh-c 1.6.x while hiroz uses
  zenoh 1.9.x; a hiroz-side 1.9.x router would need the `gateway/south`
  workaround — see `crates/hiroz-tests/tests/common/mod.rs`).
- **hiroz** — runs `crates/hiroz/examples/compose_testbed.rs`: talker,
  ping→pong echo, AddTwoInts server, AddTwoInts client (polling a
  ROS 2-hosted server), Fibonacci action server, and a parameter node.
- **gates** — runs `gates/run_gates.sh`; multicast scouting is disabled
  everywhere, so the only rendezvous is `tcp/router:7447`.

## Gates

| Gate | What it proves |
|------|----------------|
| `01_topics` | hiroz talker → `ros2 topic echo /chatter`; `ros2 topic pub /ping` → hiroz echo → `/pong` |
| `02_services` | `ros2 service call /add_two_ints` against the hiroz server; hiroz client calls a ROS 2-hosted `demo_nodes_cpp` server |
| `03_parameters` | `ros2 param list/get/set` against the hiroz `param_node` |
| `04_actions_graph` | `ros2 node list`/`topic list` see hiroz entities via liveliness; full `ros2 action send_goal /fibonacci` goal→feedback→result |

Each gate prints `GREEN`/`RED` per check; `run_gates.sh` exits with the
number of failed gates, which `--exit-code-from gates` propagates.

## Running locally

```bash
docker compose -f compose/docker-compose.yml build
docker compose -f compose/docker-compose.yml up --no-build --exit-code-from gates gates
echo $?    # 0 == all gates GREEN
docker compose -f compose/docker-compose.yml down -v --remove-orphans
```

To watch the other side, tail the testbed: `docker compose -f
compose/docker-compose.yml logs -f hiroz router`.

### Simulating a RED

- `docker compose -f compose/docker-compose.yml stop hiroz` mid-run → every
  gate goes RED.
- Point the testbed at a wrong port in `docker-compose.yml` → the hiroz
  healthcheck never passes and `depends_on` blocks the gates (non-zero exit).

## arm64 (Raspberry Pi architecture fidelity)

By default the hiroz container runs `linux/amd64`. To run it as
`linux/arm64` (cross-compiled with cargo-zigbuild; only the runtime stage is
emulated):

```bash
docker run --privileged --rm tonistiigi/binfmt --install arm64   # once per host
HIROZ_PLATFORM=linux/arm64 docker compose -f compose/docker-compose.yml build hiroz
HIROZ_PLATFORM=linux/arm64 docker compose -f compose/docker-compose.yml up --no-build --exit-code-from gates gates
```

In CI this is exposed as the `platform` input of the **Compose Interop**
workflow (`workflow_dispatch`); PR/push runs stay native for speed.

## CI

`.github/workflows/compose-interop.yml` pre-builds both images with buildx
(GitHub Actions layer cache), runs the harness with
`--exit-code-from gates`, dumps all container logs on failure, and tears
down the stack.

## Pinning rmw_zenoh_cpp

The ROS 2 image installs the latest `ros-jazzy-rmw-zenoh-cpp` on purpose so
the harness flags upstream regressions early. If a known-bad version lands,
pin it via the build arg: `RMW_ZENOH_VERSION="=<version>" docker compose
... build` (see `docker/Dockerfile.ros2`).
