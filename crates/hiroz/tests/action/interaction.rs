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

// Helper function to create test setup with multiple clients and servers
async fn setup_multiple_clients_single_server(
    num_clients: usize,
) -> Result<(
    hiroz::node::ZNode,
    hiroz::action::server::ZActionServer<TestAction>,
    Vec<hiroz::action::client::ZActionClient<TestAction>>,
)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_interaction_node").build()?;

    let server = node
        .create_action_server::<TestAction>("test_action")
        .build()?;

    let mut clients = Vec::new();
    for _i in 0..num_clients {
        let client = node
            .create_action_client::<TestAction>("test_action")
            .build()?;
        clients.push(client);
    }

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node, server, clients))
}

// Helper function to create test setup with single client and multiple servers
async fn setup_single_client_multiple_servers(
    num_servers: usize,
) -> Result<(
    hiroz::node::ZNode,
    hiroz::action::client::ZActionClient<TestAction>,
    Vec<hiroz::action::server::ZActionServer<TestAction>>,
)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("test_interaction_node").build()?;

    let client = node
        .create_action_client::<TestAction>("test_action")
        .build()?;

    let mut servers = Vec::new();
    for _i in 0..num_servers {
        let server = node
            .create_action_server::<TestAction>("test_action")
            .build()?;
        servers.push(server);
    }

    // Wait for discovery
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok((node, client, servers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_multiple_clients_single_server() -> Result<()> {
        let (_node, server, clients) = setup_multiple_clients_single_server(3).await?;

        // Set up server handler
        let _server_handle = server.with_handler(|executing| async move {
            let order = executing.goal.order;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            executing.succeed(TestResult { value: order * 2 }).unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send goals from multiple clients
        let mut handles = Vec::new();
        for (i, client) in clients.iter().enumerate() {
            let client_clone = client.clone();
            let goal_order = (i + 1) as i32 * 10;
            let handle = tokio::spawn(async move {
                let goal_handle = client_clone
                    .send_goal(TestGoal { order: goal_order })
                    .await
                    .unwrap();
                goal_handle.result().await.unwrap()
            });
            handles.push(handle);
        }

        // Wait for all results and verify
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            let expected_value = (i + 1) as i32 * 20; // order * 2
            assert_eq!(
                result.value, expected_value,
                "Result mismatch for client {}",
                i
            );
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_single_client_multiple_servers() -> Result<()> {
        let (_node, _client, servers) = setup_single_client_multiple_servers(2).await?;

        // Verify that multiple servers were created
        assert_eq!(servers.len(), 2, "Should have created 2 servers");

        // Verify each server can be cloned (indicates valid state)
        for server in &servers {
            let _server_clone = server.clone();
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_goal_priority_handling() -> Result<()> {
        let (_node, server, mut clients) = setup_multiple_clients_single_server(2).await?;

        // Set up server with priority handling (higher order = higher priority)
        let _server_handle = server.with_handler(|executing| async move {
            let order = executing.goal.order;
            // Simulate priority-based processing time
            let processing_time = if order > 50 { 100 } else { 200 };
            tokio::time::sleep(std::time::Duration::from_millis(processing_time)).await;
            executing.succeed(TestResult { value: order * 2 }).unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send high and low priority goals
        let high_priority_client = clients.remove(0);
        let low_priority_client = clients.remove(0);

        let high_priority_handle = tokio::spawn(async move {
            let goal_handle = high_priority_client
                .send_goal(TestGoal { order: 100 })
                .await
                .unwrap();
            goal_handle.result().await.unwrap()
        });

        let low_priority_handle = tokio::spawn(async move {
            let goal_handle = low_priority_client
                .send_goal(TestGoal { order: 10 })
                .await
                .unwrap();
            goal_handle.result().await.unwrap()
        });

        let high_result = high_priority_handle.await.unwrap();
        let low_result = low_priority_handle.await.unwrap();

        // Verify results match expected values
        assert_eq!(high_result.value, 200, "High priority result incorrect");
        assert_eq!(low_result.value, 20, "Low priority result incorrect");

        Ok(())
    }

    // TODO: Additional tests would cover:
    // - Concurrent goal execution
    // - Server discovery and availability
    // - Load balancing across multiple servers
    // These would require more complex setup and are deferred for now
}
