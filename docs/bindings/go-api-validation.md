# Go API coverage and validation

The Go binding exposes the topic, service, and action APIs described in
[Topics and schemas](go-topics.md), [Services and deferred replies](go-services.md),
and [Actions and goal lifecycle](go-actions.md). Keep the static library, C header,
Go package, and generated messages from the same revision when adopting these APIs.

## Run the checks

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

The malformed service-envelope regression uses a small native wire fixture.
Build it with `cargo test -p hiroz --features ffi --test ffi_service_malformed_query --no-run`,
set `HIROZ_SERVICE_MALFORMED_FIXTURE` to the printed test executable path, and run
`go test -race ./hiroz -run TestServiceCallbackSurvivesMalformedEnvelope` from
`crates/hiroz-go` with the matching `CGO_LDFLAGS`. It needs loopback socket access.

## Continuous integration

The `Go API validation` workflow runs on Linux for relevant pull requests and
pushes to `dexory`, and can be started manually. It uses Rust stable, matching the
native-library workflow, and the Go version declared in `crates/hiroz-go/go.mod`.
It runs native unit tests without FFI, builds the malformed-query fixture and
exports its executable path so that regression runs in the script's Go race suite,
then runs `scripts/check-go-api.sh` and verifies the committed C header with
cbindgen 0.29.4. ROS interop tests are compiled; running them still requires the
separate integration environment.

The job needs no repository secrets and uses a read-only token. It caches Go
build data and downloaded Rust dependencies, but not the large native target
directory. Native debug information and incremental compilation are disabled,
Cargo parallelism is capped, and temporary files use the runner's disk. The
45-minute timeout bounds cold builds.

## Regression coverage

The API audit's shared callback finding (X01) is covered by fresh allocation for
every typed delivery and tests of the production topic, service, and action
adapters. Topic cleanup findings (T02/T03) are covered by callback ownership tests,
concurrent channel cleanup, and closing subscribers and publishers from callbacks.
Native handles remain owned by active operations during closure (X02).

Action regression tests cover feedback receipt and status updates (A01/A02),
successful/aborted/canceled results without overwriting explicit completion (A03),
single/all/time-based cancellation and policy responses (A04/A05), offset-aware
result CDR (A06), transport deadlines and cancellation (A07), multiple result
waiters and expiry (A08), deferred execution and metadata (A09), and endpoint QoS
and readiness (A10).

Topic tests cover subscription discovery (T01), graph details and counts (T04),
serialized samples and metadata (T05), native dynamic CDR, schema registration,
alternate encoding, SHM, and timestamp caches (T06), and locality plus discovery
events (T07). Discovery counts report graph presence; they do not guarantee
type/QoS compatibility or delivery. Deadline, lifespan, and liveliness details
that the native graph does not retain are not fabricated by the Go API.

Service tests cover endpoint QoS, context cancellation and deferred replies (S01),
timeouts longer than ten seconds and invalid durations (S02), public raw calls
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
client goal registration cleanup, topic/service endpoint retention, concurrent
transport declarations, graph index removal, and schema discovery deadlines and
malformed responses. Changes to the pinned `third_party/zenoh` source trigger the
same CI workflow. These checks exercise the production dependency selection;
the separate NUC campaign is additional sustained testing, not a prerequisite
container or harness checked into this API series.
