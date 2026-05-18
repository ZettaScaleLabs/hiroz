# ROS 2 → hiroz API Mapping

If you know rclcpp or rclpy, this table maps familiar concepts to their hiroz equivalents.

## Core Concepts

| rclcpp / rclpy | hiroz (Rust) | Notes |
|----------------|--------------|-------|
| `rclcpp::Node` | `ZNode` | Created via `ctx.create_node("name").build()?` |
| `rclcpp::Publisher<T>` | `ZPub<T>` | `node.create_pub::<T>("topic").build()?` |
| `rclcpp::Subscription<T>` | `ZSub<T>` | Three patterns: blocking `recv()`, async `async_recv()`, or callback via `build_with_callback()` |
| `rclcpp::Service<T>` | `ZServer<T>` | Pull model — call `take_request()` when ready to handle |
| `rclcpp::Client<T>` | `ZClient<T>` | `call(&req).await` or `call_with_timeout(&req, duration).await` |
| `rclcpp_action::Server<T>` | `ZActionServer<T>` | Uses `with_handler` closure receiving `ExecutingGoal<A>` |
| `rclcpp_action::Client<T>` | `ZActionClient<T>` | `send_goal()` returns a `GoalHandle` |
| `rclcpp::ParameterServer` | `ZParameterServer` | Auto-creates `ros2 param` compatible services |
| `rclcpp_lifecycle::LifecycleNode` | `ZLifecycleNode` | Same 4-state machine (Unconfigured → Inactive → Active → Finalized) |

## No Spin Loop

hiroz has no equivalent to `rclcpp::spin()`. hiroz is fully reactive: subscribers are either
callback-based or backed by an internal queue, and services use a pull model. Your entry point
is a standard `#[tokio::main]` async function — no spin thread required.

**rclcpp:**

```cpp
rclcpp::init(argc, argv);
auto node = std::make_shared<MyNode>();
rclcpp::spin(node);
rclcpp::shutdown();
```

**hiroz:**

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ZContextBuilder::default()
        .with_connect_endpoints(["tcp/127.0.0.1:7447"])
        .build()?;
    let node = ctx.create_node("my_node").build()?;
    // ... set up pubs, subs, services ...
    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

## Initialization

| Step | rclcpp | hiroz |
|------|--------|-------|
| Initialize runtime | `rclcpp::init(argc, argv)` | `ZContextBuilder::default()` |
| Connect to network | automatic (DDS multicast) | `.with_connect_endpoints(["tcp/127.0.0.1:7447"])` |
| Set domain ID | `--ros-args -p use_sim_time:=...` / env | `.with_domain_id(42)` |
| Build context | implicit | `.build()?` |
| Create node | `std::make_shared<rclcpp::Node>("name")` | `ctx.create_node("name").build()?` |

## QoS Presets

| rclcpp | hiroz | Notes |
|--------|-------|-------|
| `rclcpp::SensorDataQoS()` | `QosProfile::sensor_data()` | Best-effort, keep last 5 |
| `rclcpp::SystemDefaultsQoS()` | `QosProfile::default()` | Reliable, keep last 10 |
| `rclcpp::ServicesQoS()` | `QosProfile::services_default()` | Reliable, volatile |
| `rclcpp::QoS(10).reliable()` | `QosProfile { history: KeepLast(10), reliability: Reliable, .. Default::default() }` | Custom profile |

## Key Differences

- **No spin loop** — hiroz is fully async and reactive. Use `#[tokio::main]` and `await`; no executor management needed.
- **Rust ownership replaces shared_ptr** — no `Node::SharedPtr`, no reference counting in user code. The borrow checker enforces correct lifetime without manual memory management.
- **Type hashes required for ROS 2 interop** — when communicating with native ROS 2 nodes, message types must carry RIHS01 type hashes. Use schema-generated types from `hiroz-codegen` rather than hand-rolled structs.
- **Pull model for services** — there is no callback registration for service servers. Call `take_request()` when your application is ready to handle an incoming request.
- **Router required** — hiroz does not use DDS multicast discovery. A Zenoh router must be running and reachable at the configured endpoint (default: `tcp/127.0.0.1:7447`).
