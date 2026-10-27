# Go services: deadlines, cancellation, and deferred replies

Both service builders accept `WithQoS(hiroz.QosServicesDefault())`. The server's
history depth controls the queued request capacity. Service names and hashes
continue to use the service-level DDS identity for ROS 2 interoperability.

Native service calls wait for a matching queryable route before sending one
request. Readiness and response handling share the original deadline; cancellation
or timeout releases the underlying query. A response queued before a scheduler
stall cannot succeed after the caller's deadline. Service clients and servers
retain their native session until endpoint cleanup finishes.

The workspace pins the reviewed Zenoh 1.9 transport corrections in
`third_party/zenoh`. Rust applications built from a different workspace must also
select that dependency override; workspace-built Go FFI libraries include it.

## Calling a service

`Call` and `CallRaw` return CDR bytes with a five-second default timeout.
`CallWithTimeout` and `CallRawWithTimeout` require a positive duration and apply
it to both the Zenoh query and the local wait. There is no fixed ten-second
transport ceiling. `CallTyped` and `CallTypedWithTimeout` deserialize into the
destination supplied by the caller.

Use `CallWithContext`, `CallRawWithContext`, or `CallTypedWithContext` for
cancellation:

```go
callCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
defer cancel()
var response example_interfaces.AddTwoIntsResponse
err := hiroz.CallTypedWithContext(callCtx, client,
    &example_interfaces.AddTwoIntsRequest{A: 3, B: 4}, &response)
```

Cancellation releases the transport query, and concurrent calls keep separate
reply channels. A late reply cannot satisfy another call. Canceling a client
request does not stop work already running on a remote server. Context methods
return context cancellation/deadline errors when the context expires; transport
timeouts also support `errors.Is(err, hiroz.ErrTimeout)`.

`WaitForServiceWithContext` supports cancelable discovery. `WaitForService(0)`
performs a nonblocking readiness check, and negative durations are rejected.
Close the client to cancel its outstanding calls and prevent new ones.

## Deferred requests

`Build` keeps the callback interface. Callbacks run serially on a Go goroutine
and may close their own server. For a worker pool or asynchronous processing,
use `BuildQueued` and own each request until it has been answered or closed:

```go
server, err := node.CreateServiceServer("add_two_ints").
    WithQoS(hiroz.QosServicesDefault()).
    BuildQueued(&example_interfaces.AddTwoInts{})
// Check err before using server.
defer server.Close()

request, err := server.TakeRequest(ctx)
// Check err before using request.
defer request.Close()
var input example_interfaces.AddTwoIntsRequest
if err := request.Decode(&input); err != nil {
    return err
}
return request.Reply(&example_interfaces.AddTwoIntsResponse{Sum: input.A + input.B})
```

`TryTakeRequest` returns `(nil, nil)` when the queue is empty. `TakeRequest`
waits for a request, cancellation, or server closure. Callback servers reject
these methods so two consumers cannot compete for the same request.

`ServiceRequest.ID` contains the client writer GID and sequence number, while
`Data` contains a Go-owned copy of the request CDR. `Reply` and `ReplyRaw` send
at most one response; `Close` abandons an unanswered request. Serialization
errors leave the request available for retry. Delivered requests can outlive
the server, provided the containing context/session remains open. Close all
requests and entities before closing the context.

## Validation

The package tests exercise deferred replies after server closure, concurrent
request correlation, context cancellation, client closure, callback self-close,
duration validation, and a reply received after eleven seconds. They use a
local Zenoh session with multicast and listening endpoints disabled; no router
or ROS installation is required. `go test -short` skips only the eleven-second
regression test.
