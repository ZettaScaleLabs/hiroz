# Message Cache

**`ZCache<T>` is a timestamp-indexed, capacity-bounded sliding-window cache of received messages.** It mirrors the ROS 2 `message_filters::Cache<T>` API, letting you query a time-ordered history of messages without a background thread or executor.

!!! note
    `ZCache<T>` wraps an Eclipse Zenoh subscriber and retains the N most recent messages in a `BTreeMap` keyed by timestamp. Queries run in O(log n) using Rust's range API.

## Quick Start

```rust
use hiroz::prelude::*;
use hiroz_msgs::sensor_msgs::Imu;
use std::time::{Duration, SystemTime};

let ctx = ZContextBuilder::default().build()?;
let node = ctx.create_node("cache_node").build()?;

// Retain the 200 most recent messages, indexed by Zenoh transport timestamp.
let cache = node.create_cache::<Imu>("/imu/data", 200).build()?;

// Query the last 100 ms.
let now = SystemTime::now();
let window = cache.get_interval(now - Duration::from_millis(100), now);
println!("Messages in window: {}", window.len());
```

!!! tip
    `use hiroz::Builder;` must be in scope to call `.build()`. It is re-exported by `hiroz::prelude::*`.

## Stamp Strategies

Two indexing strategies are available and selected at **compile time** via the type-state builder:

| Strategy | Type | When to use |
|----------|------|-------------|
| **ZenohStamp** (default) | `ZenohStamp` | Zero-config; works for any message type. Indexed by the Zenoh transport timestamp. |
| **ExtractorStamp** | `ExtractorStamp<T, F>` | Sensor fusion: align by logical time (e.g. `header.stamp`). |

### ZenohStamp (default)

No configuration needed. The cache reads the `uhlc::Timestamp` that the Zenoh transport attaches to every published sample and converts it to `SystemTime`. If the peer has timestamping disabled, the cache falls back to `SystemTime::now()` at receive time and logs a one-time warning.

```rust
use hiroz::prelude::*;
use hiroz_msgs::sensor_msgs::LaserScan;

let cache = node.create_cache::<LaserScan>("/scan", 100).build()?;
```

### ExtractorStamp (application-level)

Supply a closure that extracts a `SystemTime` from the deserialized message. Use this when you need messages aligned by their logical capture time instead of network arrival time — the classic sensor fusion use case.

```rust
use hiroz::prelude::*;
use hiroz_msgs::sensor_msgs::Imu;
use std::time::{Duration, SystemTime};

let cache = node
    .create_cache::<Imu>("/imu/data", 200)
    .with_stamp(|msg: &Imu| {
        // Read header.stamp from the message.
        SystemTime::UNIX_EPOCH
            + Duration::new(msg.header.stamp.sec as u64, msg.header.stamp.nanosec)
    })
    .build()?;
```

## Query API

All query methods take and return `SystemTime` values and return clones of the stored messages. They are safe to call from multiple threads.

### `get_interval(t_start, t_end) -> Vec<T>`

Returns all messages with timestamp in `[t_start, t_end]`, inclusive, ordered by timestamp ascending. If `t_start > t_end` the result is empty (no panic).

```rust
let msgs = cache.get_interval(
    SystemTime::now() - Duration::from_millis(500),
    SystemTime::now(),
);
```

### `get_before(t) -> Option<T>`

The most recent message with timestamp ≤ `t`. Returns `None` if the cache is empty or all messages are strictly after `t`.

```rust
let latest = cache.get_before(SystemTime::now());
```

### `get_after(t) -> Option<T>`

The earliest message with timestamp ≥ `t`. Returns `None` if the cache is empty or all messages are strictly before `t`.

```rust
let next = cache.get_after(camera_stamp);
```

### `get_nearest(t) -> Option<T>`

The message whose timestamp is nearest to `t` (either side). When two messages are equidistant, the cache returns the one with the earlier timestamp. Returns `None` if the cache is empty.

```rust
// Align an IMU sample to a camera frame timestamp.
let imu = imu_cache.get_nearest(camera_stamp);
```

### Introspection

```rust
cache.oldest_stamp()   // Option<SystemTime> — timestamp of the oldest entry
cache.newest_stamp()   // Option<SystemTime> — timestamp of the newest entry
cache.len()            // usize — number of messages currently stored
cache.is_empty()       // bool
cache.clear()          // remove all messages
```

## Capacity and Eviction

The cache holds at most `capacity` messages. When a new message arrives and the cache is full, the cache evicts the oldest message (smallest timestamp) first, regardless of insertion order. Change the capacity at build time:

```rust
let cache = node
    .create_cache::<RosString>("/topic", 10)  // initial capacity
    .with_capacity(50)                         // override
    .build()?;
```

## Comparison with `message_filters::Cache<T>`

`ZCache<T>` covers the core `message_filters::Cache<T>` API with some differences:

| Feature | `message_filters::Cache<T>` | `ZCache<T>` |
|---------|----------------------------|-------------|
| Storage | `std::deque` (insertion-sorted) | `BTreeMap` (always sorted, O(log n) range) |
| Stamp strategies | Runtime function pointer (`hasHeader` / receive time) | **Compile-time** type-state (`ZenohStamp` / `ExtractorStamp`) |
| Headerless messages | `allow_headerless` flag, falls back to receive time | `ZenohStamp` handles any type — no header needed |
| `getInterval` | ✅ | ✅ `get_interval` |
| `getElemBeforeTime` | Strict `< t` (exclusive) | `get_before`: inclusive `≤ t` |
| `getElemAfterTime` | Strict `> t` (exclusive) | `get_after`: inclusive `≥ t` |
| `getSurroundingInterval` | ✅ | Not implemented |
| `get_nearest` | Not present | ✅ nearest-neighbor lookup |
| `clear` / `len` / `is_empty` | Not present | ✅ |
| Filter chaining (`signalMessage`) | ✅ (passes through to downstream) | Not applicable (hiroz has no filter chain) |
| Subscriber lifecycle | External, via `connectInput` | Owned by `ZCache`, dropped on `ZCache` drop |

!!! note
    `get_before` and `get_after` use **inclusive** bounds (`≤` / `≥`), unlike the C++ `getElemBeforeTime` / `getElemAfterTime` which are **exclusive** (`<` / `>`). This matches the query semantics of `get_interval`.

## Running the Examples

Three focused examples demonstrate the cache. Run them in separate terminals:

```bash
# Terminal 1 — publisher (sends msg-0, msg-1, … every 100 ms)
cargo run --example z_cache_talker

# Terminal 2 — cache consumer with Zenoh transport timestamp (default)
cargo run --example z_cache_zenoh_stamp

# Terminal 2 — cache consumer with application-level timestamp
cargo run --example z_cache_app_stamp
```

## Resources

- **[Pub/Sub](./pubsub.md)** — Publisher and subscriber patterns that `ZCache` builds on
- **[Services](./services.md)** — Request/reply communication
- **API docs** — `cargo doc -p hiroz --open`, then navigate to `hiroz::cache`
