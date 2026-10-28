# Go topics, discovery, and runtime schemas

Go topic endpoints now announce their ROS discovery tokens and retain them until
Close. This includes action feedback/status subscriptions built through the raw
subscriber API. Raw publishers map reliable QoS to blocking congestion control and
best effort to dropping congestion control, matching native typed publishers.

## Publishing and receiving

Existing `Publish(Message)` and `BuildWithCallback(Message, MessageHandler)` remain
available. The latter delivers owned payload bytes. `BuildWithSampleCallback`
delivers an owned `Sample` containing payload, encoding, raw attachment, exact
Zenoh timestamp, receipt time, and optional ROS sequence number, source timestamp
and publisher GID. Metadata is absent when the sender supplies no valid ROS
attachment. Callbacks should return promptly; callback panics are contained at
the Go boundary.

```go
receiver, err := node.CreateSubscriber("telemetry").BuildSampleReceiver(message, 32)
if err != nil { return err }
defer receiver.Close()

publisher, err := node.CreatePublisher("telemetry").Build(message)
if err != nil { return err }
defer publisher.Close()
if err := publisher.WaitForSubscriptionContext(ctx); err != nil { return err }
if err := publisher.Publish(message); err != nil { return err }
sample, err := receiver.Receive(ctx)
```

`SampleReceiver` provides `Receive(ctx)`, `TryReceive`, coalesced `Ready`
notifications, and `Done`. Its bounded FIFO evicts the oldest sample on overflow;
`Dropped` reports these losses. Closing it stops delivery and permits draining
retained samples before `ErrReceiverClosed`. Multiple consumers divide messages.
Node.Close also stops owned receivers.

`PublishSerialized` accepts already encoded bytes, including an empty payload.
`PublishSample` preserves an explicitly supplied attachment, encoding and timestamp;
an absent attachment generates fresh ROS identity metadata. Use a different topic
when forwarding a received sample unchanged: transports can suppress a duplicate
timestamp on the same topic. Setting `WithEncoding("application/protobuf")` labels
serialized bytes; the application supplies its own codec. `Publish(Message)`
always calls `SerializeCDR`, regardless of the configured transport label.

`WithLocality(LocalitySessionLocal)` receives only publications from this Zenoh
session; `LocalityRemote` excludes them; `LocalityAny` is the default. Session
locality is not node locality. Restricted locality combined with transient-local
QoS returns a build error: Zenoh 1.9 history queries cannot enforce the same origin
filter as live delivery. Use `LocalityAny` for transient-local history.

## Inspection and discovery events

Publishers expose `Info`, `SubscriptionCount`, and `WaitForSubscription` (duration
or context). Subscribers expose `Info`, `PublisherCount`, and `WaitForPublisher`.
These query discovery, not negotiated transport matches: a positive count does not
certify type/QoS compatibility or that a specific message will be delivered.
Relative/private endpoint names are qualified natively before inspection.

Context provides:

- `GetEndpoints(kind, name)` and `CountEndpoints` for fully qualified topic/service names.
- `GetEndpointsByNode(kind, nodeName, namespace)` for publisher, subscription,
  service-server or service-client endpoints. Endpoint records include type/hash,
  GID and node/session identity where the transport advertises them.
- `GetActionNamesAndTypes`, action client/server names/types by node, and
  `GetNodeNamesWithEnclaves`.
- `WatchEndpoints(ctx, kind, name, interval)` for an initial snapshot followed by
  changes. A slow consumer receives the latest snapshot. Polling can coalesce
  brief intermediate changes; cancel the context when the watch is no longer needed.

`EndpointInfo.QoS` intentionally contains only reliability, durability, history and
depth: these are the policies retained by the native graph protocol. The existing
builder QoS profile still accepts deadline, lifespan and liveliness settings.
Discovery events are not DDS deadline, incompatible-QoS or liveliness events;
those are not implemented consistently in the native transport and are not
advertised by this API. Native handles remain retained through active inspection
operations, including concurrent Close.

## Schemas and dynamic CDR

Build nodes with `WithTypeDescriptionService` to advertise runtime schemas.
`NewMessageSchema` accepts the ROS `TypeDescription` structure;
`NewMessageSchemaJSON` accepts its JSON representation. The native schema model
validates it and computes its RIHS01 hash. Explicit `RegisterSchema` publishes it
through the node's standard type-description service.

Newly generated messages expose `TypeDescriptionJSON()` when the export manifest
contains their complete dependency closure. Topic builders automatically register
these schemas on enabled nodes and reject inconsistent message hashes. Regenerate
older messages to obtain that method. `SchemaMessage` is an optional interface for
custom messages that already own a `MessageSchema`. For service request/reply
schemas use `RegisterSchema` explicitly.

`DiscoverTopicSchema(topic, timeout)` queries the publishing node and retains its
advertised hash and transport type name. Its `MessageSchema.TypeName` and
`Description` remain canonical ROS schema identities; `NewMessage().TypeName()`
retains the publisher's exact wire spelling, including legacy Go component names
such as `example_interfaces/AddTwoIntsRequest`. This lets dynamic subscriptions
match those publishers without changing their transport identity.
`DiscoverServiceSchemas(service, requestType, responseType,
timeout)` resolves both schemas; use ROS schema names such as
`example_interfaces/msg/AddTwoIntsRequest`. One timeout bounds the whole discovery.
The calls return errors for unavailable type services or unsupported schema types.

```go
schema, err := node.DiscoverTopicSchema("telemetry", 3*time.Second)
if err != nil { return err }
subscriber, err := node.CreateSubscriber("telemetry").BuildDynamicWithCallback(
    schema, func(message *hiroz.DynamicMessage, err error) {
        if err != nil { log.Print(err); return }
        log.Print(message.Values)
    })
```

`schema.NewMessage()` implements `Message`: populate its `Values` map and publish
it normally. Nested objects, fixed arrays and sequences use JSON-compatible Go
values. Native CDR serialization and decoding validate integer ranges and
collection bounds.
Decoding retains integers as `json.Number`, including full-width uint64 values.
Use integer slices or `[]any` for byte sequences; Go's `[]byte` JSON encoding is
base64. The JSON value bridge accepts finite floats. Unsupported native field
kinds, including wide strings, return errors. Schema objects are immutable;
message Values maps follow normal Go ownership rules. Runtime schemas are limited
to 64 nesting levels and 64 MiB of recursively calculated default-value storage;
native dynamic decoding also limits allocated value storage to 64 MiB per message.
Oversized schemas, hostile/truncated length prefixes, and bound violations return
errors before allocating their claimed storage. These dynamic-codec limits do not
apply to raw serialized topic operations or generated typed codecs.

## Shared memory and timestamp cache

`PublisherBuilder.WithSharedMemory(poolBytes, threshold)` creates a private SHM
pool, and `WithoutSharedMemory` disables application-level SHM allocation. The
default inherits node/context SHM configuration. At/above threshold, publishing
copies Go bytes into SHM when nonblocking allocation succeeds; exhaustion falls
back to ordinary memory. Transport-level negotiation is independent of this
application allocation choice. Received samples are copied into Go-owned slices.

`BuildWithCache(message, capacity)` returns a subscriber and `SampleCache`. It
indexes ROS source time when present and receipt time otherwise. `Add` also
accepts a caller-selected timestamp. `Between` is inclusive; `BeforeOrAt` and
`AfterOrAt` support nearest-window queries. The cache retains the newest timestamps,
handles out-of-order arrivals, and returns deep copies. It is bounded independently
of transport QoS history and is safe for concurrent reads and callback writes.

Schema discovery uses one timeout budget for graph discovery, route readiness,
and schema responses. Multiple schema sources are queried concurrently so an
unavailable source cannot consume a healthy source's entire budget. Late queued
responses return a timeout, and native Rust `Duration::MAX` remains unbounded.
Graph deletion removes stale entries even while callers retain old snapshots.

Native topic endpoints retain the session through their own teardown. This does
not change `Node.Close` stopping its owned Go receivers, or explicit Rust context
shutdown invalidating every endpoint. Raw Rust `RawPublisher::new` callers must
now supply the owning `Arc<Session>` as its final argument.

## Shared-memory context provider

`ContextBuilder.WithShmPool(bytes)` attaches a native shared-memory provider to
the session; zero leaves it disabled. A publisher can inherit this provider or
select its own pool with `WithSharedMemory(poolBytes, threshold)`. Peer support
and Zenoh transport configuration still determine negotiated delivery.

The C configuration structure includes `shm_pool_bytes`. Rebuild C/Go callers
against the matching header and native library, and initialize the field; an old
shorter structure is not ABI-compatible. Go callbacks receive copied, Go-owned
bytes, so this option does not provide a zero-copy Go slice contract.
