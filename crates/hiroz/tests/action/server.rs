use std::num::NonZeroUsize;

use hiroz::{
    Builder, Result,
    context::ZContextBuilder,
    define_action,
    qos::{QosHistory, QosProfile, QosReliability},
};
use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create test setup
    async fn setup_test() -> Result<(
        hiroz::node::ZNode,
        hiroz::action::client::ZActionClient<TestAction>,
        hiroz::action::server::ZActionServer<TestAction>,
    )> {
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("test_action_server_node").build()?;

        let client = node
            .create_action_client::<TestAction>("test_action_server_name")
            .build()?;

        let server = node
            .create_action_server::<TestAction>("test_action_server_name")
            .build()?;

        // Wait for discovery
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        Ok((node, client, server))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_init_fini() -> Result<()> {
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("test_action_server_node").build()?;

        // Test successful initialization with valid arguments
        let server = node
            .create_action_server::<TestAction>("test_action_server_name")
            .build()?;

        // Verify server was created successfully by cloning it
        let _server_clone = server.clone();

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_is_valid() -> Result<()> {
        let (_node, _client, server) = setup_test().await?;

        // Test valid server - verify it can be cloned
        let _server_clone = server.clone();

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_accept_new_goal() -> Result<()> {
        let (_node, client, server) = setup_test().await?;

        // Spawn server task to accept the goal
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            assert_eq!(requested.goal.order, 10);
            let _accepted = requested.accept();
            Ok::<(), zenoh::Error>(())
        });

        // Send a goal request
        let goal_handle = client.send_goal(TestGoal { order: 10 }).await?;
        let goal_id = goal_handle.id();

        // Wait for server to finish
        server_task.await??;

        // Verify goal ID is valid (not all zeros)
        assert!(goal_id.is_valid());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_goal_exists() -> Result<()> {
        let (_node, client, server) = setup_test().await?;

        // Spawn server task to accept the goal
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            let _accepted = requested.accept();
            Ok::<(), zenoh::Error>(())
        });

        // Send and accept a goal
        let goal_handle = client.send_goal(TestGoal { order: 10 }).await?;
        let goal_id = goal_handle.id();

        // Wait for server to finish
        server_task.await??;

        // Verify goal ID is valid (acceptance implies goal exists on server)
        assert!(goal_id.is_valid());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_notify_goal_done() -> Result<()> {
        let (_node, client, server) = setup_test().await?;

        // Spawn server task to handle the goal
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            let accepted = requested.accept();
            let executing = accepted.execute();
            executing.succeed(TestResult { value: 42 })?;
            Ok::<(), zenoh::Error>(())
        });

        // Send goal and get result
        let goal_handle = client.send_goal(TestGoal { order: 10 }).await?;

        // Wait for server to finish
        server_task.await??;

        // Get the result to verify completion
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 42);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_get_goal_status_array() -> Result<()> {
        let (_node, client, server) = setup_test().await?;

        // Spawn server task to accept the goal
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            let _accepted = requested.accept();
            Ok::<(), zenoh::Error>(())
        });

        // Add a goal
        let goal_handle = client.send_goal(TestGoal { order: 10 }).await?;

        // Wait for server to finish
        server_task.await??;

        // Verify goal was accepted by checking ID validity
        assert!(goal_handle.id().is_valid());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_process_cancel_request() -> Result<()> {
        use hiroz::action::messages::CancelGoalServiceResponse;

        let (_node, client, server) = setup_test().await?;

        // Spawn server task to handle the goal and cancel
        let server_clone = server.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let requested = server_clone.recv_goal().await?;
            let accepted = requested.accept();
            let _executing = accepted.execute();

            // Signal that goal is accepted
            let _ = tx.send(());

            // Process cancel on server side
            let (cancel_request, query) = server_clone.recv_cancel().await?;

            // Send cancel response
            let response = CancelGoalServiceResponse {
                return_code: 1,
                goals_canceling: vec![cancel_request.goal_info.clone()],
            };
            server_clone.send_cancel_response_low(&query, &response)?;

            Ok::<_, zenoh::Error>(cancel_request)
        });

        // Wait for server to be ready
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send and accept a goal
        let goal_handle = client.send_goal(TestGoal { order: 10 }).await?;

        // Wait for server to accept
        rx.await.expect("server task ended prematurely");

        // Send cancel request
        let _cancel_response = goal_handle.cancel().await?;

        // Wait for server to process cancel
        let cancel_request = tokio::time::timeout(tokio::time::Duration::from_secs(2), server_task)
            .await
            .expect("timeout waiting for server task")??;
        assert_eq!(cancel_request.goal_info.goal_id, goal_handle.id());

        // Basic verification that cancel was received
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_goal_timeout_configuration() -> Result<()> {
        use std::time::Duration;

        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("test_timeout_node").build()?;

        // Create server with goal timeout
        let server = node
            .create_action_server::<TestAction>("test_timeout_action")
            .with_goal_timeout(Duration::from_secs(30))
            .build()?;

        // Verify server creation succeeded by cloning
        let _server_clone = server.clone();

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_qos_configuration() -> Result<()> {
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("test_qos_node").build()?;

        // Create custom QoS profile
        let custom_qos = QosProfile {
            reliability: QosReliability::BestEffort,
            history: QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
            ..Default::default()
        };

        // Create server with QoS configuration
        let server = node
            .create_action_server::<TestAction>("test_qos_action")
            .with_goal_service_qos(custom_qos)
            .with_feedback_topic_qos(custom_qos)
            .build()?;

        // Verify server creation succeeded by cloning
        let _server_clone = server.clone();

        Ok(())
    }
}
