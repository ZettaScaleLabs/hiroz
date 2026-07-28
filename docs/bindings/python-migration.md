# Migrating from rclpy

A practical guide for ROS 2 Python (`rclpy`) developers moving to `hiroz-py`. hiroz-py is a Python binding over the pure-Rust hiroz stack, which speaks ROS 2 over Zenoh. It deliberately keeps a **reactive, pull-based core** — there is no `rclpy.spin()` / executor — but the API has been aligned so most rclpy code maps over with mechanical changes.

## Mental-Model Differences

| Concept | rclpy | hiroz-py |
|---|---|---|
| Event loop | `rclpy.spin(node)` drives callbacks | **No spin / no executor.** You pull, or you register a callback that fires on an internal thread. |
| Subscriptions | callback-only, driven by the executor | callback **or** queue: `sub.recv(timeout=...)` pulls; or pass `callback=` to fire on an internal thread |
| Services (server) | callback-only | pull by default (`take_request` / `send_response`); pass `callback=` for rclpy-style auto-response |
| Lifecycle | `rclpy.init()` / `rclpy.shutdown()` | build a `ZContext`; it shuts down on drop or `ctx.shutdown()` |
| Context | global, implicit | explicit `ZContext` object (use it as a context manager) |
| Args order | `create_publisher(msg_type, topic, qos)` | `create_publisher(topic, msg_type, qos)` — **topic first** (pass by keyword to avoid confusion) |

The most important consequence: **there is no `spin()`**. A talker just publishes in a loop. A listener either calls `sub.recv()` in a loop or registers a callback and then does its own waiting (e.g. `time.sleep`, an `Event`, or its own work loop).

## Side-by-Side Cheatsheet

### Publisher / Subscriber

```python
# rclpy
import rclpy
from rclpy.node import Node
from std_msgs.msg import String

rclpy.init()
node = Node("talker")
pub = node.create_publisher(String, "/chatter", 10)
pub.publish(String(data="hi"))

def cb(msg): print(msg.data)
node.create_subscription(String, "/chatter", cb, 10)
rclpy.spin(node)
```

```python
# hiroz-py
import hiroz_py
from hiroz_py import std_msgs

ctx = hiroz_py.ZContextBuilder().with_connect_endpoints(["tcp/127.0.0.1:7447"]).build()
node = ctx.create_node("talker").build()

pub = node.create_publisher("/chatter", std_msgs.String, qos=10)   # topic first; int qos OK
pub.wait_for_subscription(count=1, timeout=5.0)                    # no sleep races
pub.publish(std_msgs.String(data="hi"))

def cb(msg): print(msg.data)
node.create_subscription("/chatter", std_msgs.String, callback=cb)  # alias of create_subscriber
# ... no spin(); do your own waiting/work here ...
```

Queue-style subscriber (no callback):

```python
sub = node.create_subscriber("/chatter", std_msgs.String)
msg = sub.recv(timeout=1.0)   # returns None on timeout
```

### Service Client

```python
# rclpy
from example_interfaces.srv import AddTwoInts
cli = node.create_client(AddTwoInts, "/add_two_ints")
cli.wait_for_service()
fut = cli.call_async(AddTwoInts.Request(a=2, b=3))
rclpy.spin_until_future_complete(node, fut)
print(fut.result().sum)
```

```python
# hiroz-py
from hiroz_py import example_interfaces
cli = node.create_client("/add_two_ints", example_interfaces.AddTwoInts)  # grouping type
if not cli.wait_for_service(timeout=5.0):
    raise hiroz_py.HirozError("service unavailable")
resp = cli.call(example_interfaces.AddTwoInts.Request(a=2, b=3), timeout=5.0)  # blocking
print(resp.sum)
```

### Service Server — Callback Style (rclpy-like)

```python
# rclpy
def handle(req, resp):
    resp.sum = req.a + req.b
    return resp
node.create_service(AddTwoInts, "/add_two_ints", handle)
rclpy.spin(node)
```

```python
# hiroz-py (callback returns the response; no resp out-param)
def handle(req):
    return example_interfaces.AddTwoInts.Response(sum=req.a + req.b)
# Keep the returned server alive: dropping it stops the worker and tears down
# the queryable. Binding it is what keeps the internal thread serving.
server = node.create_service(
    "/add_two_ints", example_interfaces.AddTwoInts, callback=handle
)
# server runs on an internal thread; keep the process alive (no spin needed)
```

### Service Server — Pull Style (hiroz-native)

```python
server = node.create_server("/add_two_ints", example_interfaces.AddTwoInts)
while True:
    request_id, req = server.take_request()       # blocks
    server.send_response(
        example_interfaces.AddTwoInts.Response(sum=req.a + req.b), request_id
    )
```

### Action Client

```python
# rclpy
from rclpy.action import ActionClient
from action_tutorials_interfaces.action import Fibonacci
ac = ActionClient(node, Fibonacci, "/fibonacci")
ac.wait_for_server()
fut = ac.send_goal_async(Fibonacci.Goal(order=10))
...
```

```python
# hiroz-py (Python actions are Python-to-Python via msgpack; not rmw_zenoh_cpp interop)
ac = node.create_action_client("/fibonacci", Fibonacci)   # single grouping type
if not ac.wait_for_server(timeout=5.0):
    raise hiroz_py.HirozError("action server unavailable")
handle = ac.send_goal(Fibonacci.Goal(order=10))           # blocks until accepted
while (fb := handle.recv_feedback(timeout=0.5)) is not None:
    print(fb)
result = handle.get_result(timeout=10.0)                  # raises hiroz_py.TimeoutError on timeout
```

If you don't have a generated grouping class, pass the three classes positionally (back-compat):

```python
ac = node.create_action_client("/fibonacci", FibGoal, FibResult, FibFeedback)
```

### Action Server

```python
server = node.create_action_server("/fibonacci", Fibonacci)   # or 3 positional types
while True:
    request = server.recv_goal(timeout=1.0)
    if request is None:
        continue
    goal = request.goal()
    executing = request.accept_and_execute()
    executing.publish_feedback(Fibonacci.Feedback(...))
    if executing.is_cancel_requested:
        executing.canceled(Fibonacci.Result(...))
    else:
        executing.succeed(Fibonacci.Result(...))
```

## API Name Mapping

| rclpy | hiroz-py | Notes |
|---|---|---|
| `rclpy.init()` | `ZContextBuilder()...build()` | explicit context object |
| `rclpy.shutdown()` | `ctx.shutdown()` or context-manager exit | |
| `Node("name")` | `ctx.create_node("name").build()` | builder pattern |
| `node.create_publisher(T, topic, qos)` | `node.create_publisher(topic, T, qos=...)` | **topic first** |
| `node.create_subscription(T, topic, cb, qos)` | `node.create_subscription(topic, T, callback=cb, qos=...)` | alias of `create_subscriber` |
| `node.create_client(Srv, name)` | `node.create_client(name, Srv)` | `Srv` = grouping type or bare Request |
| `node.create_service(Srv, name, cb)` | `node.create_service(name, Srv, callback=cb)` | alias of `create_server`; pull mode if no callback |
| `ActionClient(node, Act, name)` | `node.create_action_client(name, Act)` | grouping type or 3 classes |
| `ActionServer(node, Act, name, cb)` | `node.create_action_server(name, Act)` | reactive loop, not a callback |
| `client.wait_for_service(t)` | `client.wait_for_service(timeout=t)` | returns `bool` |
| `action_client.wait_for_server(t)` | `action_client.wait_for_server(timeout=t)` | returns `bool` |
| *(rclpy has no direct equivalent)* | `pub.wait_for_subscription(count, timeout)` | returns `bool` |
| `client.call_async(req)` + spin | `client.call(req, timeout=...)` | **blocking** call, returns the response |
| `sub` callback (executor) | `sub.recv(timeout=...)` **or** `callback=` | pull or push |
| `node.get_logger().info(...)` | *(use Python `logging` / `print`)* | rosout not implemented |
| `node.create_timer(...)` | *(not implemented)* | see [What's Not There Yet](#whats-not-there-yet) |
| `node.declare_parameter(...)` | *(not implemented)* | see [What's Not There Yet](#whats-not-there-yet) |

## Message Types

Messages are `msgspec.Struct`s from `hiroz_msgs_py` (re-exported by `hiroz_py`). Construct with keyword args:

```python
from hiroz_py import std_msgs, geometry_msgs
m = std_msgs.String(data="hi")
v = geometry_msgs.Twist(linear=geometry_msgs.Vector3(x=1.0))
```

### Services: `AddTwoInts.Request` / `.Response`

Each `.srv` generates three Python objects:

- `AddTwoIntsRequest` — the request struct
- `AddTwoIntsResponse` — the response struct
- `AddTwoInts` — a **grouping class** exposing `__srvtype__`, `.Request`, and `.Response`

This mirrors rclpy's `AddTwoInts.Request`. Pass the grouping class to `create_client` / `create_server` (preferred), or the bare `AddTwoIntsRequest` class (still supported for back-compat):

```python
example_interfaces.AddTwoInts.Request(a=1, b=2)      # rclpy-style
example_interfaces.AddTwoIntsRequest(a=1, b=2)       # equivalent, also works
```

### Actions: `Fibonacci.Goal` / `.Result` / `.Feedback`

`create_action_client` / `create_action_server` accept a single grouping class exposing `__actiontype__`, `.Goal`, `.Result`, `.Feedback`. If you define inline msgspec types, you can build your own grouping class:

```python
class CountTo:
    __actiontype__ = "my_pkg/action/CountTo"
    Goal = CountToGoal
    Result = CountToResult
    Feedback = CountToFeedback

node.create_action_client("/count", CountTo)
```

The 3-positional-class form (`create_action_client(name, Goal, Result, Feedback)`) still works.

!!! warning
    hiroz-py actions use a msgpack wire format and are **Python-to-Python only** — they do not interoperate with `rmw_zenoh_cpp` typed actions. Pub/sub and services *do* interoperate.

## QoS

Three ways to specify QoS, all accepted anywhere a `qos=` argument appears:

```python
# 1. Int depth shorthand (rclpy-style) -> KeepLast(n)
node.create_publisher("/t", std_msgs.String, qos=10)

# 2. Enum-like policy constants (discoverable, typo-proof)
from hiroz_py import QosProfile, ReliabilityPolicy, HistoryPolicy
qos = QosProfile(
    reliability=ReliabilityPolicy.BEST_EFFORT,
    history=HistoryPolicy.KEEP_LAST,
    depth=5,
)
node.create_subscription("/scan", sensor_msgs.LaserScan, qos=qos)

# 3. Presets
node.create_publisher("/t", std_msgs.String, qos=hiroz_py.QOS_SENSOR_DATA)
```

Available policy holders (string-valued, mirroring `rclpy.qos`):

- `ReliabilityPolicy.RELIABLE` / `.BEST_EFFORT`
- `DurabilityPolicy.VOLATILE` / `.TRANSIENT_LOCAL`
- `HistoryPolicy.KEEP_LAST` / `.KEEP_ALL`
- `LivelinessPolicy.AUTOMATIC` / `.MANUAL_BY_TOPIC` / `.MANUAL_BY_NODE`

Plain strings (`reliability="best_effort"`) and dicts still work.

## Error Handling

hiroz-py raises a small exception hierarchy (all importable from `hiroz_py`):

```text
RuntimeError                          (builtin)
└── HirozError                        (base — catch this to cover everything)
    ├── TimeoutError                  (a blocking call timed out)
    │   └── also inherits builtins.TimeoutError
    ├── SerializationError            (CDR/msgpack encode/decode failure)
    └── TypeMismatchError             (type hash / type mismatch)
```

The two extra bases exist so that ported code keeps working unchanged: `HirozError` inherits `RuntimeError` because that is what these paths raised before the typed hierarchy existed, and `TimeoutError` additionally inherits the **builtin** `TimeoutError` because that is what rclpy's `Client.call` raises.

```python
import hiroz_py
try:
    resp = client.call(req, timeout=2.0)
except hiroz_py.TimeoutError:
    ...   # the server was present but slow
except hiroz_py.HirozError as e:
    ...   # any other call failure (e.g. no server responded)
```

Notes:

- `hiroz_py.TimeoutError` **is** catchable as Python's builtin `TimeoutError`, as well as `hiroz_py.HirozError` and `RuntimeError`. An `except TimeoutError:` block ported straight from rclpy keeps working. Because the builtin derives from `OSError`, these instances are `OSError`s too — the same as in rclpy.
- A service call with **no server present at all** fails fast with a plain `HirozError` (not a timeout) — guard with `wait_for_service()` first. Timeout classification requires a server that matched but did not respond in time.
- `recv(...)` and `recv_goal(...)` return **`None`** on timeout rather than raising — that is their documented contract. `get_result(...)` is the exception: it raises `hiroz_py.TimeoutError` on timeout, matching `ZClient.call`.

## What's Not There Yet

Unreachable from Python today. The **Status** column distinguishes two very different cases: some of these exist in hiroz core and merely lack a Python surface, while others are unimplemented in core as well.

| Feature | Status | Workaround |
|---|---|---|
| Parameters (`declare_parameter`, parameter server) | implemented in core (`ZNode`), **not exposed to Python** | plain Python config / env vars |
| Lifecycle nodes | implemented in core, **not exposed to Python** | manage state yourself |
| Sim time / clock | implemented in core (`ZClock`), **not exposed to Python** | `time.time()` |
| Timers (`create_timer`) | not implemented in core | `time.sleep` in your own loop / a `threading.Timer` |
| Logging (`get_logger()` / rosout) | not implemented in core | Python `logging` or `print` |
| Executors / `spin()` | by design — hiroz is reactive, with no spin loop | pull (`recv`) or `callback=` |
| Action ROS 2 interop | Python-to-Python only (msgpack wire format) | use typed Rust actions for `rmw_zenoh_cpp` interop |

Pub/sub and services **do** interoperate with standard ROS 2 nodes through the Zenoh RMW.

## Migration Checklist

Mechanical steps to port an rclpy node:

1. **Context**: replace `rclpy.init()` / `Node(...)` / `rclpy.shutdown()` with
   `ctx = hiroz_py.ZContextBuilder().with_connect_endpoints(["tcp/127.0.0.1:7447"]).build()`
   and `node = ctx.create_node("name").build()`.
2. **Imports**: `from std_msgs.msg import String` → `from hiroz_py import std_msgs` and use `std_msgs.String`. Same for `srv`/`action` packages.
3. **Flip pub/sub arg order**: `create_publisher(T, topic, qos)` → `create_publisher(topic, T, qos=qos)`. Easiest safe edit: pass by keyword — `create_publisher(topic=..., msg_type=..., qos=...)`. (If you leave the rclpy order, you get a clear `TypeError` telling you they look swapped.)
4. **Rename calls** (or rely on aliases): `create_subscription` and `create_service` both exist as aliases; `create_client` is the same name. Action: `ActionClient(node, A, name)` → `node.create_action_client(name, A)`.
5. **Services**: pass the grouping class (`pkg.Srv`) instead of `pkg.Srv.Request` where you can. For servers, either keep a `callback=` (rclpy-style, but the callback **returns** the response rather than mutating an out-param) or switch to the pull loop.
6. **Service client calls**: `call_async()` + `spin_until_future_complete()` → blocking `client.call(req, timeout=...)`. Add `client.wait_for_service(timeout=...)` before the first call.
7. **Remove `rclpy.spin(node)`**: replace with your own loop. For queue subscribers, loop on `sub.recv(timeout=...)`. For callback subscribers/servers, the work happens on internal threads — just keep the process alive (e.g. `while True: time.sleep(1)` or block on an `Event`).
8. **QoS**: `qos_profile=10` → `qos=10`; `QoSProfile(reliability=ReliabilityPolicy.BEST_EFFORT, depth=5)` → `hiroz_py.QosProfile(reliability=hiroz_py.ReliabilityPolicy.BEST_EFFORT, depth=5)`.
9. **Exceptions**: nothing to change — `except RuntimeError:` and `except TimeoutError:` both still catch, by design. Tighten to `except hiroz_py.HirozError:` / `except hiroz_py.TimeoutError:` when you want to catch hiroz failures specifically rather than any runtime error.
10. **Drop sleeps used for discovery**: replace `time.sleep(1.0)` before first publish/call with `pub.wait_for_subscription(...)`, `client.wait_for_service(...)`, or `action_client.wait_for_server(...)`.
11. **Audit unsupported features**: remove or replace timers, parameters, logging, lifecycle (see [What's Not There Yet](#whats-not-there-yet)).

A useful first sweep (review each hit by hand — these are starting points, not blind rewrites):

```bash
grep -rn "rclpy.spin\|create_timer\|declare_parameter\|get_logger\|call_async\|spin_until_future_complete" your_pkg/
```
