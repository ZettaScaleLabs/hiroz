//! Tests for action goal expiration functionality.
//!
//! These tests verify that:
//! 1. Terminated goals expire after the result timeout
//! 2. Accepted/Executing goals can expire if goal timeout is configured
//! 3. Expired goals are properly cleaned up
//! 4. Status is updated when goals expire

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use hiroz::action::state::*;
use hiroz::action::*;
use hiroz::context::ZContextBuilder;
use hiroz::{Builder, Result, define_action};
use serde::{Deserialize, Serialize};

// Simple test action type
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestGoal {
    order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestResult {
    sequence: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestFeedback {
    current: i32,
}

struct TestAction;

define_action! {
    TestAction,
    action_name: "test_action/Expiration",
    Goal: TestGoal,
    Result: TestResult,
    Feedback: TestFeedback,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_terminated_goal_expiration() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_expiration_node").build()?;

    // Create a server with short result timeout (1 second)
    let server = node
        .create_action_server::<TestAction>("test_action")
        .with_result_timeout(Duration::from_secs(1))
        .build()?;

    let goal_id = GoalId::new();

    // Simulate accepting and terminating a goal
    server.goal_manager().modify(|manager| {
        let now = Instant::now();
        manager.goals.insert(
            goal_id,
            ServerGoalState::Terminated {
                result: TestResult {
                    sequence: vec![0, 1, 1, 2, 3, 5],
                },
                status: GoalStatus::Succeeded,
                timestamp: now,
                expires_at: Some(now + Duration::from_secs(1)),
            },
        );
    });

    // Verify goal exists
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 1);

    // Wait for expiration time to pass
    sleep(Duration::from_millis(1100)).await;

    // Manually trigger expiration check
    let expired = server.expire_goals();

    // Verify the goal was expired
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], goal_id);

    // Verify goal was removed
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_executing_goal_expiration_with_timeout() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_expiration_node2").build()?;

    // Create a server with short goal timeout (1 second)
    let server = node
        .create_action_server::<TestAction>("test_action2")
        .with_goal_timeout(Duration::from_secs(1))
        .build()?;

    let goal_id = GoalId::new();

    // Simulate an executing goal with expiration
    server.goal_manager().modify(|manager| {
        let now = Instant::now();
        manager.goals.insert(
            goal_id,
            ServerGoalState::Executing {
                goal: TestGoal { order: 5 },
                cancel_flag: Arc::new(AtomicBool::new(false)),
                expires_at: Some(now + Duration::from_secs(1)),
            },
        );
    });

    // Verify goal exists and is executing
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 1);

    // Wait for expiration time to pass
    sleep(Duration::from_millis(1100)).await;

    // Manually trigger expiration check
    let expired = server.expire_goals();

    // Verify the executing goal was expired due to timeout
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], goal_id);

    // Verify goal was removed
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_accepted_goal_expiration_with_timeout() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_expiration_node3").build()?;

    // Create a server with short goal timeout (1 second)
    let server = node
        .create_action_server::<TestAction>("test_action3")
        .with_goal_timeout(Duration::from_secs(1))
        .build()?;

    let goal_id = GoalId::new();

    // Simulate an accepted goal with expiration
    server.goal_manager().modify(|manager| {
        let now = Instant::now();
        manager.goals.insert(
            goal_id,
            ServerGoalState::Accepted {
                goal: TestGoal { order: 5 },
                timestamp: now,
                expires_at: Some(now + Duration::from_secs(1)),
            },
        );
    });

    // Verify goal exists and is accepted
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 1);

    // Wait for expiration time to pass
    sleep(Duration::from_millis(1100)).await;

    // Manually trigger expiration check
    let expired = server.expire_goals();

    // Verify the accepted goal was expired due to timeout
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], goal_id);

    // Verify goal was removed
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_no_expiration_without_timeout() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_expiration_node4").build()?;

    // Create a server WITHOUT goal timeout
    let server = node
        .create_action_server::<TestAction>("test_action4")
        .build()?;

    let goal_id = GoalId::new();

    // Simulate an executing goal WITHOUT expiration (None)
    server.goal_manager().modify(|manager| {
        manager.goals.insert(
            goal_id,
            ServerGoalState::Executing {
                goal: TestGoal { order: 5 },
                cancel_flag: Arc::new(AtomicBool::new(false)),
                expires_at: None, // No expiration
            },
        );
    });

    // Wait a bit
    sleep(Duration::from_millis(1100)).await;

    // Trigger expiration check
    let expired = server.expire_goals();

    // Verify the goal was NOT expired (no timeout configured)
    assert_eq!(expired.len(), 0);

    // Verify goal still exists
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multiple_goals_expiration() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_expiration_node5").build()?;

    // Create a server with short timeouts
    let server = node
        .create_action_server::<TestAction>("test_action5")
        .with_result_timeout(Duration::from_secs(1))
        .with_goal_timeout(Duration::from_secs(1))
        .build()?;

    let goal_id1 = GoalId::new();
    let goal_id2 = GoalId::new();
    let goal_id3 = GoalId::new();

    // Add multiple goals with different states, all expiring
    server.goal_manager().modify(|manager| {
        let now = Instant::now();
        let expires = now + Duration::from_secs(1);

        manager.goals.insert(
            goal_id1,
            ServerGoalState::Terminated {
                result: TestResult {
                    sequence: vec![0, 1],
                },
                status: GoalStatus::Succeeded,
                timestamp: now,
                expires_at: Some(expires),
            },
        );

        manager.goals.insert(
            goal_id2,
            ServerGoalState::Executing {
                goal: TestGoal { order: 3 },
                cancel_flag: Arc::new(AtomicBool::new(false)),
                expires_at: Some(expires),
            },
        );

        manager.goals.insert(
            goal_id3,
            ServerGoalState::Accepted {
                goal: TestGoal { order: 2 },
                timestamp: now,
                expires_at: Some(expires),
            },
        );
    });

    // Verify all goals exist
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 3);

    // Wait for expiration
    sleep(Duration::from_millis(1100)).await;

    // Trigger expiration check
    let expired = server.expire_goals();

    // Verify all goals were expired
    assert_eq!(expired.len(), 3);
    assert!(expired.contains(&goal_id1));
    assert!(expired.contains(&goal_id2));
    assert!(expired.contains(&goal_id3));

    // Verify all goals were removed
    let goal_count = server.goal_manager().read(|manager| manager.goals.len());
    assert_eq!(goal_count, 0);

    Ok(())
}
