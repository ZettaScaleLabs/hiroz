use std::num::NonZeroUsize;
use std::sync::Arc;

use hiroz::{Builder, Result, context::ZContextBuilder, define_action};
use serde::{Deserialize, Serialize};
use serial_test::serial;

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
async fn setup_test_base() -> Result<(hiroz::node::ZNode,)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_action_client_node").build()?;

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node,))
}

// Helper function to create test setup with client
async fn setup_test_with_client() -> Result<(
    hiroz::node::ZNode,
    std::sync::Arc<hiroz::action::client::ZActionClient<TestAction>>,
)> {
    let (node,) = setup_test_base().await?;

    let client = Arc::new(
        node.create_action_client::<TestAction>("/test_action_client_name")
            .build()?,
    );

    Ok((node, client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_init_fini() -> Result<()> {
        let (node,) = setup_test_base().await?;

        // Test successful initialization with valid arguments
        let client = node
            .create_action_client::<TestAction>("/test_action_client_name")
            .build()?;

        // Verify the client was created successfully by checking it can be cloned
        let _client_clone = client.clone();

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_is_valid() -> Result<()> {
        let (_node, client) = setup_test_with_client().await?;

        // Test valid client - verify it can be cloned (proves internal structures are valid)
        let _client_clone = client.clone();

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_server_is_available() -> Result<()> {
        let (node, _client) = setup_test_with_client().await?;

        // Create a server to verify availability detection
        let _server = node
            .create_action_server::<TestAction>("/test_action_client_name")
            .build()?;

        // Wait for discovery
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // Verify server is discoverable through graph
        let server_names_types = node
            .graph()
            .get_action_server_names_and_types_by_node(hiroz::entity::node_key(node.node_entity()));
        assert!(!server_names_types.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_get_action_name() -> Result<()> {
        let (node, _client) = setup_test_with_client().await?;

        // Verify action name through graph introspection
        let client_names_types = node
            .graph()
            .get_action_client_names_and_types_by_node(hiroz::entity::node_key(node.node_entity()));

        // Should find the action client with the expected name
        let action_found = client_names_types
            .iter()
            .any(|(name, _)| name.contains("test_action_client_name"));
        assert!(action_found);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_get_options() -> Result<()> {
        use hiroz::qos::{QosHistory, QosProfile, QosReliability};

        let (node,) = setup_test_base().await?;

        // Create client with custom QoS options
        let custom_qos = QosProfile {
            reliability: QosReliability::BestEffort,
            history: QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
            ..Default::default()
        };

        let _client = node
            .create_action_client::<TestAction>("/test_action_options")
            .with_goal_service_qos(custom_qos)
            .with_result_service_qos(custom_qos)
            .build()?;

        // Verify client creation with custom options succeeded
        Ok(())
    }

    #[serial]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_wait_for_server() -> Result<()> {
        let ctx = ZContextBuilder::default().build()?;
        let client_node = ctx.create_node("action_wait_client").build()?;
        let client = client_node
            .create_action_client::<TestAction>("/wait_for_action")
            .build()?;

        let server_ctx = ctx.clone();
        let server_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let server_node = server_ctx.create_node("action_wait_server").build()?;
            let _server = server_node
                .create_action_server::<TestAction>("/wait_for_action")
                .build()?;

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Result::<()>::Ok(())
        });

        assert!(
            client
                .wait_for_server(std::time::Duration::from_secs(3))
                .await
        );
        server_task.await??;
        Ok(())
    }

    #[serial]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_action_client_wait_for_server_no_server() -> Result<()> {
        // No server is ever started — wait_for_server must return false within the timeout.
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("wait_no_server_client").build()?;
        let client = node
            .create_action_client::<TestAction>("/nonexistent_action")
            .build()?;

        let ready = client
            .wait_for_server(std::time::Duration::from_millis(300))
            .await;
        assert!(
            !ready,
            "wait_for_server must return false when no server is present"
        );
        Ok(())
    }

    #[serial]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_has_action_server_direct() -> Result<()> {
        // Verify has_action_server via the graph API directly, without going through
        // wait_for_server. Local action servers are indexed synchronously in add_local_entity,
        // so no discovery delay is needed.
        let ctx = ZContextBuilder::default().build()?;
        let node = ctx.create_node("has_server_direct_node").build()?;

        assert!(
            !node.graph().has_action_server("/direct_test_action"),
            "has_action_server must be false before any server is created"
        );

        let _server = node
            .create_action_server::<TestAction>("/direct_test_action")
            .build()?;

        assert!(
            node.graph().has_action_server("/direct_test_action"),
            "has_action_server must be true immediately after build() — \
             all 5 sub-endpoints (send_goal, get_result, cancel_goal, feedback, status) \
             are indexed synchronously via add_local_entity"
        );
        Ok(())
    }
}
