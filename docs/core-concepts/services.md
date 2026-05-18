# Services

!!! note "Go users"
    The code examples in this chapter are **Rust**. The Go service API is callback-based — handlers are registered instead of using `take_request()`. For Go service patterns and the typed service API, see the [Go Bindings](../bindings/go.md) chapter.

**hiroz implements ROS 2's service pattern with type-safe request-response communication over Eclipse Zenoh.** This enables synchronous, point-to-point interactions between nodes using a pull-based model for full control over request processing.

!!! note
    Services provide request-response communication for operations that need immediate feedback. Unlike topics, services are bidirectional and ensure a response for each request. hiroz uses a pull model that gives you explicit control over when to process requests.

## What is a Service?

```mermaid
sequenceDiagram
accTitle: Service request and response between client and server
accDescr: The client sends a typed request with two integers to the server, which computes the sum and returns a typed response.
    participant C as Client
    participant S as Server

    C->>S: Request(a=3, b=5)
    Note right of S: compute sum
    S-->>C: Response(sum=8)
```

**A named remote function call. Client sends a typed request; server returns a typed response.**

- One server per service name (multiple = undefined behavior)
- Any number of clients can call the same service
- Client blocks (or awaits) until the response arrives
- Use for short operations — milliseconds, not minutes

### Service vs Topic vs Action

```mermaid
graph TD
accTitle: Decision tree for choosing Topic, Service, or Action
accDescr: A decision tree that routes continuous data streams to Topics, fast single results to Services, and long tasks needing progress and cancellation to Actions.
    Q{What do you need?}
    Q -->|Continuous data stream| T[Topic]
    Q -->|One result, fast| S[Service]
    Q -->|Long task + progress + cancel| A[Action]
    style S fill:#3f51b5,color:#fff,stroke:#3f51b5
```

| | Topic | Service | Action |
|-|-------|---------|--------|
| **Direction** | One-way push | Request → Response | Request → Feedback → Result |
| **Duration** | Continuous | Milliseconds | Seconds to minutes |
| **Cancellation** | N/A | No | Yes |
| **Feedback** | N/A | No | Yes |

### The .srv format

```text
# Request (above the ---)
uint32 a
uint32 b
---
# Response (below the ---)
uint32 sum
```

Two sections, separated by `---`. Each is a message definition. hiroz generates a Rust trait:

```rust
trait AddTwoInts: ZService {
    type Request  = AddTwoIntsRequest;   // { a: u32, b: u32 }
    type Response = AddTwoIntsResponse;  // { sum: u32 }
}
```

### What happens with no server?

```mermaid
sequenceDiagram
accTitle: Service call timeout when no server is registered
accDescr: The client sends a request to the Zenoh router, which waits indefinitely for a server and eventually times out after the configured queries_default_timeout.
    participant C as Client
    participant Z as Zenoh Router

    C->>Z: Request (no server registered)
    Note over Z: waits for server...
    Z-->>C: Timeout after queries_default_timeout (default: 10 min)
```

!!! tip
    Set a shorter timeout in the Zenoh config for faster failure detection in production.

### QoS note

Services use **reliable + volatile** durability. Volatile means: if a server restarts, old in-flight requests are discarded — no stale requests reach the new server.

### Key Concepts at a Glance

<div class="flashcard-grid">
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">Pattern</div>
        <div class="flashcard-term">What is a Service?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        A named request-response channel. A client sends a typed request; the server processes it and returns a typed response. Like a remote function call.
      </div>
    </div>
  </div>
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">Constraint</div>
        <div class="flashcard-term">How many servers can a service have?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        One. Multiple servers on the same service name is undefined behavior. Multiple clients are fine — any number can call the same service.
      </div>
    </div>
  </div>
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">vs Action</div>
        <div class="flashcard-term">Service vs Action — when to pick which?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        <div>• <strong>Service</strong>: result in milliseconds, no progress needed.</div>
        <div>• <strong>Action</strong>: takes seconds/minutes, need feedback or cancellation.</div>
      </div>
    </div>
  </div>
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">Timeout</div>
        <div class="flashcard-term">What happens if no server is running?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        The call blocks until the server comes online or a timeout fires. hiroz uses Zenoh's query timeout (default: 10 minutes — configure with <strong>queries_default_timeout</strong>).
      </div>
    </div>
  </div>
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">Pull Model</div>
        <div class="flashcard-term">What is the pull model in hiroz?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        Call <strong>take_request()</strong> when you are ready — you control timing. Requests queue up until you retrieve them. Contrast with a callback model where the framework calls you.
      </div>
    </div>
  </div>
  <div class="flashcard">
    <div class="flashcard-inner">
      <div class="flashcard-front">
        <div class="flashcard-tag">QoS</div>
        <div class="flashcard-term">Why do services use volatile durability?</div>
        <div class="flashcard-hint">Click to flip</div>
      </div>
      <div class="flashcard-back">
        If a server restarts, old requests should not be replayed — stale requests could cause incorrect state. Volatile discards requests that no running server received.
      </div>
    </div>
  </div>
</div>

## Visual Flow

```mermaid
graph TD
accTitle: Service visual flow from context creation to response handling
accDescr: ZContextBuilder creates a ZContext that spawns both a client node and server node; the client sends a request routed through the service call to the server's handler, which returns a response the client takes.
    A[ZContextBuilder] -->|configure| B[ZContext]
    B -->|create| C[Client Node]
    B -->|create| D[Server Node]
    C -->|create_client| E[Service Client]
    D -->|create_service| F[Service Server]
    E -->|call| G[Service Call]
    G -->|route| F
    F -->|take_request| H[ServiceRequest]
    H -->|reply_blocking| G
    G -->|deliver| E
```

## Key Features

| Feature | Description | Benefit |
|---------|-------------|---------|
| **Type Safety** | Strongly-typed service definitions with Rust structs | Compile-time error detection |
| **Pull Model** | Explicit control over request processing timing | Predictable concurrency and backpressure |
| **Async/Blocking** | Dual API for both paradigms | Flexible integration patterns |
| **Unified Request Object** | `ServiceRequest` carries message and reply method together | No dangling reply tokens |

## Service Server Example

A server that adds two integers. Full source: [`crates/hiroz/examples/demo_nodes/add_two_ints_server.rs`](https://github.com/ZettaScaleLabs/hiroz/blob/main/crates/hiroz/examples/demo_nodes/add_two_ints_server.rs)

```rust
--8<-- "crates/hiroz/examples/demo_nodes/add_two_ints_server.rs:full_example"
```

**Key points:**

- `take_request()` blocks until a request arrives — the pull model means you control timing
- `req.message()` accesses the request fields (`req.message().a`, `req.message().b`)
- `req.reply_blocking(&resp)` sends the response; the request object owns the reply channel
- `service` must be `mut` because `take_request` takes `&mut self`
- `max_requests: Option<usize>` lets tests stop the server after N requests; pass `None` for a perpetual server

!!! tip "Run it now"
    Jump to [Complete Service Workflow](#complete-service-workflow) below to see server and client exchange requests with one command per terminal.

## Service Client Example

A client that sends an addition request and prints the result. Full source: [`crates/hiroz/examples/demo_nodes/add_two_ints_client.rs`](https://github.com/ZettaScaleLabs/hiroz/blob/main/crates/hiroz/examples/demo_nodes/add_two_ints_client.rs)

```rust
--8<-- "crates/hiroz/examples/demo_nodes/add_two_ints_client.rs:full_example"
```

**Key points:**

- `call(&req).await` is the single-step async API — it sends the request and waits for the response
- `call_with_timeout(&req, duration).await` enforces an explicit deadline; returns `Err` on timeout
- If no server is running, the call blocks until Zenoh's query timeout fires (default: 10 minutes)

## Complete Service Workflow

!!! note
    These commands run the ready-made examples from the hiroz repository. Clone it first with `git clone https://github.com/ZettaScaleLabs/hiroz.git && cd hiroz`. If you're building your own project, run your binaries with `cargo run` instead.

**Terminal 1 — Start Zenoh Router:**

```bash
cargo run --example zenoh_router
```

**Terminal 2 — Start Server:**

```bash
cargo run --example demo_nodes_add_two_ints_server
```

**Terminal 3 — Send Client Requests:**

```bash
# Request 1
cargo run --example demo_nodes_add_two_ints_client -- --a 10 --b 20

# Request 2
cargo run --example demo_nodes_add_two_ints_client -- --a 100 --b 200
```

<script src="https://asciinema.org/a/yChRkMOyYKoKBPqM.js" id="asciicast-yChRkMOyYKoKBPqM" async="true"></script>

!!! success
    The server processes each client request immediately, demonstrating synchronous request-response communication over Zenoh.

## Service Server Patterns

Service servers in hiroz follow a **pull model** pattern, similar to subscribers. You explicitly receive requests when ready to process them, giving you full control over request handling timing and concurrency.

!!! info
    This pull-based approach is consistent with subscriber's `recv()` pattern, allowing you to control when work happens without callbacks interrupting your flow.

### Pattern 1: Blocking Request Handling

Best for: Simple synchronous service implementations

```rust
use hiroz::Builder;

let mut service = node
    .create_service::<ServiceType>("service_name")
    .build()?;

loop {
    let req = service.take_request()?;
    let response = process_request(req.message());
    req.reply_blocking(&response)?;
}
```

`take_request()` blocks until a request arrives. The server variable must be `mut` because `take_request` takes `&mut self`. The returned `ServiceRequest` owns the reply channel — call `req.reply_blocking(&response)` to send the response. Access message fields via `req.message()`.

### Pattern 2: Async Request Handling

Best for: Services that need to await other operations

```rust
use hiroz::Builder;

let mut service = node
    .create_service::<ServiceType>("service_name")
    .build()?;

loop {
    let req = service.async_take_request().await?;
    let response = async_process_request(req.message()).await;
    req.reply(&response).await?;
}
```

### Why Pull Model?

| Aspect | Pull Model (take_request) | Push Model (callback) |
|--------|---------------------------|----------------------|
| **Control** | Explicit control over when to accept requests | Interrupts current work |
| **Concurrency** | Easy to reason about | Requires careful synchronization |
| **Backpressure** | Natural - slow processing slows acceptance | Can overwhelm if processing is slow |
| **Consistency** | Same pattern as subscriber `recv()` | Different pattern |

## Service Client Patterns

Service clients use a single `call` method — send and receive in one await.

### Pattern 1: Call with Timeout

Best for: Simple request-response where you want to fail fast on a slow or absent server

```rust
use hiroz::Builder;
use std::time::Duration;

let client = node
    .create_client::<ServiceType>("service_name")
    .build()?;

let request = create_request();
let response = client.call_with_timeout(&request, Duration::from_secs(5)).await?;
```

### Pattern 2: Call without Explicit Timeout

Best for: Integration with async codebases where you manage timeouts externally (e.g., `tokio::time::timeout`)

```rust
use hiroz::Builder;

let client = node
    .create_client::<ServiceType>("service_name")
    .build()?;

let request = create_request();
let response = client.call(&request).await?;
```

!!! tip
    `use hiroz::Builder;` must be in scope to call `.build()`. Both patterns require an async runtime such as `tokio`. For logging, call `zenoh::init_log_from_env_or("error")` before building the context.

## ROS 2 Interoperability

hiroz services interoperate with ROS 2 C++ and Python nodes when both sides share the same Zenoh transport:

**Requirements:**

- ROS 2 nodes must use [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) (`export RMW_IMPLEMENTATION=rmw_zenoh_cpp`)
- Both sides must use matching service types with identical RIHS01 type hashes
- All nodes must connect to the same Zenoh router

```bash
# List available services
ros2 service list

# Call hiroz service from ROS 2 CLI
ros2 service call /add_two_ints example_interfaces/srv/AddTwoInts "{a: 42, b: 58}"

# Show service type
ros2 service type /add_two_ints

# Get service info
ros2 service info /add_two_ints
```

!!! warning
    Service interop requires [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh) on the ROS 2 side. The `zenoh-bridge-ros2dds` approach
    works for pub/sub but does not fully support services.

## Error Handling

### When the server is not running

`call()` blocks until a response arrives or Zenoh's query timeout fires (default: 10 minutes). For explicit deadline control, use `call_with_timeout`:

```rust
match client.call_with_timeout(&request, Duration::from_secs(5)).await {
    Ok(response) => println!("Got response: {:?}", response),
    Err(e) => eprintln!("Service not available or timed out: {}", e),
}
```

### Client methods compared

| Method | Behavior |
|--------|----------|
| `call(&req).await` | Waits for response; Zenoh query timeout applies (default: 10 min) |
| `call_with_timeout(&req, duration).await` | Waits up to `duration`; `Err` on timeout |

## Resources

- **[Custom Messages](../user-guide/custom-messages.md)** - Defining and using custom service types
- **[Message Generation](../user-guide/message-generation.md)** - Generating service definitions
- **[Actions](./actions.md)** - For long-running operations with feedback

**Start with the examples above to understand the basic service workflow, then explore custom service types for domain-specific operations.**
