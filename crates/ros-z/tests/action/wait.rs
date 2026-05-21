use std::{sync::Arc, time::Duration};

use ros_z::{Builder, Result, action::GoalStatus, context::ZContextBuilder, define_action};
use serde::{Deserialize, Serialize};
use serial_test::serial;
use tokio::time;

// Define test action messages (similar to Fibonacci)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestGoal {
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFeedback {
    pub progress: i32,
}

// Define the action type
pub struct TestAction;

define_action! {
    TestAction,
    action_name: "test_action",
    Goal: TestGoal,
    Result: TestResult,
    Feedback: TestFeedback,
}

// Helper function to create test setup
#[allow(dead_code)]
async fn setup_test_base() -> Result<(ros_z::node::ZNode,)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_action_wait_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node,))
}

// Helper function to create test setup with client and server
async fn setup_test_with_client_server() -> Result<(
    ros_z::node::ZNode,
    ros_z::node::ZNode,
    std::sync::Arc<ros_z::action::client::ZActionClient<TestAction>>,
    ros_z::action::server::ZActionServer<TestAction>,
)> {
    let ctx = ZContextBuilder::default().build()?;

    let client_node = ctx.create_node("test_action_client_wait_node").build()?;
    let server_node = ctx.create_node("test_action_server_wait_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = Arc::new(
        client_node
            .create_action_client::<TestAction>("/test_action_wait_name")
            .build()?,
    );

    let server = server_node
        .create_action_server::<TestAction>("/test_action_wait_name")
        .build()?;

    Ok((client_node, server_node, client, server))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_action_client_async_wait_timeout() -> Result<()> {
        let (_client_node, _server_node, client, server) = setup_test_with_client_server().await?;

        // Spawn a server task that accepts but never completes goals
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Ok(requested) = server_clone.recv_goal().await {
                let _accepted = requested.accept();
                // Don't execute - just accept and do nothing
                // This leaves the goal in Accepted state indefinitely
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });

        // Test that async operations can timeout properly
        // This tests the async waiting behavior similar to wait sets
        let goal = TestGoal { order: 5 };

        // Send a goal and wait for result with timeout
        let goal_handle = client.send_goal(goal).await?;

        // Try to get result with a short timeout - should timeout since server never completes
        let result_future = goal_handle.result();
        let timeout_result = time::timeout(Duration::from_millis(100), result_future).await;

        // Should timeout since no server response
        assert!(timeout_result.is_err());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_feedback_wait() -> Result<()> {
        let (_client_node, _server_node, client, server) = setup_test_with_client_server().await?;

        // Start server processing in background
        let server_clone = server.clone();
        tokio::spawn(async move {
            // FIXME: Using `if` instead of `while` because this test only sends one goal.
            // Real applications using `while` loops should implement proper shutdown handling.
            if let Ok(requested) = server_clone.recv_goal().await {
                let accepted = requested.accept();
                let executing = accepted.execute();
                // Send some feedback
                let _ = executing.publish_feedback(TestFeedback { progress: 50 });
                // Complete the goal
                let _ = executing.succeed(TestResult { value: 10 });
            }
        });

        let goal = TestGoal { order: 5 };
        let mut goal_handle = client.send_goal(goal).await?;

        // Test waiting for feedback
        let mut feedback_stream = goal_handle.feedback().unwrap();
        let feedback = time::timeout(Duration::from_millis(1000), feedback_stream.recv()).await?;
        let feedback = feedback.unwrap();

        assert_eq!(feedback.progress, 50);

        // Test waiting for result
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 10);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_async_wait() -> Result<()> {
        let (_client_node, _server_node, _client, server) = setup_test_with_client_server().await?;

        // Test that server can wait for goal requests asynchronously
        let recv_future = server.recv_goal();
        let timeout_result = time::timeout(Duration::from_millis(100), recv_future).await;

        // Should timeout since no client sent a goal
        assert!(timeout_result.is_err());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_concurrent_async_operations() -> Result<()> {
        let (_client_node, _server_node, client, server) = setup_test_with_client_server().await?;

        // Test concurrent async operations (similar to wait set with multiple entities)
        let server_clone = server.clone();
        let client_clone = client.clone();

        // Spawn server task
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            let accepted = requested.accept();
            let executing = accepted.execute();
            executing.succeed(TestResult { value: 42 })?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });

        // Spawn client task
        let client_task = tokio::spawn(async move {
            let goal_handle = client_clone.send_goal(TestGoal { order: 10 }).await?;
            let result = goal_handle.result().await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(result)
        });

        // Wait for both tasks to complete
        let (server_result, client_result) = tokio::try_join!(server_task, client_task)?;

        server_result?;
        let result = client_result?;
        assert_eq!(result.value, 42);

        Ok(())
    }

    /// Tests that status watch correctly receives status changes asynchronously.
    ///
    /// **Race Condition Prevention:**
    /// - Uses explicit synchronization (oneshot channel) to ensure client is ready before server processes
    /// - Client uses `borrow_and_update()` to mark initial status as "seen"
    /// - Without these, status could transition Unknown->Accepted->Executing->Succeeded
    ///   before `changed()` is called, causing it to wait forever for a change that already happened
    #[serial]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_action_status_async_wait() -> Result<()> {
        use tokio::sync::oneshot;

        let (_client_node, _server_node, client, server) = setup_test_with_client_server().await?;

        // Synchronization: client signals when ready to observe status changes
        let (ready_tx, ready_rx) = oneshot::channel();

        // Start server that will update status
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Ok(requested) = server_clone.recv_goal().await {
                let accepted = requested.accept();

                // Wait for client to be ready to observe status changes
                let _ = ready_rx.await;

                // Now proceed with status transitions
                let executing = accepted.execute();
                // Yield so the tokio scheduler can deliver Executing status to the
                // client before Succeeded is published.  Without this, both
                // transitions can be coalesced and the second changed() call never
                // fires, causing a timeout on macOS CI.
                tokio::task::yield_now().await;
                let _ = executing.succeed(TestResult { value: 100 });
            }
        });

        let goal = TestGoal { order: 5 };
        let mut goal_handle = client.send_goal(goal).await?;

        // Test waiting for status changes
        let mut status_watch = goal_handle.status_watch().unwrap();

        // send_goal() seeds the watch to Accepted before returning, so borrow_and_update()
        // here always consumes Accepted (never Unknown). Exactly 2 further transitions
        // (Executing, Succeeded) will be observed below.
        let initial_status = *status_watch.borrow_and_update();
        tracing::debug!("Initial status: {:?}", initial_status);

        // Signal server that we're ready to observe status changes
        let _ = ready_tx.send(());

        // Wait for status changes until we reach a terminal state.
        // execute() and succeed() may fire back-to-back before the first
        // changed() call returns, so the watch channel may batch them into a
        // single notification.  Loop until Succeeded/Aborted/Canceled rather
        // than assuming a fixed number of intermediate transitions.
        loop {
            time::timeout(Duration::from_secs(5), status_watch.changed())
                .await
                .expect("timeout waiting for status change")?;
            let status = *status_watch.borrow_and_update();
            tracing::debug!("Status: {:?}", status);
            if matches!(
                status,
                GoalStatus::Succeeded | GoalStatus::Aborted | GoalStatus::Canceled
            ) {
                break;
            }
        }

        Ok(())
    }

    // TODO: Additional tests would cover:
    // - Multiple concurrent goals
    // - Cancellation waiting
    // - Feedback streaming with timeouts
    // - Server-side waiting for different request types (goal, cancel, result)
}
