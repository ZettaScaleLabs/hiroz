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
    let node = ctx.create_node("test_action_graph_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node,))
}

// Helper function to create test setup with client and server
async fn setup_test_with_client_server() -> Result<(
    hiroz::context::ZContext,
    hiroz::node::ZNode,
    hiroz::node::ZNode,
    std::sync::Arc<hiroz::action::client::ZActionClient<TestAction>>,
    hiroz::action::server::ZActionServer<TestAction>,
)> {
    let ctx = ZContextBuilder::default().build()?;

    let client_node = ctx.create_node("test_action_graph_client_node").build()?;
    let server_node = ctx.create_node("test_action_graph_server_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = Arc::new(
        client_node
            .create_action_client::<TestAction>("/test_action_graph_name")
            .build()?,
    );

    let server = server_node
        .create_action_server::<TestAction>("/test_action_graph_name")
        .build()?;

    // Wait for graph discovery of action topics
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    Ok((ctx, client_node, server_node, client, server))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_graph_node_discovery() -> Result<()> {
        let (_ctx, client_node, server_node, _client, _server) =
            setup_test_with_client_server().await?;

        // Verify nodes were created with correct names
        assert_eq!(client_node.name(), "test_action_graph_client_node");
        assert_eq!(server_node.name(), "test_action_graph_server_node");

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_server_discovery() -> Result<()> {
        let (_ctx, _client_node, _server_node, client, server) =
            setup_test_with_client_server().await?;

        // Test that action clients and servers can discover each other
        // This would involve checking the graph for action-related entities

        // For now, test that client and server can communicate
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Ok(requested) = server_clone.recv_goal().await {
                let accepted = requested.accept();
                let executing = accepted.execute();
                let _ = executing.succeed(TestResult { value: 42 });
            }
        });

        let goal = TestGoal { order: 5 };
        let goal_handle = client.send_goal(goal).await?;
        let result = goal_handle.result().await?;

        assert_eq!(result.value, 42);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_basic_graph_discovery() -> Result<()> {
        // Use a standard message type
        use hiroz_msgs::std_msgs::String as StringMsg;

        let ctx = ZContextBuilder::default().build()?;
        let node1 = ctx.create_node("node1").build()?;
        let node2 = ctx.create_node("node2").build()?;

        // Create a publisher on node1
        let _pub = node1.create_pub::<StringMsg>("/test_topic").build()?;

        // Wait for discovery
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        // Check if node2's graph can see it
        let topics = node2.graph().get_topic_names_and_types();
        eprintln!("Node2 discovered {} topics:", topics.len());
        for (name, typ) in &topics {
            eprintln!("  - {} ({})", name, typ);
        }

        assert!(
            !topics.is_empty(),
            "Graph discovery not working for regular topics either!"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_graph_introspection_by_node() -> Result<()> {
        let (_ctx, _client_node, _server_node, _client, _server) =
            setup_test_with_client_server().await?;

        // Test getting action clients by node
        let client_node_key = hiroz::entity::node_key(_client_node.node_entity());
        let client_names_types = _client_node
            .graph()
            .get_action_client_names_and_types_by_node(client_node_key);

        // Should find the action client we created
        assert!(!client_names_types.is_empty());
        // The action name should be in the list
        let action_found = client_names_types
            .iter()
            .any(|(name, _)| name.contains("test_action_graph_name"));
        assert!(action_found);

        // Test getting action servers by node
        let server_node_key = hiroz::entity::node_key(_server_node.node_entity());
        let server_names_types = _server_node
            .graph()
            .get_action_server_names_and_types_by_node(server_node_key);

        // Should find the action server we created
        assert!(!server_names_types.is_empty());
        let action_found = server_names_types
            .iter()
            .any(|(name, _)| name.contains("test_action_graph_name"));
        assert!(action_found);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_graph_introspection_all() -> Result<()> {
        let (_ctx, _client_node, _server_node, _client, _server) =
            setup_test_with_client_server().await?;

        // Test getting all action names and types
        let all_actions = _client_node.graph().get_action_names_and_types();

        // Should find both client and server actions
        assert!(!all_actions.is_empty());
        let action_found = all_actions
            .iter()
            .any(|(name, _)| name.contains("test_action_graph_name"));
        assert!(action_found);

        Ok(())
    }

    // TODO: Additional tests would cover:
    // - Action client discovery by node
    // - Action server discovery by node
    // - Action name and type enumeration
    // - Multi-node action graph scenarios
}
