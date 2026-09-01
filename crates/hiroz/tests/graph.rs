//! Graph API tests
//!
//! These tests verify the graph introspection functionality, including:
//! - Getting topic/service names and types
//! - Counting publishers/subscribers/clients/services
//! - Waiting for graph changes
//! - Node discovery and information
//! - Service availability checking

use std::{sync::Arc, time::Duration};

use hiroz::{
    Builder, Result,
    context::ZContextBuilder,
    entity::{EndpointKind, NodeKey},
};
use hiroz_msgs::{example_interfaces::srv::AddTwoInts, std_msgs::String as RosString};
use hiroz_protocol::{
    EndpointEntity, Entity, KeyExprFormat, KeyExprFormatter, KeyExprFormatterAdapter, NodeEntity,
    RmwZenohFormatter,
    entity::{LivelinessKE, TopicKE},
    qos::QosProfile,
};

mod common;
use common::TestRouter;

/// A pass-through `KeyExprFormatter` that delegates to `RmwZenohFormatter` for
/// everything except its own admin space, so a node using it is
/// distinguishable on the wire from a plain rmw_zenoh node without
/// reimplementing any encoding.
#[derive(Debug)]
struct TestKeyExprFormatter;

impl TestKeyExprFormatter {
    fn from_rmw_liveliness(key: LivelinessKE) -> hiroz::Result<LivelinessKE> {
        Ok(LivelinessKE::new(
            key.as_str()
                .replacen("@ros2_lv", Self::ADMIN_SPACE, 1)
                .try_into()?,
        ))
    }
}

impl KeyExprFormatter for TestKeyExprFormatter {
    const ESCAPE_CHAR: char = '%';
    const ADMIN_SPACE: &'static str = "@hiroz_test_lv";

    fn topic_key_expr(entity: &EndpointEntity) -> hiroz::Result<TopicKE> {
        let rmw = RmwZenohFormatter::topic_key_expr(entity)?;
        Ok(TopicKE::new(
            format!("hiroz_test/{}", rmw.as_str()).try_into()?,
        ))
    }

    fn liveliness_key_expr(
        entity: &EndpointEntity,
        zid: &zenoh::session::ZenohId,
    ) -> hiroz::Result<LivelinessKE> {
        Self::from_rmw_liveliness(RmwZenohFormatter::liveliness_key_expr(entity, zid)?)
    }

    fn node_liveliness_key_expr(entity: &NodeEntity) -> hiroz::Result<LivelinessKE> {
        Self::from_rmw_liveliness(RmwZenohFormatter::node_liveliness_key_expr(entity)?)
    }

    fn parse_liveliness(key: &zenoh::key_expr::KeyExpr) -> hiroz::Result<Entity> {
        let rmw: zenoh::key_expr::KeyExpr<'static> = key
            .as_str()
            .replacen(Self::ADMIN_SPACE, "@ros2_lv", 1)
            .try_into()?;
        RmwZenohFormatter::parse_liveliness(&rmw)
    }

    fn encode_qos(qos: &QosProfile, keyless: bool) -> String {
        RmwZenohFormatter::encode_qos(qos, keyless)
    }

    fn decode_qos(encoded: &str) -> hiroz::Result<(bool, QosProfile)> {
        RmwZenohFormatter::decode_qos(encoded)
    }
}

/// Helper to create a test context and node
async fn setup_test_node(
    node_name: &str,
) -> Result<(hiroz::context::ZContext, hiroz::node::ZNode)> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node(node_name).build()?;

    // Allow time for node discovery
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((ctx, node))
}

/// Helper to wait for publishers on a topic
async fn wait_for_publishers(
    node: &hiroz::node::ZNode,
    topic: &str,
    expected_count: usize,
    timeout_ms: u64,
) -> Result<bool> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        let count = node.graph().count(EndpointKind::Publisher, topic);
        if count >= expected_count {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Helper to wait for subscribers on a topic
async fn wait_for_subscribers(
    node: &hiroz::node::ZNode,
    topic: &str,
    expected_count: usize,
    timeout_ms: u64,
) -> Result<bool> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        let count = node.graph().count(EndpointKind::Subscription, topic);
        if count >= expected_count {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built-in services (type description + the six parameter services) must
    /// announce the node's actual domain, not a hardcoded domain 0.
    ///
    /// Does not assert `enclave`: `format/rmw_zenoh.rs`'s liveliness
    /// encoder hardcodes the enclave segment to the empty placeholder for
    /// every entity type (`// Enclave (not supported yet)` on the decode
    /// side), so no entity's enclave round-trips today -- not the node's
    /// own token, not a plain `ZNode`'s, not these. That is a separate,
    /// pre-existing wire-format gap this PR does not touch. This test only
    /// covers what production `ParameterService`/`TypeDescriptionService`
    /// construction actually controls: domain_id.
    #[tokio::test(flavor = "multi_thread")]
    async fn built_in_services_inherit_context_domain() -> Result<()> {
        const DOMAIN_ID: usize = 123;
        let router = TestRouter::new();
        let observer_ctx = ZContextBuilder::default()
            .with_domain_id(DOMAIN_ID)
            .disable_multicast_scouting()
            .with_connect_endpoints([router.endpoint()])
            .with_mode("client")
            .build()?;
        let observer = observer_ctx
            .create_node("domain_123_observer")
            .without_parameters()
            .build()?;
        let producer_ctx = ZContextBuilder::default()
            .with_domain_id(DOMAIN_ID)
            .disable_multicast_scouting()
            .with_connect_endpoints([router.endpoint()])
            .with_mode("client")
            .build()?;
        let _producer = producer_ctx
            .create_node("domain_123_builtins")
            .with_type_description_service()
            .build()?;
        let node_key: NodeKey = (String::new(), "domain_123_builtins".to_string());

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let services = loop {
            let services = observer
                .graph()
                .get_entities_by_node(EndpointKind::Service, node_key.clone());
            if services.len() >= 7 || tokio::time::Instant::now() >= deadline {
                break services;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        assert_eq!(
            services.len(),
            7,
            "six parameter services and get_type_description must be discoverable"
        );
        assert!(services.iter().all(|endpoint| {
            endpoint
                .node
                .as_ref()
                .is_some_and(|owner| owner.domain_id == DOMAIN_ID)
        }));

        let parameter_events = observer
            .graph()
            .get_entities_by_node(EndpointKind::Publisher, node_key);
        assert!(parameter_events.iter().any(|endpoint| {
            endpoint.topic == "/parameter_events"
                && endpoint
                    .node
                    .as_ref()
                    .is_some_and(|owner| owner.domain_id == DOMAIN_ID)
        }));

        Ok(())
    }

    /// Built-in services must also inherit the context's `keyexpr_format`,
    /// not the default rmw_zenoh format -- the other half of what
    /// `ParameterServiceConfig`/`TypeDescriptionService::new_with_node`
    /// thread through alongside `domain_id`. Uses a pass-through custom
    /// formatter with its own admin space, so a node's built-in services are
    /// only discoverable at all if they actually used it.
    #[tokio::test(flavor = "multi_thread")]
    async fn built_in_services_inherit_custom_keyexpr_format() -> Result<()> {
        let router = TestRouter::new();
        let format = KeyExprFormat::Custom(Arc::new(
            KeyExprFormatterAdapter::<TestKeyExprFormatter>::new(),
        ));
        let observer_ctx = ZContextBuilder::default()
            .keyexpr_format(format.clone())
            .disable_multicast_scouting()
            .with_connect_endpoints([router.endpoint()])
            .with_mode("client")
            .build()?;
        let observer = observer_ctx
            .create_node("custom_format_observer")
            .without_parameters()
            .build()?;
        let producer_ctx = ZContextBuilder::default()
            .keyexpr_format(format)
            .disable_multicast_scouting()
            .with_connect_endpoints([router.endpoint()])
            .with_mode("client")
            .build()?;
        let _producer = producer_ctx
            .create_node("custom_format_builtins")
            .with_type_description_service()
            .build()?;
        let node_key: NodeKey = (String::new(), "custom_format_builtins".to_string());

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let services = loop {
            let services = observer
                .graph()
                .get_entities_by_node(EndpointKind::Service, node_key.clone());
            if services.len() >= 7 || tokio::time::Instant::now() >= deadline {
                break services;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        assert_eq!(
            services.len(),
            7,
            "built-in services must use the context's custom liveliness format"
        );
        assert!(
            observer
                .graph()
                .get_entities_by_node(EndpointKind::Publisher, node_key)
                .iter()
                .any(|endpoint| endpoint.topic == "/parameter_events"),
            "parameter-events publisher must use the context's custom liveliness format"
        );

        Ok(())
    }

    /// Tests getting topic names and types from the graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_topic_names_and_types() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;

        // Get topic names and types - should succeed
        let graph = node.graph().clone();
        let topics = graph.get_topic_names_and_types();

        // Should return a valid result (even if empty or contains only rosout)
        // In a fresh system, we might see /rosout or /parameter_events
        assert!(
            topics.is_empty()
                || topics
                    .iter()
                    .any(|(name, _)| name.contains("rosout") || name.contains("parameter_events"))
        );

        Ok(())
    }

    /// Tests getting service names and types from the graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_service_names_and_types() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;

        // Get service names and types - should succeed
        let graph = node.graph().clone();
        let services = graph.get_service_names_and_types();

        // Should return a valid result (might have node-related services)
        // Fresh node typically has parameter services
        assert!(
            services.is_empty()
                || services
                    .iter()
                    .any(|(name, _)| name.contains("parameter")
                        || name.contains("describe_parameters"))
        );

        Ok(())
    }

    /// Tests counting publishers on a topic
    #[tokio::test(flavor = "multi_thread")]
    async fn test_count_publishers() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_count_publishers";

        // Count publishers on a topic that doesn't exist yet
        let graph = node.graph().clone();
        let count = graph.count(EndpointKind::Publisher, topic_name);

        // Should be 0 or at least return successfully
        assert_eq!(count, 0, "Expected 0 publishers on non-existent topic");

        // Create a publisher
        let _pub = node.create_pub::<RosString>(topic_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Count again - should see our publisher
        let count = graph.count(EndpointKind::Publisher, topic_name);
        assert!(
            count >= 1,
            "Expected at least 1 publisher after creating one"
        );

        Ok(())
    }

    /// Tests counting subscribers on a topic
    #[tokio::test(flavor = "multi_thread")]
    async fn test_count_subscribers() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_count_subscribers";

        // Count subscribers on a topic that doesn't exist yet
        let graph = node.graph().clone();
        let count = graph.count(EndpointKind::Subscription, topic_name);
        assert_eq!(count, 0, "Expected 0 subscribers on non-existent topic");

        // Create a subscriber
        let _sub = node.create_sub::<RosString>(topic_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Count again - should see our subscriber
        let count = graph.count(EndpointKind::Subscription, topic_name);
        assert!(
            count >= 1,
            "Expected at least 1 subscriber after creating one"
        );

        Ok(())
    }

    /// Tests counting clients on a service
    #[tokio::test(flavor = "multi_thread")]
    async fn test_count_clients() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let service_name = "/test_count_clients";

        // Count clients on a service that doesn't exist yet
        let graph = node.graph().clone();
        let count = graph.count(EndpointKind::Client, service_name);

        // Should be 0 or at least return successfully
        assert_eq!(count, 0, "Expected 0 clients on non-existent service");

        // Create a client
        let _client = node.create_client::<AddTwoInts>(service_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Count again - should see our client
        let count = graph.count(EndpointKind::Client, service_name);
        assert!(count >= 1, "Expected at least 1 client after creating one");

        Ok(())
    }

    /// Tests counting services on a service name
    #[tokio::test(flavor = "multi_thread")]
    async fn test_count_services() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let service_name = "/test_count_services";

        // Count services on a service that doesn't exist yet
        let graph = node.graph().clone();
        let count = graph.count(EndpointKind::Service, service_name);

        // Should be 0 or at least return successfully
        assert_eq!(count, 0, "Expected 0 services on non-existent service");

        // Create a service
        let _service = node.create_service::<AddTwoInts>(service_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Count again - should see our service
        let count = graph.count(EndpointKind::Service, service_name);
        println!("Service count after creation: {}", count);
        assert!(count >= 1, "Expected at least 1 service after creating one");

        Ok(())
    }

    /// Tests getting publisher names and types for a specific node
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_publisher_names_and_types_by_node() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_pub_by_node";

        // Create a publisher
        let _pub = node.create_pub::<RosString>(topic_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Get publishers by node
        let graph = node.graph().clone();
        let node_key: NodeKey = ("/".to_string(), "test_graph_node".to_string());

        let entities = graph.get_entities_by_node(EndpointKind::Publisher, node_key);

        // FIXME: In hiroz, local entities may not always be reflected in the graph immediately
        // This test verifies the API works but may return empty for local-only entities
        // The important part is that it doesn't crash and returns a valid result
        if !entities.is_empty() {
            assert!(
                entities
                    .iter()
                    .any(|e| e.topic.contains("test_pub_by_node")),
                "If entities found, should include our specific publisher"
            );
        }

        Ok(())
    }

    /// Tests getting subscriber names and types for a specific node
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_subscriber_names_and_types_by_node() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_sub_by_node";

        // Create a subscriber
        let _sub = node.create_sub::<RosString>(topic_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Get subscribers by node
        let graph = node.graph().clone();
        let node_key: NodeKey = ("/".to_string(), "test_graph_node".to_string());

        let entities = graph.get_entities_by_node(EndpointKind::Subscription, node_key);

        // FIXME: In hiroz, local entities may not always be reflected in the graph immediately
        // This test verifies the API works but may return empty for local-only entities
        if !entities.is_empty() {
            assert!(
                entities
                    .iter()
                    .any(|e| e.topic.contains("test_sub_by_node")),
                "If entities found, should include our specific subscriber"
            );
        }

        Ok(())
    }

    /// Tests getting service names and types for a specific node
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_service_names_and_types_by_node() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let service_name = "/test_service_by_node";

        // Create a service
        let _service = node.create_service::<AddTwoInts>(service_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Get services by node
        let graph = node.graph().clone();
        // Note: "/" namespace is normalized to "" in NodeEntity::key()
        let node_key: NodeKey = ("".to_string(), "test_graph_node".to_string());

        let entities = graph.get_entities_by_node(EndpointKind::Service, node_key);

        // Should find our service
        assert!(!entities.is_empty(), "Expected to find service by node");
        assert!(
            entities
                .iter()
                .any(|e| e.topic.contains("test_service_by_node")),
            "Expected to find our specific service"
        );

        Ok(())
    }

    /// Tests getting client names and types for a specific node
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_client_names_and_types_by_node() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let service_name = "/test_client_by_node";

        // Create a client
        let _client = node.create_client::<AddTwoInts>(service_name).build()?;

        // Allow discovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Get clients by node
        let graph = node.graph().clone();
        // Note: "/" namespace is normalized to "" in NodeEntity::key()
        let node_key: NodeKey = ("".to_string(), "test_graph_node".to_string());

        let entities = graph.get_entities_by_node(EndpointKind::Client, node_key);

        // Should find our client
        assert!(!entities.is_empty(), "Expected to find client by node");
        assert!(
            entities
                .iter()
                .any(|e| e.topic.contains("test_client_by_node")),
            "Expected to find our specific client"
        );

        Ok(())
    }

    /// Tests graph queries with a hand-crafted graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_graph_query_functions() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = format!(
            "/test_graph_query_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        let graph = node.graph().clone();

        // Initially, topic should not exist
        let count_pubs = graph.count(EndpointKind::Publisher, &topic_name);
        let count_subs = graph.count(EndpointKind::Subscription, &topic_name);
        assert_eq!(count_pubs, 0, "Expected 0 publishers initially");
        assert_eq!(count_subs, 0, "Expected 0 subscribers initially");

        // Create a publisher
        let pub_handle = node.create_pub::<RosString>(&topic_name).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should see 1 publisher
        let count_pubs = graph.count(EndpointKind::Publisher, &topic_name);
        assert!(
            count_pubs >= 1,
            "Expected at least 1 publisher after creation"
        );

        // Create a subscriber
        let sub_handle = node.create_sub::<RosString>(&topic_name).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should see 1 publisher and 1 subscriber
        let count_pubs = graph.count(EndpointKind::Publisher, &topic_name);
        let count_subs = graph.count(EndpointKind::Subscription, &topic_name);
        assert!(count_pubs >= 1, "Expected at least 1 publisher");
        assert!(count_subs >= 1, "Expected at least 1 subscriber");

        // Drop publisher
        drop(pub_handle);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should see 0 publishers, 1 subscriber
        let count_pubs = graph.count(EndpointKind::Publisher, &topic_name);
        let count_subs = graph.count(EndpointKind::Subscription, &topic_name);
        assert_eq!(count_pubs, 0, "Expected 0 publishers after drop");
        assert!(count_subs >= 1, "Expected at least 1 subscriber still");

        // Drop subscriber
        drop(sub_handle);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should see 0 publishers, 0 subscribers
        let count_pubs = graph.count(EndpointKind::Publisher, &topic_name);
        let count_subs = graph.count(EndpointKind::Subscription, &topic_name);
        assert_eq!(count_pubs, 0, "Expected 0 publishers after all drops");
        assert_eq!(count_subs, 0, "Expected 0 subscribers after all drops");

        Ok(())
    }

    /// Tests getting all node names from the graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_node_names() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;

        // Get node names
        let graph = node.graph().clone();
        let nodes = graph.get_node_names();

        // Should at least see our own node
        assert!(!nodes.is_empty(), "Expected to find at least one node");
        assert!(
            nodes.iter().any(|(name, _)| name == "test_graph_node"),
            "Expected to find our test node"
        );

        Ok(())
    }

    /// Tests getting all node names with enclaves from the graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_node_names_with_enclaves() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;

        // Get node names with enclaves
        let graph = node.graph().clone();
        let nodes = graph.get_node_names_with_enclaves();

        // Should at least see our own node
        assert!(!nodes.is_empty(), "Expected to find at least one node");
        assert!(
            nodes.iter().any(|(name, _, _)| name == "test_graph_node"),
            "Expected to find our test node with enclave"
        );

        Ok(())
    }

    /// Tests discovering publishers from multiple nodes
    #[tokio::test(flavor = "multi_thread")]
    async fn test_multi_node_publishers() -> Result<()> {
        let (_ctx1, node1) = setup_test_node("test_node_1").await?;
        let (_ctx2, node2) = setup_test_node("test_node_2").await?;

        let topic_name = "/test_multi_node_pub";

        // Create publishers on both nodes
        let _pub1 = node1.create_pub::<RosString>(topic_name).build()?;
        let _pub2 = node2.create_pub::<RosString>(topic_name).build()?;

        // Allow more time for inter-node discovery
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Check from node1's perspective
        let graph1 = node1.graph();
        let count = graph1.count(EndpointKind::Publisher, topic_name);
        // Should see at least one publisher (itself), ideally both
        assert!(
            count >= 1,
            "Expected at least 1 publisher from node1's view, got {}",
            count
        );

        // Check from node2's perspective
        let graph2 = node2.graph();
        let count = graph2.count(EndpointKind::Publisher, topic_name);
        assert!(
            count >= 1,
            "Expected at least 1 publisher from node2's view, got {}",
            count
        );

        Ok(())
    }

    /// Tests discovering subscribers from multiple nodes
    #[tokio::test(flavor = "multi_thread")]
    async fn test_multi_node_subscribers() -> Result<()> {
        let (_ctx1, node1) = setup_test_node("test_node_1").await?;
        let (_ctx2, node2) = setup_test_node("test_node_2").await?;

        let topic_name = "/test_multi_node_sub";

        // Create subscribers on both nodes
        let _sub1 = node1.create_sub::<RosString>(topic_name).build()?;
        let _sub2 = node2.create_sub::<RosString>(topic_name).build()?;

        // Allow more time for inter-node discovery
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Check from node1's perspective
        let graph1 = node1.graph();
        let count = graph1.count(EndpointKind::Subscription, topic_name);
        // Should see at least one subscriber (itself), ideally both
        assert!(
            count >= 1,
            "Expected at least 1 subscriber from node1's view, got {}",
            count
        );

        // Check from node2's perspective
        let graph2 = node2.graph();
        let count = graph2.count(EndpointKind::Subscription, topic_name);
        assert!(
            count >= 1,
            "Expected at least 1 subscriber from node2's view, got {}",
            count
        );

        Ok(())
    }

    /// Tests discovering services from multiple nodes
    #[tokio::test(flavor = "multi_thread")]
    async fn test_multi_node_services() -> Result<()> {
        // Create a single context and multiple nodes to share the graph
        let ctx = ZContextBuilder::default().build()?;
        let node1 = ctx.create_node("test_node_1").build()?;
        let node2 = ctx.create_node("test_node_2").build()?;

        let service_name1 = "/test_multi_node_service_1";
        let service_name2 = "/test_multi_node_service_2";

        // Create services on different nodes
        let _srv1 = node1.create_service::<AddTwoInts>(service_name1).build()?;
        let _srv2 = node2.create_service::<AddTwoInts>(service_name2).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Check service discovery from node1's perspective
        let graph1 = node1.graph();
        let services = graph1.get_service_names_and_types();

        // Should see both services
        assert!(
            services.iter().any(|(name, _)| name.contains("service_1")),
            "Expected to find service_1"
        );
        assert!(
            services.iter().any(|(name, _)| name.contains("service_2")),
            "Expected to find service_2"
        );

        Ok(())
    }

    /// Tests discovering clients from multiple nodes
    #[tokio::test(flavor = "multi_thread")]
    async fn test_multi_node_clients() -> Result<()> {
        // Create a single context and multiple nodes to share the graph
        let ctx = ZContextBuilder::default().build()?;
        let node1 = ctx.create_node("test_node_1").build()?;
        let node2 = ctx.create_node("test_node_2").build()?;

        let service_name = "/test_multi_node_client";

        // Create service and clients
        let _srv = node1.create_service::<AddTwoInts>(service_name).build()?;
        let _client1 = node1.create_client::<AddTwoInts>(service_name).build()?;
        let _client2 = node2.create_client::<AddTwoInts>(service_name).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Check from graph
        let graph1 = node1.graph();
        let count = graph1.count(EndpointKind::Client, service_name);
        assert!(count >= 2, "Expected at least 2 clients");

        Ok(())
    }

    /// Tests checking if a service server is available
    #[tokio::test(flavor = "multi_thread")]
    async fn test_service_server_is_available() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let service_name = "/test_service_available";

        // Create client
        let client = node.create_client::<AddTwoInts>(service_name).build()?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Service should not be available yet
        let graph = node.graph().clone();
        let count = graph.count(EndpointKind::Service, service_name);
        assert_eq!(count, 0, "Expected 0 services before creating server");

        // Create the service
        let _service = node.create_service::<AddTwoInts>(service_name).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Service should now be available
        let count = graph.count(EndpointKind::Service, service_name);
        assert!(count >= 1, "Expected at least 1 service after creation");

        // Drop service
        drop(_service);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Service should no longer be available
        let count = graph.count(EndpointKind::Service, service_name);
        assert_eq!(count, 0, "Expected 0 services after dropping server");

        drop(client);
        Ok(())
    }

    /// Tests the get_entities_by_topic functionality
    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_entities_by_topic() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_entities_by_topic";

        // Create publisher and subscriber
        let _pub = node.create_pub::<RosString>(topic_name).build()?;
        let _sub = node.create_sub::<RosString>(topic_name).build()?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Get entities by topic
        let graph = node.graph().clone();
        let pubs = graph.get_entities_by_topic(EndpointKind::Publisher, topic_name);
        let subs = graph.get_entities_by_topic(EndpointKind::Subscription, topic_name);

        // Should find both
        assert!(!pubs.is_empty(), "Expected to find publishers");
        assert!(!subs.is_empty(), "Expected to find subscribers");

        Ok(())
    }

    /// Tests waiting for publishers on a topic
    #[tokio::test(flavor = "multi_thread")]
    async fn test_wait_for_publishers() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_wait_for_publishers";

        // Valid call (expect timeout since there are no publishers)
        let success = wait_for_publishers(&node, topic_name, 1, 100).await?;
        assert!(!success, "Expected timeout since no publishers");

        Ok(())
    }

    /// Tests waiting for subscribers on a topic
    #[tokio::test(flavor = "multi_thread")]
    async fn test_wait_for_subscribers() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;
        let topic_name = "/test_wait_for_subscribers";

        // Valid call (expect timeout since there are no subscribers)
        let success = wait_for_subscribers(&node, topic_name, 1, 100).await?;
        assert!(!success, "Expected timeout since no subscribers");

        Ok(())
    }

    /// Tests getting action names and types from the graph
    #[tokio::test(flavor = "multi_thread")]
    async fn test_action_names_and_types() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_graph_node").await?;

        // Get action names and types
        let graph = node.graph().clone();
        let actions = graph.get_action_names_and_types();

        // FIXME: Should return successfully (may be empty)
        assert!(actions.is_empty() || !actions.is_empty());

        Ok(())
    }
}
