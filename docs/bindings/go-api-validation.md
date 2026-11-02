# Go API coverage and validation

The Go binding exposes the topic, service, and action APIs described in
[Topics and schemas](go-topics.md), [Services and deferred replies](go-services.md),
and [Actions and goal lifecycle](go-actions.md). Keep the static library, C header,
Go package, and generated messages from the same revision when adopting these APIs.

## Run the checks

Use the repository development environment (`nix develop`), or install Rust, a C/C++
toolchain, the Go version from `crates/hiroz-go/go.mod`, Python 3, and cbindgen 0.29.4.
The tests need local loopback sockets; no NUC or Balena deployment is required.
From the repository root, run:

```bash
./scripts/check-go-api.sh
```

This builds the FFI library, generates bundled messages using the module's actual
`github.com/ZettaScaleLabs/hiroz/crates/hiroz-go` import path, runs the Go generator tests,
runs the Go race suite and serializer fixtures, vets the library and examples,
compiles the separate ROS interop test suite, and checks the production-service
example module. The local-session tests require no ROS installation or router.
They deliberately include long requests beyond the former transport timeout.
Native integration targets also check callback ownership and bounded schemas.
The generated-message suite sends actual typed topics, service requests, action
feedback, and action results through the public APIs and checks schema discovery.

Use `CARGO_TARGET_DIR`, `GOCACHE`, and `GOTMPDIR` to choose cache locations. The
script generates ignored files under `crates/hiroz-go/generated` and `_tmp`.
Generated messages include `PackToRawAt` and `UnpackFromRawAt`; action result
envelopes require those offset-aware methods. Regenerate existing message packages
before using the new action implementation.

Compile-time checks do not establish ROS 2 wire interoperability. The separate
[interop suite](../user-guide/go_interop_tests.md) requires its router and ROS/Rust
fixtures. Treat a skipped test as unverified, not as a successful interop check.

The script builds and enables the malformed service-envelope wire fixture automatically.
To run that regression separately, build it with `cargo test -p hiroz --features ffi --test ffi_service_malformed_query --no-run`,
set `HIROZ_SERVICE_MALFORMED_FIXTURE` to the printed test executable path, and run
`go test -race ./hiroz -run TestServiceCallbackSurvivesMalformedEnvelope` from
`crates/hiroz-go` with the matching `CGO_LDFLAGS`. It needs loopback socket access.

## Minimal regression commands

Use a clean checkout of the PR under review. Each command below names a committed
regression; the larger validation script is unnecessary for the native cases.
A dependent API PR must include its listed prerequisites. These tests use synthetic
messages and do not require Dexory packages.

### Volatile subscriber ordering

```bash
cargo test --locked -p hiroz --features ffi --test volatile_subscriber_order -- --nocapture
```

The test sends three samples with timestamps `2, 1, 2` and requires both typed and
raw subscribers to receive all three. The unfixed implementation drops the older
and repeated timestamps. This test uses one local session, so it isolates subscriber
selection from routed Zenoh failures.

### Callback ownership and endpoint lifetimes

```bash
cargo test --locked -p hiroz --features ffi --test ffi_subscriber_lifetime --test endpoint_lifetime
```

Owned callback data must be released exactly once, and an endpoint must retain its
native session while it remains usable. Explicit context shutdown still takes effect.

### Service deadlines and malformed requests

```bash
cargo test --locked -p hiroz --features ffi --test ffi_service_timeout --test service_readiness
```

The tests distinguish a transport timeout from an application payload named
`Timeout`, and check that readiness and response waits share one deadline. For the
buffered-success regression, `successful_reply_buffered_before_poll_respects_deadline`,
requires a timeout after a delayed foreign poller resumes. For the
malformed-envelope callback test, use the fixture instructions above or the complete
validation script; an unset fixture variable causes a skip, not a pass.

### Atomic action transitions and cancellation selection

```bash
cargo test --locked -p hiroz --lib action::server::transition_tests
```

An accepted goal must remain visible during execution registration; result requests
must wait for its real terminal result. See the [action guide](go-actions.md) for
the native cancellation-selection regression and all four UUID/timestamp modes.

### Action cleanup and expired results

```bash
cargo test --locked -p hiroz --features ffi --test action_client_lifetime --test ffi_action_lifetime
```

Dropping pending operations must release local registrations. A queued result
observed after the caller's absolute deadline must fail, and callback data must stay
alive until native callbacks finish.

### Schema discovery and dynamic payload bounds

```bash
cargo test --locked -p hiroz --features ffi \
    --test concurrent_schema_discovery --test schema_discovery_malformed_reply \
    --test dynamic_bounded_schema
(cd crates/hiroz-codegen-go && go test -count=1 -run TestGeneratedTypeDescriptions ./...)
```

Discovery must respect one deadline across graph and description-route waits;
malformed replies retain their error classification. Dynamic decoding must reject
payloads that exceed declared bounds. Generated messages, services and actions must
retain their complete type descriptions.

### Public Go APIs

Build the matching library first, then run individual regressions from the Go module:

```bash
export CARGO_TARGET_DIR="$PWD/target"
cargo build --locked -p hiroz --features ffi
export CGO_LDFLAGS="-L$CARGO_TARGET_DIR/debug ${CGO_LDFLAGS:-}"
(cd crates/hiroz-go && go test -race -count=1 ./hiroz -run '^TestTypedSubscriberPublicEntryPoints$')
(cd crates/hiroz-go && go test -race -count=1 ./hiroz -run '^TestServiceContextCancellationAndClientClose$')
(cd crates/hiroz-go && go test -race -count=1 ./hiroz -run '^TestActionLiveResultBeyondOldTransportDeadline$')
(cd crates/hiroz-go && go test -race -count=1 ./hiroz -run '^TestTopicAPITransport$')
```

The action test intentionally runs for at least 12 seconds. Do not use `-short`
when validating the former ten-second transport limit. A successful focused test
establishes only its asserted behavior: full-suite failures, router diagnostics,
ROS interoperability and endurance results must be reported separately.

## Continuous integration

The `Go API validation` workflow runs on Linux for relevant pull requests and
pushes to `main`, and can be started manually. It uses Rust stable, matching the
native-library workflow, and the Go version declared in `crates/hiroz-go/go.mod`.
It runs native unit tests without FFI, strict Clippy with FFI enabled,
then runs `scripts/check-go-api.sh`,
which builds the malformed-query fixture and enables it in the Go race suite.
The workflow verifies the committed C header with
cbindgen 0.29.4. ROS interop tests are compiled; running them still requires the
separate integration environment.

The job needs no repository secrets and uses a read-only token. It caches Go
build data and downloaded Rust dependencies, but not the large native target
directory. Native debug information and incremental compilation are disabled,
Cargo parallelism is capped, and temporary files use the runner's disk. The
45-minute timeout bounds cold builds.

## Regression coverage

Shared callback safety is covered by fresh allocation for
every typed delivery and tests of the production topic, service, and action
adapters. Topic cleanup is covered by callback ownership tests,
concurrent channel cleanup, and closing subscribers and publishers from callbacks.
Native handles remain owned by active operations during closure.

Action regression tests cover feedback receipt and status updates,
successful/aborted/canceled results without overwriting explicit completion,
single/all/time-based cancellation and policy responses, offset-aware
result CDR, transport deadlines and cancellation, multiple result
waiters and expiry, deferred execution and metadata, and endpoint QoS
and readiness.

Topic tests cover subscription discovery, graph details and counts,
serialized samples and metadata, native dynamic CDR, schema registration,
alternate encoding, SHM, and timestamp caches, and locality plus discovery
events. Discovery counts report graph presence; they do not guarantee
type/QoS compatibility or delivery. Deadline, lifespan, and liveliness details
that the native graph does not retain are not fabricated by the Go API.

Service tests cover endpoint QoS, context cancellation and deferred replies,
timeouts longer than ten seconds and invalid durations, public raw calls
(S03), concurrent response correlation, callback self-close, and pending calls
when the client closes. Deferred requests remain independently owned until they
are replied to or closed.

## Lifetime rules

Explicitly close entities and requests; finalizers are a fallback. Close child
entities before their node and context. Closing an entity prevents new operations
and cancels or retires active native work as documented by that entity. User
callbacks already executing may finish after `Close` returns. Coordinate any
application state those callbacks use separately from the transport handle.

Dynamic JSON values preserve integer precision, but support finite floating-point
values only. Encoding options label serialized bytes; they do not transcode them.
Go's SHM API copies between Go memory and native shared memory, so applications
must not infer a zero-copy Go slice contract.

The native integration checks also cover action callback/session ownership,
client goal registration cleanup, topic/service endpoint retention, graph index
removal, and schema discovery deadlines and malformed responses. These checks
use the Zenoh dependency selected by this checkout;
the separate NUC campaign is additional sustained testing, not a prerequisite
container or harness checked into this API series.
