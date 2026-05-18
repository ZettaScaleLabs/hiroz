use std::sync::Arc;

use hiroz::{Builder, Result, context::ZContextBuilder, define_action};
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

// Helper function to create test setup
#[allow(dead_code)]
async fn setup_test_base() -> Result<(hiroz::node::ZNode,)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_action_remapping_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node,))
}

// Helper function to create test setup with client and server
async fn setup_test_with_client_server(
    action_name: &str,
) -> Result<(
    hiroz::node::ZNode,
    hiroz::node::ZNode,
    std::sync::Arc<hiroz::action::client::ZActionClient<TestAction>>,
    hiroz::action::server::ZActionServer<TestAction>,
)> {
    let ctx = ZContextBuilder::default().build()?;

    let client_node = ctx
        .create_node("test_action_remapping_client_node")
        .build()?;
    let server_node = ctx
        .create_node("test_action_remapping_server_node")
        .build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = Arc::new(
        client_node
            .create_action_client::<TestAction>(action_name)
            .build()?,
    );

    let server = server_node
        .create_action_server::<TestAction>(action_name)
        .build()?;

    Ok((client_node, server_node, client, server))
}

// Helper function to run server with timeout and cleanup
async fn run_server_with_timeout(
    server: hiroz::action::server::ZActionServer<TestAction>,
    expected_result: i32,
    timeout_ms: u64,
) -> Result<()> {
    let timeout = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        if let Ok(requested) = server.recv_goal().await {
            let accepted = requested.accept();
            let executing = accepted.execute();
            let _ = executing.succeed(TestResult {
                value: expected_result,
            });
        }
    })
    .await;

    match timeout {
        Ok(_) => Ok(()),
        Err(_) => Err("Server timeout".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_name_remapping_absolute() -> Result<()> {
        // Test absolute action names that should not be remapped
        let (_client_node, _server_node, client, server) =
            setup_test_with_client_server("/absolute_action_name").await?;

        // Test that client and server can communicate with absolute names
        let server_clone = server.clone();
        let server_handle =
            tokio::spawn(async move { run_server_with_timeout(server_clone, 100, 5000).await });

        let goal = TestGoal { order: 5 };
        let goal_handle = client.send_goal(goal).await?;
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 100);

        // Wait for server to complete
        let _ = server_handle.await;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_name_remapping_relative() -> Result<()> {
        // Test relative action names that should be resolved with namespace
        let (_client_node, _server_node, client, server) =
            setup_test_with_client_server("relative_action_name").await?;

        // Test that client and server can communicate with relative names
        let server_clone = server.clone();
        let server_handle =
            tokio::spawn(async move { run_server_with_timeout(server_clone, 200, 5000).await });

        let goal = TestGoal { order: 10 };
        let goal_handle = client.send_goal(goal).await?;
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 200);

        // Wait for server to complete
        let _ = server_handle.await;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_name_remapping_with_rules() -> Result<()> {
        // Test action name remapping with remapping rules
        let ctx = ZContextBuilder::default()
            .with_remap_rule("original_action:=remapped_action")?
            .build()?;

        let client_node = ctx.create_node("test_client").build()?;
        let server_node = ctx.create_node("test_server").build()?;

        // Wait for discovery
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Create client with original name - should be remapped to "remapped_action"
        let client = Arc::new(
            client_node
                .create_action_client::<TestAction>("original_action")
                .build()?,
        );

        // Create server with remapped name
        let server = server_node
            .create_action_server::<TestAction>("remapped_action")
            .build()?;

        // Test that client and server can communicate through remapping
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Ok(requested) = server_clone.recv_goal().await {
                let accepted = requested.accept();
                let executing = accepted.execute();
                let _ = executing.succeed(TestResult { value: 300 });
            }
        });

        let goal = TestGoal { order: 15 };
        let goal_handle = client.send_goal(goal).await?;
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 300);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_name_remapping_multiple_rules() -> Result<()> {
        // Test multiple remapping rules
        let ctx = ZContextBuilder::default()
            .with_remap_rule("action1:=remapped_action1")?
            .with_remap_rule("action2:=remapped_action2")?
            .build()?;

        let client_node = ctx.create_node("test_client").build()?;
        let server_node = ctx.create_node("test_server").build()?;

        // Wait for discovery
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Test first remapping
        let client1 = Arc::new(
            client_node
                .create_action_client::<TestAction>("action1")
                .build()?,
        );
        let server1 = server_node
            .create_action_server::<TestAction>("remapped_action1")
            .build()?;

        let server_clone1 = server1.clone();
        let server_handle1 =
            tokio::spawn(async move { run_server_with_timeout(server_clone1, 400, 5000).await });

        let goal = TestGoal { order: 20 };
        let goal_handle = client1.send_goal(goal).await?;
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 400);

        // Wait for first server to complete
        let _ = server_handle1.await;

        // Test second remapping
        let client2 = Arc::new(
            client_node
                .create_action_client::<TestAction>("action2")
                .build()?,
        );
        let server2 = server_node
            .create_action_server::<TestAction>("remapped_action2")
            .build()?;

        let server_clone2 = server2.clone();
        let server_handle2 =
            tokio::spawn(async move { run_server_with_timeout(server_clone2, 500, 5000).await });

        let goal = TestGoal { order: 25 };
        let goal_handle = client2.send_goal(goal).await?;
        let result = goal_handle.result().await?;
        assert_eq!(result.value, 500);

        // Wait for second server to complete
        let _ = server_handle2.await;

        Ok(())
    }

    #[test]
    fn test_remap_rules_apply() -> Result<()> {
        // Test that RemapRules.apply works correctly
        let mut rules = hiroz::context::RemapRules::new();
        rules.add_rule("original:=remapped")?;

        assert_eq!(rules.apply("original"), "remapped");
        assert_eq!(rules.apply("unchanged"), "unchanged");

        Ok(())
    }

    #[test]
    fn test_context_with_remap_rules() -> Result<()> {
        // Test that context builder accepts remap rules
        let ctx = ZContextBuilder::default()
            .with_remap_rule("test_action:=remapped_action")?
            .build()?;

        let node = ctx.create_node("test_node").build()?;

        // Verify that the remap rules are passed to the node
        assert_eq!(node.apply_remap("test_action"), "remapped_action");
        assert_eq!(node.apply_remap("other_action"), "other_action");

        Ok(())
    }

    #[test]
    fn test_remap_rules_complex_scenarios() -> Result<()> {
        let mut rules = hiroz::context::RemapRules::new();

        // Test multiple rules
        rules.add_rule("action1:=remapped_action1")?;
        rules.add_rule("action2:=remapped_action2")?;
        rules.add_rule("__node:=new_node")?;
        rules.add_rule("__ns:=/new_namespace")?;

        // Test action name remapping
        assert_eq!(rules.apply("action1"), "remapped_action1");
        assert_eq!(rules.apply("action2"), "remapped_action2");

        // Test node name remapping (should be handled at context level)
        assert_eq!(rules.apply("__node"), "new_node");
        assert_eq!(rules.apply("__ns"), "/new_namespace");

        // Test that unmapped names pass through
        assert_eq!(rules.apply("unchanged_action"), "unchanged_action");

        Ok(())
    }

    #[test]
    fn test_remap_rules_edge_cases() -> Result<()> {
        let mut rules = hiroz::context::RemapRules::new();

        // Test empty rules
        assert_eq!(rules.apply("any_name"), "any_name");
        assert!(rules.is_empty());

        // Test rule with empty target (should fail)
        assert!(rules.add_rule("source:=").is_err());

        // Test rule with empty source (should fail)
        assert!(rules.add_rule(":=target").is_err());

        // Test invalid format (should fail)
        assert!(rules.add_rule("invalid_format").is_err());

        // Test valid rule
        rules.add_rule("valid:=rule")?;
        assert_eq!(rules.apply("valid"), "rule");
        assert!(!rules.is_empty());

        Ok(())
    }

    #[test]
    fn test_remap_rules_namespace_resolution() -> Result<()> {
        let mut rules = hiroz::context::RemapRules::new();

        // Test namespace remapping
        rules.add_rule("__ns:=/test_namespace")?;
        rules.add_rule("local_action:=global_action")?;

        // These would be applied at different levels:
        // - __ns rules affect node namespace resolution
        // - action name rules affect action topic resolution
        assert_eq!(rules.apply("__ns"), "/test_namespace");
        assert_eq!(rules.apply("local_action"), "global_action");

        Ok(())
    }

    // TODO: Additional tests would cover:
    // - Node-specific remapping rules
    // - Namespace-relative action names
    // - Complex remapping chains
    // - Invalid remapping rules
}
