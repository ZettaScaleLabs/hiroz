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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    use hiroz::action::{
        GoalId, GoalInfo, GoalStatus, Time,
        messages::{CancelService, GoalService, SendGoalRequest},
    };
    use hiroz::time::{ZClock, ZTime};
    use std::time::Duration;
    use tokio::time::timeout;

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_goal_uses_the_configured_node_clock() -> Result<()> {
        let clock = ZClock::simulated(ZTime::zero());
        clock.set_time(ZTime::from_unix_nanos(42_000_000_123))?;
        let ctx = ZContextBuilder::default().with_clock(clock).build()?;
        let node = ctx.create_node("sim_clock_action_server").build()?;
        let client = node
            .create_action_client::<TestAction>("sim_clock_action")
            .build()?;
        let server = node
            .create_action_server::<TestAction>("sim_clock_action")
            .build()?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let server_task = tokio::spawn(async move {
            let accepted = server.recv_goal().await.unwrap().accept();
            assert_eq!(
                accepted.info().stamp,
                Time {
                    sec: 42,
                    nanosec: 123
                }
            );
        });
        let _goal = timeout(
            Duration::from_secs(2),
            client.send_goal(TestGoal { order: 1 }),
        )
        .await
        .expect("send goal timed out")?;
        server_task.await.unwrap();
        Ok(())
    }

    async fn send_cancel_request(
        node: &hiroz::node::ZNode,
        goal_info: GoalInfo,
    ) -> Result<hiroz::action::messages::CancelGoalServiceResponse> {
        let client = node
            .create_client::<CancelService<TestAction>>(
                "test_action_server_name/_action/cancel_goal",
            )
            .build()?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        client
            .call(&hiroz::action::messages::CancelGoalServiceRequest { goal_info })
            .await
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_result_replies_with_unknown_status() -> Result<()> {
        let (_node, client, server) = setup_test().await?;
        let _server = server.with_handler(|goal| async move {
            goal.succeed(TestResult::default()).unwrap();
        });

        let response = timeout(
            Duration::from_secs(2),
            client.get_result_with_status(GoalId::new()),
        )
        .await
        .expect("unknown result request timed out")?;
        assert_eq!(response.status, GoalStatus::Unknown as i8);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pending_result_does_not_block_cancel_or_another_goal() -> Result<()> {
        let (_node, client, server) = setup_test().await?;
        let _server = server.with_handler(|goal| async move {
            for _ in 0..100 {
                if goal.is_cancel_requested() {
                    goal.canceled(TestResult::default()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            goal.succeed(TestResult::default()).unwrap();
        });

        let first = client.send_goal(TestGoal { order: 1 }).await?;
        let first_id = first.id();
        let result_task = tokio::spawn(async move { first.result_with_status().await });
        let second = timeout(
            Duration::from_secs(2),
            client.send_goal(TestGoal { order: 2 }),
        )
        .await
        .expect("second goal was blocked")?;
        let cancel = timeout(Duration::from_secs(2), client.cancel_goal(first_id))
            .await
            .expect("cancel was blocked")?;
        assert_eq!(cancel.return_code, 0);
        assert_eq!(cancel.goals_canceling.len(), 1);
        assert_eq!(cancel.goals_canceling[0].goal_id, first_id);

        let result = timeout(Duration::from_secs(2), result_task)
            .await
            .expect("pending result did not finish")
            .expect("result task panicked")?;
        assert_eq!(result.status, GoalStatus::Canceled as i8);
        second.cancel().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_codes_distinguish_unknown_and_terminal_goals() -> Result<()> {
        let (node, client, server) = setup_test().await?;
        let server = server.with_handler(|goal| async move {
            goal.succeed(TestResult::default()).unwrap();
        });

        let response = send_cancel_request(
            &node,
            GoalInfo {
                goal_id: GoalId::new(),
                stamp: Time::zero(),
            },
        )
        .await?;
        assert_eq!(response.return_code, 2);

        let response = send_cancel_request(
            &node,
            GoalInfo {
                goal_id: GoalId::from_bytes([0; 16]),
                stamp: Time::zero(),
            },
        )
        .await?;
        assert_eq!(response.return_code, 0);
        assert!(response.goals_canceling.is_empty());

        let goal = client.send_goal(TestGoal { order: 3 }).await?;
        let goal_id = goal.id();
        timeout(Duration::from_secs(2), goal.result_with_status())
            .await
            .expect("goal did not terminate")?;
        let response = send_cancel_request(
            &node,
            GoalInfo {
                goal_id,
                stamp: Time::zero(),
            },
        )
        .await?;
        assert_eq!(response.return_code, 3);
        drop(server);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_timestamp_selects_only_older_active_goals() -> Result<()> {
        let (node, client, server) = setup_test().await?;
        let server = server.with_handler(|goal| async move {
            for _ in 0..200 {
                if goal.is_cancel_requested() {
                    goal.canceled(TestResult::default()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            goal.abort(TestResult::default()).unwrap();
        });

        let first = client.send_goal(TestGoal { order: 1 }).await?;
        let cutoff = Time::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let second = client.send_goal(TestGoal { order: 2 }).await?;
        let response = send_cancel_request(
            &node,
            GoalInfo {
                goal_id: GoalId::from_bytes([0; 16]),
                stamp: cutoff,
            },
        )
        .await?;
        assert_eq!(response.return_code, 0);
        assert_eq!(response.goals_canceling.len(), 1);
        assert_eq!(response.goals_canceling[0].goal_id, first.id());
        assert!(response.goals_canceling[0].stamp <= cutoff);
        second.cancel().await?;
        drop(server);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn expired_accepted_goal_cannot_be_resurrected() -> Result<()> {
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("expired_goal_node").build()?;
        let client = node.create_action_client::<TestAction>("expired").build()?;
        let server = node
            .create_action_server::<TestAction>("expired")
            .with_goal_timeout(Duration::ZERO)
            .build()?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let accepted = server_clone.recv_goal().await?.accept();
            assert_eq!(server_clone.expire_goals().len(), 1);
            let goal_id = accepted.info().goal_id;
            let executing = accepted.execute();
            assert!(executing.succeed(TestResult::default()).is_err());
            assert!(
                !server_clone
                    .goal_manager()
                    .read(|manager| manager.goals.contains_key(&goal_id))
            );
            Ok::<_, zenoh::Error>(())
        });
        client.send_goal(TestGoal { order: 1 }).await?;
        server_task.await??;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_goal_can_be_canceled_before_execute() -> Result<()> {
        let (_node, client, server) = setup_test().await?;
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let accepted = server_clone.recv_goal().await?.accept();
            timeout(Duration::from_secs(2), async {
                while !server_clone.is_cancel_request_ready() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancel request did not arrive");
            assert!(accepted.try_process_cancel());
            let executing = accepted.execute();
            assert!(executing.is_cancel_requested());
            executing.canceled(TestResult::default())
        });

        let goal = client.send_goal(TestGoal { order: 1 }).await?;
        let response = goal.cancel().await?;
        assert_eq!(response.return_code, 0);
        server_task.await??;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_duplicate_request_does_not_abort_active_goal() -> Result<()> {
        let (node, client, server) = setup_test().await?;
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let executing = server_clone.recv_goal().await?.accept().execute();
            let duplicate = server_clone.recv_goal().await?;
            drop(duplicate);
            assert!(server_clone.goal_manager().read(|manager| {
                matches!(
                    manager.goals.get(&executing.info().goal_id),
                    Some(hiroz::action::state::ServerGoalState::Executing { .. })
                )
            }));
            executing.succeed(TestResult::default())
        });

        let goal = client.send_goal(TestGoal { order: 1 }).await?;
        let raw_client = node
            .create_client::<GoalService<TestAction>>("test_action_server_name/_action/send_goal")
            .build()?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let response = raw_client
            .call(&SendGoalRequest::<TestAction> {
                goal_id: goal.id(),
                goal: TestGoal { order: 2 },
            })
            .await?;
        assert!(!response.accepted);
        server_task.await??;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_state_retains_acceptance_stamp() -> Result<()> {
        let (_node, client, server) = setup_test().await?;
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let accepted = server_clone.recv_goal().await?.accept();
            let goal_id = accepted.info().goal_id;
            let accepted_at = accepted.info().stamp;
            accepted.execute().succeed(TestResult::default())?;
            server_clone.goal_manager().read(|manager| {
                let Some(hiroz::action::state::ServerGoalState::Terminated {
                    accepted_at: terminal_stamp,
                    ..
                }) = manager.goals.get(&goal_id)
                else {
                    panic!("goal was not terminal");
                };
                assert_eq!(*terminal_stamp, accepted_at);
            });
            Ok::<_, zenoh::Error>(())
        });
        client.send_goal(TestGoal { order: 1 }).await?;
        server_task.await??;
        Ok(())
    }
}
