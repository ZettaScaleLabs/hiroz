# Go Interop Tests

The Go binding test suite is tiered by dependency level.  Each tier is independent — you can run pure tests without any system dependencies and add heavier tiers as the environment allows.

## Test Tiers

| Tier | Build tag | What it tests | Requires |
|------|-----------|---------------|----------|
| **pure** | (none) | CDR serialization correctness, codegen output | Nothing |
| **ffi** | (none) | `rosz` package: context, pub/sub, service, action APIs | `libros_z.a` |
| **integration** | `integration` | Full end-to-end with live Zenoh sessions | `zenohd`, `libros_z.a` |

## Running Tests

```bash
# Pure tests — no external dependencies
just -f crates/ros-z-go/justfile test-go-pure

# FFI unit tests — requires libros_z.a
just -f crates/ros-z-go/justfile build-rust
just -f crates/ros-z-go/justfile test-go-ffi

# Integration tests — requires zenohd running on PATH
just -f crates/ros-z-go/justfile build-rust
just -f crates/ros-z-go/justfile test-integration

# Go↔Rust interop tests — also requires compiled Rust example binaries
just -f crates/ros-z-go/justfile build-rust-examples
just -f crates/ros-z-go/justfile test-integration

# Run with race detector
cd crates/ros-z-go
CGO_LDFLAGS="-L../../target/release" go test -race -tags integration ./interop_tests/...

# Everything
just -f crates/ros-z-go/justfile test-all
```

## Integration Test Structure

Integration tests live in `crates/ros-z-go/interop_tests/` and use the Go `integration` build tag so they are excluded from normal `go test ./...` runs.

### Common Infrastructure (`common_test.go`)

**`ZenohRouter`** manages a per-test isolated Zenoh router:

- Starts `zenohd` on a PID-derived port (`7447 + pid % 1000`) to avoid conflicts when tests run in parallel
- Stops the process via `t.Cleanup` — no manual teardown needed
- `router.Config()` returns the `ZENOH_CONFIG_OVERRIDE`-format string for Go contexts
- `getROS2Env(router)` returns env vars for ROS 2 processes: `RMW_IMPLEMENTATION=rmw_zenoh_cpp` and the matching `ZENOH_CONFIG_OVERRIDE`
- `rustEnv(router)` returns env vars for Rust ros-z processes: `ROSZ_CONFIG_OVERRIDE` pointing at the test router

**Availability checks** run before tests that need external tools:

- `checkROS2Available()` — verifies `ros2` CLI is on PATH
- `rustExampleBinary(name)` — returns the binary path if `target/release/examples/<name>` exists, or `("", false)` to skip gracefully

### Go ↔ ROS 2 Tests (`pubsub_test.go`, `service_test.go`, `action_test.go`)

These tests verify that Go nodes communicate with standard ROS 2 nodes running via `rmw_zenoh_cpp`.

| Test | Direction | Status |
|------|-----------|--------|
| `TestGoPublisherToROS2Subscriber` | Go pub → ROS 2 sub | Pass |
| `TestROS2PublisherToGoSubscriber` | ROS 2 pub → Go sub | Pass |
| `TestGoPublisherToGoSubscriber` | Go pub → Go sub | Pass |
| `TestGoServiceServerToROS2Client` | Go server ← ROS 2 client | Pass |
| `TestROS2ServiceServerToGoClient` | ROS 2 server → Go client | Pass |
| `TestGoServiceServerToGoClient` | Go server ↔ Go client | Pass |
| `TestGoActionServerToROS2Client` | Go server ← ROS 2 client | Pass |
| `TestROS2ActionServerToGoClient` | ROS 2 server → Go client | Pass |
| `TestGoActionServerToGoClient` | Go server ↔ Go client | Pass |
| `TestActionFeedbackMonitoring` | Server publishes feedback | Pass |
| `TestActionGoalCancellation` | Cooperative cancel stops execution early | Pass |
| `TestActionWithCustomTypes` | Custom action types | Skipped — needs fixture |

Tests that require ROS 2 skip automatically when `ros2` is not on PATH.

### Go ↔ Rust ros-z Tests (`ros_z_rust_test.go`)

These tests spawn compiled Rust ros-z example binaries and verify that the Go binding interoperates correctly with the native Rust implementation — without going through the ROS 2 rmw layer.

This matters because Go↔Go tests use the same FFI library on both sides; a bug that affects both paths symmetrically could go undetected.  Go↔Rust tests exercise the full serialization and protocol stack between two independent implementations.

| Test | Direction | Binary |
|------|-----------|--------|
| `TestGoPublisherToRustSubscriber` | Go pub → Rust sub | `z_pubsub --role listener` |
| `TestRustPublisherToGoSubscriber` | Rust pub → Go sub | `z_pubsub --role talker` |
| `TestGoServiceClientToRustServer` | Go client → Rust server | `z_srvcli --mode server` |
| `TestRustServiceClientToGoServer` | Rust client → Go server | `z_srvcli --mode client` |

Tests skip gracefully when the Rust example binary is missing — build it first:

```bash
just -f crates/ros-z-go/justfile build-rust-examples
```

**How Rust processes are configured:** `ZContextBuilder` in the Rust examples reads `ROSZ_CONFIG_OVERRIDE` (same `key=value;key=value` format as `ZENOH_CONFIG_OVERRIDE` for rmw) and applies those keys on top of the default ROS session config.  The test injects this variable to point the Rust process at the per-test router.

## Adding a New Integration Test

Create or add to a `_test.go` file in `crates/ros-z-go/interop_tests/`. Add the build tag at the top:

```go
//go:build integration
// +build integration
```

Start a per-test router:

```go
router := startZenohRouter(t)
os.Setenv("ZENOH_CONFIG_OVERRIDE", router.Config())
defer os.Unsetenv("ZENOH_CONFIG_OVERRIDE")
```

If your test needs ROS 2:

```go
if !checkROS2Available() {
    t.Skip("ROS2 not available")
}
// Pass getROS2Env(router) to exec.Command env
```

If your test needs a Rust binary:

```go
bin, ok := rustExampleBinary("z_pubsub")
if !ok {
    t.Skip("z_pubsub not built — run: cargo build --release --example z_pubsub")
}
// Pass rustEnv(router) to exec.Command env
```
