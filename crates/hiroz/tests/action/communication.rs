//! Action communication tests
//!
//! These tests verify the low-level protocol communication between action clients and servers.

use std::time::Duration;

use hiroz::{Builder, Result, context::ZContextBuilder, define_action, msg::ZSerializer};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use zenoh::Wait;

// Test action messages (equivalent to test_msgs/action/Fibonacci)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestGoal {
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    pub value: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestFeedback {
    pub sequence: Vec<i32>,
}

pub struct TestAction;

define_action! {
    TestAction,
    action_name: "test_action_comm",
    Goal: TestGoal,
    Result: TestResult,
    Feedback: TestFeedback,
}

/// Helper to setup test fixtures
async fn setup_test() -> Result<(
    hiroz::context::ZContext,
    hiroz::node::ZNode,
    hiroz::action::client::ZActionClient<TestAction>,
    hiroz::action::server::ZActionServer<TestAction>,
)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_action_comm_node").build()?;

    let server = node
        .create_action_server::<TestAction>("test_action_comm")
        .build()?;

    let client = node
        .create_action_client::<TestAction>("test_action_comm")
        .build()?;

    // Longer delay to allow Zenoh discovery
    // Server needs to be fully initialized before client can connect
    tokio::time::sleep(Duration::from_millis(500)).await;

    Ok((ctx, node, client, server))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests basic goal request/response communication
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_valid_goal_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn server processing task
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            // Server should receive the goal request
            let requested = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal")?;

            // Extract values before accepting (which moves requested)
            let goal_order = requested.goal.order;
            let goal_id = requested.info.goal_id;

            // Accept the goal (sends goal response)
            let _accepted = requested.accept();

            Ok::<_, zenoh::Error>((goal_order, goal_id))
        });

        // Create and send goal request
        let outgoing_goal = TestGoal { order: 10 };
        let goal_handle = timeout(
            Duration::from_secs(5),
            client.send_goal(outgoing_goal.clone()),
        )
        .await
        .expect("timeout sending goal")?;

        // Wait for server to process
        let (goal_order, goal_id) = server_task.await.expect("server task failed")?;

        // Verify goal data matches
        assert_eq!(goal_order, outgoing_goal.order);
        assert_eq!(goal_id, goal_handle.id());

        // Client should receive the acceptance response
        // This happens automatically in send_goal, but we can verify the goal is valid
        assert_ne!(goal_handle.id(), hiroz::action::GoalId::default());

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Tests cancel request/response communication
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_valid_cancel_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn both goal and cancel handling together
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal")?;
            let goal_id = requested.info.goal_id;
            let accepted = requested.accept();
            let _executing = accepted.execute();

            // Server should receive cancel request
            let (cancel_request, response_tx) =
                timeout(Duration::from_secs(5), server_clone.recv_cancel())
                    .await
                    .expect("timeout receiving cancel")?;

            // Verify cancel request has correct goal ID
            assert_eq!(cancel_request.goal_info.goal_id, goal_id);

            // Send cancel response
            let cancel_resp = hiroz::action::messages::CancelGoalResponse {
                return_code: 0, // ERROR_NONE
                goals_canceling: vec![hiroz::action::GoalInfo {
                    goal_id,
                    stamp: hiroz::action::Time::zero(), // Current time in nanoseconds
                }],
            };

            // Respond to the cancel request
            let response_bytes = hiroz::msg::SerdeCdrSerdes::<
                hiroz::action::messages::CancelGoalResponse,
            >::serialize(&cancel_resp);
            response_tx
                .reply(response_tx.key_expr().clone(), response_bytes)
                .wait()?;

            Ok::<_, zenoh::Error>(())
        });

        // Send and accept a goal first
        let goal = TestGoal { order: 10 };
        let goal_handle = timeout(Duration::from_secs(5), client.send_goal(goal))
            .await
            .expect("timeout sending goal")?;

        // Send cancel request
        let cancel_response = timeout(Duration::from_secs(5), goal_handle.cancel())
            .await
            .expect("timeout sending cancel")?;

        // Wait for server to process everything
        server_task.await.expect("server task failed")?;

        // Verify client received response
        assert_eq!(cancel_response.return_code, 0); // ERROR_NONE

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Tests result request/response communication
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_valid_result_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn server processing
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal")?;
            let accepted = requested.accept();
            let executing = accepted.execute();

            // Complete the goal with result
            let outgoing_result = TestResult { value: 42 };
            executing.succeed(outgoing_result.clone())?;

            Ok::<_, zenoh::Error>(outgoing_result)
        });

        // Send goal
        let goal = TestGoal { order: 10 };
        let goal_handle = timeout(Duration::from_secs(5), client.send_goal(goal))
            .await
            .expect("timeout sending goal")?;

        let outgoing_result = server_task.await.expect("server task failed")?;

        // Client requests result
        let incoming_result = timeout(Duration::from_secs(5), goal_handle.result())
            .await
            .expect("timeout getting result")?;

        // Verify result data matches
        assert_eq!(incoming_result.value, outgoing_result.value);

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Tests feedback publishing/subscription
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_valid_feedback_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn server processing first
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let requested = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal")?;
            let accepted = requested.accept();
            let executing = accepted.execute();

            // Publish feedback
            let outgoing_feedback = TestFeedback {
                sequence: vec![0, 1, 1, 2, 3, 5, 8, 13],
            };
            executing.publish_feedback(outgoing_feedback.clone())?;

            // Wait a bit for client to receive feedback
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Complete goal
            executing.succeed(TestResult { value: 13 })?;

            Ok::<_, zenoh::Error>(outgoing_feedback)
        });

        // Send goal
        let goal = TestGoal { order: 10 };
        let mut goal_handle = timeout(Duration::from_secs(5), client.send_goal(goal))
            .await
            .expect("timeout sending goal")?;

        // Get feedback stream after goal is sent
        let mut feedback_rx = goal_handle
            .feedback()
            .expect("failed to get feedback stream");

        // Client receives feedback
        let incoming_feedback = timeout(Duration::from_secs(5), feedback_rx.recv())
            .await
            .expect("timeout receiving feedback")
            .expect("feedback channel closed");

        let outgoing_feedback = server_task.await.expect("server task failed")?;

        // Verify feedback data matches
        assert_eq!(incoming_feedback.sequence, outgoing_feedback.sequence);

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Tests status publishing/subscription
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_valid_status_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn server processing task
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            // Accept and execute goal on server
            let requested = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal")?;
            let accepted = requested.accept();

            // Give client time to observe Accepted status
            tokio::time::sleep(Duration::from_millis(300)).await;

            let executing = accepted.execute();

            // Wait for client to observe EXECUTING status
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Complete the goal
            executing.succeed(TestResult { value: 42 })?;

            Ok::<_, zenoh::Error>(())
        });

        // Send goal
        let goal = TestGoal { order: 10 };
        let goal_handle = timeout(Duration::from_secs(5), client.send_goal(goal))
            .await
            .expect("timeout sending goal")?;

        // Watch status for this goal
        let mut status_watch = client
            .status_watch(goal_handle.id())
            .expect("failed to watch status");

        // Wait for status updates - should eventually see Succeeded
        // Status transitions: Accepted -> Executing -> Succeeded
        let mut final_status = *status_watch.borrow();
        let mut iterations = 0;
        while final_status != hiroz::action::GoalStatus::Succeeded && iterations < 10 {
            match timeout(Duration::from_millis(600), status_watch.changed()).await {
                Ok(Ok(_)) => {
                    final_status = *status_watch.borrow();
                }
                Ok(Err(_)) => break, // Watch closed
                Err(_) => {
                    // Timeout - break to check status
                    final_status = *status_watch.borrow();
                    break;
                }
            }
            iterations += 1;
        }

        // Verify we reached Succeeded status
        assert_eq!(
            final_status,
            hiroz::action::GoalStatus::Succeeded,
            "Expected Succeeded status after {} iterations",
            iterations
        );

        // Wait for server task to complete
        server_task.await.expect("server task failed")?;

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Regression test: cancel for goal B must not be silently dropped when goal A polls first.
    ///
    /// With the old `try_process_cancel` implementation, if goal A polled the shared cancel
    /// queue and found a request for goal B, it would discard the message (ID mismatch) and
    /// return false. Goal B's subsequent poll would find an empty queue and also return false —
    /// the cancel was lost.
    ///
    /// The fix introduces `CancelDispatcher`: both handles call `drain()` which routes every
    /// pending message to the correct per-goal channel, so each handle only sees its own cancels.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_try_process_cancel_multi_goal() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Send two goals and get handles to both
        let goal1 = TestGoal { order: 1 };
        let goal2 = TestGoal { order: 2 };

        // Server must accept each goal immediately so the client can proceed to send the next one.
        // (client.send_goal blocks until the server sends an accept response)
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let req1 = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal 1")?;
            // Accept immediately so the client unblocks and sends goal 2
            let handle1 = req1.accept().execute();

            let req2 = timeout(Duration::from_secs(5), server_clone.recv_goal())
                .await
                .expect("timeout receiving goal 2")?;
            let handle2 = req2.accept().execute();

            Ok::<_, zenoh::Error>((handle1, handle2))
        });

        let goal_handle1 = timeout(Duration::from_secs(5), client.send_goal(goal1))
            .await
            .expect("timeout sending goal 1")?;
        let goal_handle2 = timeout(Duration::from_secs(5), client.send_goal(goal2))
            .await
            .expect("timeout sending goal 2")?;

        let (handle1, handle2) = server_task.await.expect("server task failed")?;

        // Spawn a task that sends the cancel for goal2 and then awaits the result.
        // We move goal_handle2 into this task so that cancel() and result() can be called
        // in sequence without a borrow/move conflict in the outer scope.
        let client_task = tokio::spawn(async move {
            let cancel_response = timeout(Duration::from_secs(5), goal_handle2.cancel())
                .await
                .expect("timeout awaiting cancel response")?;
            // After cancel is confirmed, fetch the final result
            let result = timeout(Duration::from_secs(5), goal_handle2.result())
                .await
                .expect("timeout getting result 2")?;
            Ok::<_, zenoh::Error>((cancel_response, result))
        });

        // Give the cancel request time to arrive on the server side
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Goal 1 polls first — must NOT steal the cancel intended for goal 2
        assert!(
            !handle1.try_process_cancel(),
            "handle1.try_process_cancel() should return false (cancel was for goal2)"
        );

        // Goal 2 polls second — must see the cancel routed to its channel
        assert!(
            handle2.try_process_cancel(),
            "handle2.try_process_cancel() should return true (cancel was for goal2)"
        );

        // Complete both goals so the client task can finish
        handle1.succeed(TestResult { value: 1 })?;
        handle2.canceled(TestResult { value: 2 })?;

        let (cancel_response, _) = client_task.await.expect("client task panicked")?;
        assert_eq!(cancel_response.return_code, 1);

        let _ = timeout(Duration::from_secs(5), goal_handle1.result())
            .await
            .expect("timeout getting result 1")?;

        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Tests handling multiple concurrent goals
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_multiple_goals_comm() -> Result<()> {
        let (_ctx, _node, client, server) = setup_test().await?;

        // Spawn server processing
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            // Server processes all goals
            for i in 0..3 {
                let requested = timeout(Duration::from_secs(2), server_clone.recv_goal())
                    .await
                    .expect("timeout receiving goal")?;
                assert_eq!(requested.goal.order, i * 10);

                let accepted = requested.accept();
                let executing = accepted.execute();
                executing.succeed(TestResult { value: i * 100 })?;
            }

            Ok::<_, zenoh::Error>(())
        });

        // Send multiple goals
        let mut goal_handles = vec![];
        for i in 0..3 {
            let goal = TestGoal { order: i * 10 };
            let handle = timeout(Duration::from_secs(2), client.send_goal(goal))
                .await
                .expect("timeout sending goal")?;
            goal_handles.push(handle);
        }

        // Wait for server to process all goals
        server_task.await.expect("server task failed")?;

        // Verify all results
        for (i, handle) in goal_handles.into_iter().enumerate() {
            let result = timeout(Duration::from_secs(2), handle.result())
                .await
                .expect("timeout getting result")?;
            assert_eq!(result.value, i as i32 * 100);
        }

        // Clean shutdown
        drop(server);
        drop(client);
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    }
}
