# Go action API

Action clients and servers expose all five ROS action endpoints: send-goal,
get-result and cancel-goal services, and feedback and status topics. Each endpoint
uses the hash supplied by the `Action` descriptor and has independent QoS in
`ActionQoS`. Defaults use reliable service/feedback delivery and transient-local
status history.

Build with current generated messages. Action result types must implement
`PackToRawAt` and `UnpackFromRawAt`, as emitted by `hiroz-codegen-go`. A build fails
with an actionable error for old or handwritten message types without these
methods. These codecs align nested fields relative to the complete ROS envelope;
a result beginning with a boolean, uint16, float64 or nested struct needs no
special case.

## Clients

```go
client, err := node.CreateActionClient("fibonacci").
    WithQoS(hiroz.ActionQoSDefault()).
    WithRequestTimeout(5*time.Second).
    WithFeedbackBuffer(128).
    Build(&example_interfaces.Fibonacci{})
// Check err, then defer client.Close().

ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
defer cancel()
if err := client.WaitForServer(ctx); err != nil { return err }
goal, err := client.SendGoalWithContext(ctx, &example_interfaces.FibonacciGoal{Order: 20})
// Check err, then defer goal.Close().
status, resultBytes, err := goal.GetResultWithStatus(ctx)
// status distinguishes SUCCEEDED, ABORTED and CANCELED, even when result decoding succeeds.
```

`Feedback()` returns a per-goal channel of standalone CDR feedback messages.
`ReceiveTypedFeedback(ctx, goal, allocatedDestination)` decodes its next message.
Consume feedback concurrently with a blocking result wait. The buffer is bounded;
`DroppedFeedback()` reports messages dropped when a consumer falls behind.
`StatusUpdates()` coalesces live updates; `Status()` returns the latest observed
state. `Info()` includes the goal UUID and server acceptance timestamp. Incoming
feedback/status is registered before sending the request, including fast goals
that finish before the acceptance response arrives.

`GetResult`, `GetResultWithContext` and `GetTypedResult` remain available.
`GetTypedResultWithContext` adds typed context cancellation. Each concurrent result
wait receives the result; an unknown or expired goal returns promptly with UNKNOWN
and an error. `WithResultTimeout` configures a result request lifetime. Its zero
default uses the context deadline, or a 100-year transport lifetime when no deadline
is present. Request deadlines include native worker scheduling and time between
polls: an expired request cannot succeed by consuming an older buffered response.
Cancellation of a context, goal handle or client closes the actual
local transport request and drains its worker; it does not cancel remote execution.

`CancelWithContext` returns a structured `CancelResponse`, containing the ROS return
code and accepted goal list. `Cancel` retains its error-only convenience signature
and returns an error if the server rejected cancellation. `CancelAllGoals(ctx)`,
`CancelGoalsBefore(ctx, timestamp)` and `CancelGoals(ctx, id, timestamp)` implement
all ROS selection modes. A UUID-only cancellation always sends a zero timestamp,
so it cannot select unrelated earlier goals. The native Rust client uses the same
correct UUID-only policy.

## Servers

`Build` retains the goal-decision and execution callbacks. `WithGoalCallback`
optionally replaces the payload-only decision callback with a UUID-aware callback.
`WithCancelCallback` supplies a cancellation acceptance policy; the default accepts
eligible goals. Unknown and already-terminal UUIDs produce distinct ROS response
codes. All/earlier-time selection applies only to eligible active goals.

A `ServerGoalHandle` exposes `GoalID`, `Info`, `Status`, `IsActive`, `IsTerminal`,
`IsCancelRequested`, `Context`, `PublishFeedback`, `Succeed`, `Abort` and `Canceled`.
Explicit completion wins over the callback's eventual return value. Ordinary
callback success completes as SUCCEEDED, or CANCELED if cancellation was accepted.
Callback errors and recovered panics complete as ABORTED with a default result,
including replies to every pending waiter. Cancellation remains cooperative:
applications must check `IsCancelRequested` or perform explicit completion.

`BuildDeferred(action, decision, accepted)` accepts a goal and invokes the accepted
callback with its handle and data. The application can retain the handle and
schedule work. Call `Execute` when starting, then `Succeed`, `Abort` or `Canceled`
when finished. The accepted callback itself does not automatically complete the
goal. `WithGoalTimeout` optionally aborts unfinished goals after a lifetime;
`WithResultTimeout` controls completed result retention (15 minutes by default).
`WaitForFeedbackSubscriber(ctx)` waits for a feedback transport match.

`Close` prevents new operations, cancels the server context and initiates
asynchronous cleanup. Native workers and their callback handles remain retained
until every active callback returns. `Close` is safe from any server callback;
`WaitClosed(ctx)` provides an explicit cleanup barrier for callers outside callbacks.
The native server retains its transport session while callbacks drain, including
when its node and context have already closed. Close the server explicitly and
wait for this barrier when deterministic resource release is required.

Go action clients use the owned native event constructor: foreign callback data
is released only after every in-progress delivery has returned. The legacy C
constructor remains caller-owned and cannot destroy itself from an event callback.
Execution callbacks should observe `handle.Context().Done()` for responsive
shutdown. Deferred application goroutines should also observe the server context;
retained handles return closed errors afterward. A configured goal timeout changes
the goal to ABORTED; workers can inspect `IsTerminal()` to stop their own work.

## C FFI migration

This repair changes the low-level action FFI, while preserving the established Go
entry points. The old C API passed independently encoded result payloads without
any schema or offset-aware codec. Correctly embedding those bytes in GetResult is
not possible for arbitrary CDR layouts. The old result API also discarded the
terminal status. Consumers of the C header must rebuild and migrate; old symbols
are not silently reused with incompatible signatures.

- Replace `hiroz_action_client_create` and `hiroz_action_server_create` with the
  corresponding `_create_with_options` functions. Pass all five hashes, endpoint
  QoS, callback hooks and a default, fully encoded GetResult response for servers.
- Replace the old client `send_goal`, `get_result` and `cancel_goal` functions and
  `CGoalHandle` with `hiroz_action_request_create/poll/destroy`. Requests and
  responses are complete ROS CDR envelopes. Destroy cancels and drains the request.
- Replace server `succeed`, `abort` and `canceled` functions with
  `hiroz_action_server_finish`, passing the complete status/result envelope.
- Replace `hiroz_action_server_publish_feedback` with
  `hiroz_action_server_publish_feedback_message`, passing the complete UUID/feedback
  envelope. Query cancellation through `hiroz_action_server_goal_status`.
- Client/server destroy entry points retain their existing signatures.
  `hiroz_action_server_shutdown` requests nonblocking shutdown and is safe from
  callbacks; call the blocking destroy operation from outside callbacks.

The Rust action transport never guesses padding in an opaque result. The Go path
encodes/decodes envelopes through the generated offset-aware codec before crossing
the FFI boundary.

## Reproduce deadline and transport regressions

Run the existing Rust action suite with its default parallel runner and the FFI
request deadline regression:

```bash
cargo test --locked -p hiroz --features ffi \
  --test action --test action_client_lifetime --test ffi_action_lifetime
```

The action fixtures explicitly clear default router connections. The deadline
regression queues a real response, suspends the foreign poller beyond its timeout,
and requires a timeout for send-goal, get-result and cancel-goal. Before the fix,
all three incorrectly returned success. The buffered-feedback regression similarly
requires an already-canceled caller to leave its queued sample unread.

Build a matching native library and run the Go tests. A fresh library path also
prevents Go's build cache from reusing an executable linked to an older static
library after native code changes:

```bash
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build --locked -p hiroz --features ffi
native_dir=$(mktemp -d)
trap 'rm -rf "$native_dir"' EXIT
cp "$CARGO_TARGET_DIR/debug/libhiroz.a" "$native_dir/"
export CGO_LDFLAGS="-L$native_dir ${CGO_LDFLAGS:-}"
(cd crates/hiroz-go && go test -race -count=1 -timeout=5m ./hiroz -run Action)
(cd crates/hiroz-go && go test -race -count=10 -timeout=5m ./hiroz \
  -run '^TestActionTransportConcurrentLifecycle$')
```

The transport regression creates its own local session, direct TCP peers and
loopback router with separate client sessions. These three cases run in parallel;
each keeps successful results pending for at least 12 seconds while exercising
feedback, concurrent goals and result waiters, selected cancellation, a short
local wait timeout, repeated retained-result retrieval and bounded shutdown.
No installed ROS router, ROS messages, NUC or Balena deployment is required.
This checks transport behavior with Go endpoints; ROS wire interoperability
still requires the separate interop suite.
