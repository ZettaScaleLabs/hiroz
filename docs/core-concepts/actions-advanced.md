# Actions — Advanced Patterns

This page covers the full Fibonacci examples, GoalHandle API, cancellation contract details, and defining custom action types. For the core concepts and minimal patterns, see [Actions](./actions.md).

## Action Server Example

This example demonstrates an action server that computes Fibonacci sequences. The server accepts goals, publishes periodic feedback with partial results, and supports cancellation.

```rust
/// Fibonacci action server node that computes Fibonacci sequences
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `timeout` - Optional timeout duration. If None, runs until ctrl+c.
pub async fn run_fibonacci_action_server(ctx: ZContext, timeout: Option<Duration>) -> Result<()> {
    // Create a node named "fibonacci_action_server"
    let node = ctx.create_node("fibonacci_action_server").build()?;

    // Create an action server
    // Note: The server variable must be kept alive for the duration of the function
    // to ensure the action server and its background tasks remain active
    let _server = node
        .create_action_server::<Fibonacci>("fibonacci")
        .build()?
        .with_handler(|executing: ExecutingGoal<Fibonacci>| async move {
            let order = executing.goal.order;
            let mut sequence = vec![0, 1];

            println!("Executing Fibonacci goal with order {}", order);

            let mut canceled = false;
            let mut cancel_sequence = None;

            for i in 2..=order {
                // Check for cancellation
                if executing.is_cancel_requested() {
                    println!("Goal canceled!");
                    canceled = true;
                    cancel_sequence = Some(sequence.clone());
                    break;
                }

                let next = sequence[i as usize - 1] + sequence[i as usize - 2];
                sequence.push(next);

                // Publish feedback
                // Distro-specific feedback field names
                #[cfg(feature = "kilted")]
                let feedback = FibonacciFeedback {
                    sequence: sequence.clone(),
                };
                #[cfg(not(feature = "kilted"))]
                let feedback = FibonacciFeedback {
                    partial_sequence: sequence.clone(),
                };
                executing
                    .publish_feedback(feedback)
                    .expect("Failed to publish feedback");

                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            if canceled {
                executing
                    .canceled(FibonacciResult {
                        sequence: cancel_sequence.unwrap(),
                    })
                    .unwrap();
            } else {
                println!("Goal succeeded!");
                executing.succeed(FibonacciResult { sequence }).unwrap();
            }
        });

    println!("Fibonacci action server started");

    if let Some(timeout) = timeout {
        // For testing: run for the specified timeout
        tokio::time::sleep(timeout).await;
    } else {
        tokio::signal::ctrl_c().await?;
    }

    Ok(())
}
```

**Key points:**

- **Handler Pattern**: Uses `.with_handler()` to define asynchronous goal execution
- **Feedback Publishing**: Sends partial results periodically via `publish_feedback()`
- **Cancellation Support**: Checks `is_cancel_requested()` and handles graceful cancellation
- **Completion**: Uses `.succeed()` or `.canceled()` to send final result

**Running the server** (clone the repo first: `git clone https://github.com/ZettaScaleLabs/hiroz.git && cd hiroz`):

```bash
# Start Eclipse Zenoh router first
cargo run --example zenoh_router

# Run the server (runs until Ctrl+C)
cargo run --example demo_nodes_fibonacci_action_server
```

## Action Client Example

This example demonstrates an action client that sends goals and monitors execution progress with feedback updates.

```rust
/// Fibonacci action client node that sends goals to compute Fibonacci sequences
///
/// # Arguments
/// * `ctx` - The hiroz context
/// * `order` - The order of the Fibonacci sequence to compute
pub async fn run_fibonacci_action_client(ctx: ZContext, order: i32) -> Result<Vec<i32>> {
    // Create a node named "fibonacci_action_client"
    let node = ctx.create_node("fibonacci_action_client").build()?;

    // Create an action client
    let client = node
        .create_action_client::<Fibonacci>("fibonacci")
        .build()?;

    // Wait a bit for the server to be discovered
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    println!(
        "Fibonacci action client started, sending goal with order {}",
        order
    );

    // Send the goal
    let mut goal_handle = client.send_goal(FibonacciGoal { order }).await?;
    println!("Goal sent and accepted!");

    // Set up feedback monitoring
    if let Some(mut feedback_stream) = goal_handle.feedback() {
        tokio::spawn(async move {
            while let Some(fb) = feedback_stream.recv().await {
                // Distro-specific feedback field names
                #[cfg(feature = "kilted")]
                println!("Feedback: {:?}", fb.sequence);
                #[cfg(not(feature = "kilted"))]
                println!("Feedback: {:?}", fb.partial_sequence);
            }
        });
    }

    // Wait for the result with timeout
    println!("Waiting for result (timeout: 10s)...");
    let result = match tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        goal_handle.result(),
    )
    .await
    {
        Ok(Ok(result)) => {
            println!("Final result: {:?}", result.sequence);
            result
        }
        Ok(Err(e)) => {
            eprintln!("Action failed: {}", e);
            return Err(e);
        }
        Err(_) => {
            eprintln!("Timeout waiting for action result");
            return Err(zenoh::Error::from("Timeout waiting for action result"));
        }
    };

    Ok(result.sequence)
}
```

**Key points:**

- **Goal Sending**: Uses `send_goal()` to submit goals and get a handle
- **Feedback Monitoring**: Spawns async task to receive and display feedback
- **Result Handling**: Waits for completion with timeout and error handling
- **Type Safety**: Strongly-typed goal, feedback, and result messages

**Running the client:**

```bash
# Basic usage - compute Fibonacci(10)
cargo run --example demo_nodes_fibonacci_action_client

# Compute Fibonacci(15)
cargo run --example demo_nodes_fibonacci_action_client -- --order 15

# Connect to specific router
cargo run --example demo_nodes_fibonacci_action_client -- --endpoint tcp/localhost:7447
```

## Complete Action Workflow

!!! note
    These commands run the ready-made examples from the hiroz repository. Clone it first with `git clone https://github.com/ZettaScaleLabs/hiroz.git && cd hiroz`. If you're building your own project, run your binaries with `cargo run` instead.

**Terminal 1 - Start Zenoh Router:**

```bash
cargo run --example zenoh_router
```

**Terminal 2 - Start Action Server:**

```bash
cargo run --example demo_nodes_fibonacci_action_server
```

**Terminal 3 - Send Goals from Client:**

```bash
cargo run --example demo_nodes_fibonacci_action_client -- --order 10
```

You'll see:

- **Client**: Goal sent, feedback updates with partial sequences, final result
- **Server**: Goal received, executing with feedback, completion status

!!! warning
    Always implement timeout mechanisms for action clients. Long-running actions can fail or hang, and clients need graceful degradation strategies.

## Canceling an Action from the Client

The `GoalHandle` returned by `send_goal()` has a `cancel()` method for requesting cancellation:

```rust
let goal_handle = client.send_goal(goal).await?;

// ... later, if you need to stop the action:
let cancel_response = goal_handle.cancel().await?;
println!("Cancel accepted: {}", cancel_response.goals_canceling.len() > 0);
```

To cancel a specific goal by ID without a handle:

```rust
client.cancel_goal(goal_id).await?;
```

To cancel every in-progress goal at once:

```rust
client.cancel_all_goals().await?;
```

The server checks cancellation with `is_cancel_requested()` and calls `.canceled()` to complete the handshake (see Server section above). There is no guarantee the server will honor the cancellation — it may complete the goal before processing the cancel request.

## GoalHandle API Reference

`send_goal()` returns a `GoalHandle<A>`. All interaction with an in-flight goal goes through this handle:

| Method | Returns | Notes |
|--------|---------|-------|
| `goal_handle.id()` | `GoalId` | Unique identifier for this goal |
| `goal_handle.feedback()` | `Option<UnboundedReceiver<A::Feedback>>` | First call only — receiver is moved out |
| `goal_handle.status_watch()` | `Option<watch::Receiver<GoalStatus>>` | First call only — watcher is moved out |
| `goal_handle.cancel()` | `Result<CancelGoalServiceResponse>` | Requests server cancellation |
| `goal_handle.result()` | `Result<A::Result>` | Waits for terminal state, then fetches result |

**Receiving feedback:**

```rust
// Take the feedback receiver — this works exactly once per GoalHandle
if let Some(mut rx) = goal_handle.feedback() {
    while let Some(fb) = rx.recv().await {
        println!("Feedback: {:?}", fb);
    }
    // Loop ends when the server drops its feedback sender (goal completes)
}
```

**Watching status transitions:**

```rust
if let Some(mut status_rx) = goal_handle.status_watch() {
    while status_rx.changed().await.is_ok() {
        println!("Status: {:?}", *status_rx.borrow());
    }
}
```

**Waiting for the result:**

```rust
// Blocks until the goal reaches a terminal state (Succeeded, Canceled, or Aborted)
// then fetches and returns the final result
let result = goal_handle.result().await?;
```

!!! tip
    Spawn `feedback()` in a separate task while awaiting `result()` in the main task. Both can run concurrently — `result()` does not consume feedback messages.

## Defining a Custom Action Type

The examples above use the pre-built `Fibonacci` action from `hiroz-msgs`. To define your own action type, implement the `ZAction` trait:

```rust
use serde::{Deserialize, Serialize};
use hiroz::action::ZAction;

// Define the three message structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountGoal {
    pub target: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountFeedback {
    pub current: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountResult {
    pub message: String,
}

// Tie them together with ZAction
pub struct CountAction;

impl ZAction for CountAction {
    type Goal = CountGoal;
    type Feedback = CountFeedback;
    type Result = CountResult;

    fn name() -> &'static str {
        "my_robot/count_to_n"
    }
}
```

**Key points:**

- `Goal`, `Feedback`, and `Result` can be any struct that implements `Serialize + Deserialize + Clone + Send + Sync + 'static` (blanket implementation via `ZMessage`)
- `name()` sets the Zenoh key prefix for the action's internal services and topics
- The default `send_goal_type_info()`, `get_result_type_info()`, etc. all return `TypeHash::zero()`, which is correct for hiroz-to-hiroz communication. For ROS 2 interop, override these with the correct RIHS01 hashes.

!!! tip
    For hiroz-to-hiroz-only actions, the defaults work without any additional configuration. For ROS 2 interop, use schema-generated types from a `.action` file via `hiroz-codegen` — see [Custom Messages](../user-guide/custom-messages.md).

## Comparison with Other Patterns

| Pattern | Duration | Feedback | Cancellation | Use Case |
|---------|----------|----------|--------------|----------|
| **Pub-Sub** | Continuous | No | N/A | Sensor data streaming |
| **Service** | < 1 second | No | No | Quick queries |
| **Action** | Seconds to minutes | Yes | Yes | Long-running tasks |

## Resources

- **[ROS 2 Actions Documentation](https://docs.ros.org/en/rolling/Tutorials/Intermediate/Writing-an-Action-Server-Client/Py.html)** - Official ROS 2 action guide
- **[hiroz Examples](https://github.com/ZettaScaleLabs/hiroz/tree/main/crates/hiroz/examples)** - Working action implementations
- **[Services](./services.md)** - Simpler request-response pattern
- **[Custom Messages](../user-guide/custom-messages.md)** - Defining custom action types with `.action` files
