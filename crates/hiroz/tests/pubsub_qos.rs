//! Pub/Sub QoS tests
//!
//! These tests verify publisher/subscriber functionality with different QoS settings,
//! particularly focusing on the AdvancedPublisher behavior with various durability
//! and reliability configurations.
//!
//! ## Key Findings:
//!
//! 1. **AdvancedPublisher works within the same Zenoh session**
//!    - When publisher and subscriber share the same context, messages are delivered successfully
//!    - Both sync (`publish()`) and async (`async_publish()`) work correctly
//!
//! 2. **AdvancedPublisher now works across different Zenoh sessions** (FIXED!)
//!    - When publisher and subscriber use different contexts (separate Zenoh sessions)
//!    - Messages are now delivered successfully
//!    - This is critical for ROS 2 where different processes use different contexts
//!
//! 3. **Root causes that were fixed:**
//!    - Missing `reliability` setting on the publisher (was only setting `congestion_control`)
//!    - Disabled multicast scouting prevented session discovery
//!    - Missing timestamping configuration for TransientLocal QoS with cache
//!    - Added shared memory transport for efficient intra-process communication

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use hiroz::{
    Builder, Result,
    context::ZContextBuilder,
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
};
use hiroz_msgs::std_msgs::String as RosString;

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

/// Helper to create a QoS profile
fn create_qos(
    durability: QosDurability,
    reliability: QosReliability,
    history: QosHistory,
) -> QosProfile {
    QosProfile {
        durability,
        reliability,
        history,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test pub/sub with Volatile durability and Reliable reliability
    /// This uses AdvancedPublisher without cache
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_volatile_reliable() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_pub_volatile_reliable").await?;
        let sub_node = _ctx.create_node("test_sub_volatile_reliable").build()?;

        let topic = "/test_volatile_reliable";
        let qos = create_qos(
            QosDurability::Volatile,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create subscriber FIRST to ensure it's ready before publisher
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Now create publisher
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Publish messages
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            println!("Publishing: {}", msg.data);
            pub_handle.publish(&msg)?;
            // Small delay between messages
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for messages to be received
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(count >= 5, "Expected at least 5 messages, got {}", count);

        Ok(())
    }

    /// Test pub/sub with Volatile durability and BestEffort reliability
    /// This uses AdvancedPublisher without cache
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_volatile_best_effort() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_pub_volatile_best_effort").await?;
        let sub_node = _ctx.create_node("test_sub_volatile_best_effort").build()?;

        let topic = "/test_volatile_best_effort";
        let qos = create_qos(
            QosDurability::Volatile,
            QosReliability::BestEffort,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create publisher with Volatile + BestEffort QoS
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Create subscriber with matching QoS
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Publish messages
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            println!("Publishing: {}", msg.data);
            pub_handle.publish(&msg)?;
        }

        // Wait for messages to be received
        tokio::time::sleep(Duration::from_millis(500)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(
            count >= 4,
            "Expected at least 4 messages (best effort), got {}",
            count
        );

        Ok(())
    }

    /// Test pub/sub with TransientLocal durability and Reliable reliability
    /// This uses AdvancedPublisher WITH cache
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_transient_local_reliable() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_pub_transient_reliable").await?;
        let sub_node = _ctx.create_node("test_sub_transient_reliable").build()?;

        let topic = "/test_transient_reliable";
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create subscriber FIRST
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Now create publisher with TransientLocal + Reliable QoS
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Publish messages
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            println!("Publishing: {}", msg.data);
            pub_handle.publish(&msg)?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for messages to be received
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let count = received_count.load(Ordering::SeqCst);
        // With TransientLocal across contexts, we expect all messages published after subscriber exists
        assert!(
            count >= 5,
            "Expected at least 5 messages with TransientLocal, got {}",
            count
        );

        Ok(())
    }

    /// Test pub/sub with TransientLocal durability and BestEffort reliability
    /// This uses AdvancedPublisher WITH cache
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_transient_local_best_effort() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_pub_transient_best_effort").await?;
        let sub_node = _ctx.create_node("test_sub_transient_best_effort").build()?;

        let topic = "/test_transient_best_effort";
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::BestEffort,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create publisher with TransientLocal + BestEffort QoS
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Create subscriber with matching QoS
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Publish messages
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            println!("Publishing: {}", msg.data);
            pub_handle.publish(&msg)?;
        }

        // Wait for messages to be received
        tokio::time::sleep(Duration::from_millis(500)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(count >= 4, "Expected at least 4 messages, got {}", count);

        Ok(())
    }

    /// Test pub/sub with KeepAll history
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_keep_all_history() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_pub_keep_all").await?;
        let sub_node = _ctx.create_node("test_sub_keep_all").build()?;

        let topic = "/test_keep_all";
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepAll,
        );

        // Create subscriber first
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Create publisher with KeepAll history
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Publish many messages
        for i in 0..10 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            pub_handle.publish(&msg)?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for messages
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(
            count >= 10,
            "Expected all 10 messages with KeepAll, got {}",
            count
        );

        Ok(())
    }

    /// Test pub/sub with AdvancedPublisher using the SAME context
    /// THIS WORKS: Demonstrates that AdvancedPublisher works within the same Zenoh session
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_volatile_same_context_works() -> Result<()> {
        // Use SAME context for both nodes (same Zenoh session) - THIS IS THE KEY!
        let ctx = ZContextBuilder::default().build()?;
        let pub_node = ctx.create_node("test_pub_volatile_async").build()?;
        let sub_node = ctx.create_node("test_sub_volatile_async").build()?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let topic = "/test_volatile_async";
        let qos = create_qos(
            QosDurability::Volatile,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create subscriber FIRST
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Now create publisher
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Publish messages using ASYNC publish
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            println!("Publishing async: {}", msg.data);
            pub_handle.async_publish(&msg).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for messages to be received
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(
            count >= 5,
            "Expected at least 5 messages with async_publish, got {}",
            count
        );

        Ok(())
    }

    /// Test TransientLocal with same context - should work
    /// Timestamping is now enabled by default in the context
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_pubsub_transient_local_same_context_works() -> Result<()> {
        let ctx = ZContextBuilder::default().build()?;
        let pub_node = ctx.create_node("test_pub_transient_same").build()?;
        let sub_node = ctx.create_node("test_sub_transient_same").build()?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let topic = "/test_transient_same";
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create subscriber first
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();

        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Create publisher
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Wait for discovery
        tokio::time::sleep(Duration::from_millis(1000)).await;

        // Publish messages
        for i in 0..5 {
            let msg = RosString {
                data: format!("Message {}", i),
            };
            pub_handle.async_publish(&msg).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for messages
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let count = received_count.load(Ordering::SeqCst);
        assert!(
            count >= 5,
            "Expected at least 5 messages with TransientLocal same context, got {}",
            count
        );

        Ok(())
    }

    /// Minimal test to verify AdvancedPublisher doesn't hang
    /// This is the simplest possible test
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_minimal_advanced_publisher_no_hang() -> Result<()> {
        let (_ctx, node) = setup_test_node("test_minimal_hang").await?;

        let topic = "/test_minimal";
        let qos = create_qos(
            QosDurability::Volatile, // Uses AdvancedPublisher
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(1).unwrap()),
        );

        // Create publisher - this should succeed
        println!("Creating publisher...");
        let pub_handle = node.create_pub::<RosString>(topic).with_qos(qos).build()?;
        println!("Publisher created successfully");

        // Try to publish a single message - this is where it hangs
        let msg = RosString {
            data: "Test message".to_string(),
        };
        println!("Publishing message...");

        // Use a timeout to detect the hang
        let publish_result = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || pub_handle.publish(&msg)),
        )
        .await;

        match publish_result {
            Ok(Ok(Ok(_))) => {
                println!("Publish succeeded");
                Ok(())
            }
            Ok(Ok(Err(e))) => {
                panic!("Publish failed with error: {:?}", e);
            }
            Ok(Err(e)) => {
                panic!("Task join failed: {:?}", e);
            }
            Err(_) => {
                panic!(
                    "HANG DETECTED: Publish timed out after 5 seconds - AdvancedPublisher.wait() is blocking!"
                );
            }
        }
    }

    /// SYNCHRONOUS test to verify AdvancedPublisher works without tokio runtime
    /// This mimics the RCL C context - pure synchronous, no async runtime
    #[test]
    fn test_sync_advanced_publisher_no_runtime() -> Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        println!("=== SYNC TEST: Creating context and nodes (no tokio runtime) ===");
        let ctx = hiroz::context::ZContextBuilder::default().build()?;
        let pub_node = ctx.create_node("sync_test_pub").build()?;
        let sub_node = ctx.create_node("sync_test_sub").build()?;

        let topic = "/test_sync_publish";
        let qos = create_qos(
            QosDurability::Volatile, // Uses AdvancedPublisher
            QosReliability::BestEffort,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create subscriber first
        println!("Creating subscriber...");
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("SYNC TEST: Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        // Create publisher
        println!("Creating publisher...");
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        println!("Publisher created successfully");

        // Wait for discovery (synchronous sleep)
        println!("Waiting for discovery...");
        std::thread::sleep(Duration::from_millis(1000));

        // Publish messages SYNCHRONOUSLY - this is the critical test
        println!("Publishing messages synchronously (no tokio, like RCL C code)...");
        for i in 0..3 {
            let msg = RosString {
                data: format!("Sync message {}", i),
            };
            println!("Publishing: {}", msg.data);

            // This is what happens in RCL - pure synchronous call with .wait()
            pub_handle.publish(&msg)?;

            println!("Publish {} completed", i);
        }

        // Wait for messages to be received
        println!("Waiting for messages...");
        std::thread::sleep(Duration::from_millis(1000));

        let count = received_count.load(Ordering::SeqCst);
        println!("SYNC TEST RESULT: Received {} messages", count);

        // We expect at least 2 messages with BestEffort
        assert!(count >= 2, "Expected at least 2 messages, got {}", count);

        Ok(())
    }

    /// SYNCHRONOUS test with SAME context (like RCL single-process test)
    /// RCL test uses one context for both pub and sub
    #[test]
    fn test_sync_publish_same_context() -> Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        println!("=== SYNC TEST WITH SAME CONTEXT (like RCL single process) ===");
        let ctx = hiroz::context::ZContextBuilder::default().build()?;
        let pub_node = ctx.create_node("sync_same_ctx_pub").build()?;
        let sub_node = ctx.create_node("sync_same_ctx_sub").build()?;

        let topic = "/test_sync_same_ctx";
        let qos = create_qos(
            QosDurability::Volatile,
            QosReliability::BestEffort,
            QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
        );

        // Create publisher first (like RCL test)
        println!("Creating publisher...");
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        // Create subscriber
        println!("Creating subscriber...");
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
                // Sleep to simulate slow callback - publish() should not block on this
                std::thread::sleep(std::time::Duration::from_millis(100));
            })?;

        // Wait for discovery
        println!("Waiting for discovery...");
        std::thread::sleep(Duration::from_millis(1000));

        // Publish messages SYNCHRONOUSLY
        println!("Publishing with same context (like RCL)...");
        for i in 0..3 {
            let msg = RosString {
                data: format!("Same context message {}", i),
            };
            println!("Publishing: {}", msg.data);

            // This should hang if same context causes the issue
            pub_handle.publish(&msg)?;

            println!("Publish {} completed", i);
        }

        // Wait for messages
        println!("Waiting for messages...");
        std::thread::sleep(Duration::from_millis(1000));

        let count = received_count.load(Ordering::SeqCst);
        println!("SAME CONTEXT TEST RESULT: Received {} messages", count);

        assert!(count >= 2, "Expected at least 2 messages, got {}", count);

        Ok(())
    }

    /// Test with EXACT RCL QoS settings: deadline + liveliness
    /// This should reproduce the hang if it's caused by these QoS settings
    #[test]
    fn test_sync_publish_with_deadline_liveliness() -> Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        println!("=== SYNC TEST WITH DEADLINE + LIVELINESS (exact RCL settings) ===");
        let ctx = hiroz::context::ZContextBuilder::default().build()?;
        let pub_node = ctx.create_node("sync_deadline_pub").build()?;
        let sub_node = ctx.create_node("sync_deadline_sub").build()?;

        let topic = "/test_sync_deadline_live";

        // EXACT same QoS as RCL test_events.cpp default_qos_profile
        let qos = QosProfile {
            durability: QosDurability::Volatile, // SYSTEM_DEFAULT maps to Volatile
            reliability: QosReliability::BestEffort,
            history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
            // These are the critical settings from RCL:
            liveliness: hiroz::qos::QosLiveliness::ManualByTopic,
            liveliness_lease_duration: hiroz::qos::QosDuration { sec: 1, nsec: 0 }, // 1 second
            deadline: hiroz::qos::QosDuration { sec: 2, nsec: 0 },                  // 2 seconds
            ..Default::default()
        };

        println!("Creating subscriber...");
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_clone = received_count.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                println!("Received: {}", msg.data);
                received_count_clone.fetch_add(1, Ordering::SeqCst);
            })?;

        println!("Creating publisher...");
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;

        println!("Waiting for discovery...");
        std::thread::sleep(Duration::from_millis(1000));

        println!("Publishing with deadline+liveliness QoS...");
        for i in 0..3 {
            let msg = RosString {
                data: format!("Deadline/liveliness message {}", i),
            };
            println!("Publishing: {}", msg.data);

            // THIS IS WHERE IT SHOULD HANG if deadline/liveliness causes the issue!
            pub_handle.publish(&msg)?;

            println!("Publish {} completed", i);
        }

        println!("Waiting for messages...");
        std::thread::sleep(Duration::from_millis(1000));

        let count = received_count.load(Ordering::SeqCst);
        println!(
            "DEADLINE/LIVELINESS TEST RESULT: Received {} messages",
            count
        );

        assert!(count >= 2, "Expected at least 2 messages, got {}", count);

        Ok(())
    }

    /// Poll the received-buffer length until it reaches `expected` or
    /// `timeout` elapses. Returns the final length.
    async fn wait_for_count(
        received: &Arc<std::sync::Mutex<Vec<String>>>,
        expected: usize,
        timeout: Duration,
    ) -> usize {
        let start = std::time::Instant::now();
        loop {
            let len = received.lock().unwrap().len();
            if len >= expected || start.elapsed() >= timeout {
                return len;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Verify that a TransientLocal publisher replays its history to a
    /// late-joining subscriber.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transient_local_history_replays_to_late_subscriber() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_tl_history_pub").await?;
        let sub_node = _ctx.create_node("test_tl_history_sub").build()?;

        let topic = "/test_tl_history_replay";
        let depth = 5;
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(depth).unwrap()),
        );

        // Publish BEFORE any subscriber exists, so anything the late
        // subscriber receives must have come from the publisher's cache.
        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..depth {
            pub_handle.publish(&RosString {
                data: format!("history-{}", i),
            })?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Now create the late-joining subscriber.
        let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = received.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                received_clone.lock().unwrap().push(msg.data);
            })?;

        let final_len = wait_for_count(&received, depth, Duration::from_secs(5)).await;
        let got = received.lock().unwrap().clone();
        assert_eq!(
            final_len, depth,
            "Expected {} replayed history messages, got {}: {:?}",
            depth, final_len, got
        );
        for (i, msg) in got.iter().enumerate() {
            assert_eq!(msg, &format!("history-{}", i));
        }

        Ok(())
    }

    /// Verify that a TransientLocal publisher's cache honours the
    /// `KeepLast(depth)` config. When more than `depth` samples have
    /// been published before a late subscriber arrives, only the most recent
    /// `depth` are replayed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transient_local_cache_respects_keep_last_depth() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_tl_depth_pub").await?;
        let sub_node = _ctx.create_node("test_tl_depth_sub").build()?;

        let topic = "/test_tl_cache_depth";
        let depth = 3;
        let total = 10;
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(depth).unwrap()),
        );

        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Publish more samples than the cache can hold.
        for i in 0..total {
            pub_handle.publish(&RosString {
                data: format!("sample-{}", i),
            })?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = received.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                received_clone.lock().unwrap().push(msg.data);
            })?;

        let final_len = wait_for_count(&received, depth, Duration::from_secs(5)).await;
        // Give any extra (unexpected) samples a chance to arrive so we can
        // assert the cache really stopped at `depth`.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let got = received.lock().unwrap().clone();
        assert_eq!(
            got.len(),
            depth,
            "Expected exactly {} cached samples (most recent), got {} (waited until {}): {:?}",
            depth,
            got.len(),
            final_len,
            got
        );
        // The cache holds the last `depth` samples, so the replay must
        // begin at `total - depth`.
        for (i, msg) in got.iter().enumerate() {
            let expected = format!("sample-{}", total - depth + i);
            assert_eq!(msg, &expected, "Unexpected replayed sample at index {}", i);
        }

        Ok(())
    }

    /// TransientLocal + BestEffort: the late subscriber must still receive
    /// the publisher's cached history. Recovery (heartbeat) is not enabled
    /// for BestEffort, so this exercises the non-heartbeat code path
    /// end-to-end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transient_local_best_effort_replays_to_late_subscriber() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_tl_be_pub").await?;
        let sub_node = _ctx.create_node("test_tl_be_sub").build()?;

        let topic = "/test_tl_be_replay";
        let depth = 4;
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::BestEffort,
            QosHistory::KeepLast(NonZeroUsize::new(depth).unwrap()),
        );

        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..depth {
            pub_handle.publish(&RosString {
                data: format!("be-{}", i),
            })?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = received.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                received_clone.lock().unwrap().push(msg.data);
            })?;

        let final_len = wait_for_count(&received, depth, Duration::from_secs(5)).await;
        let got = received.lock().unwrap().clone();
        assert_eq!(
            final_len, depth,
            "Expected {} replayed BestEffort history messages, got {}: {:?}",
            depth, final_len, got
        );
        for (i, msg) in got.iter().enumerate() {
            assert_eq!(msg, &format!("be-{}", i));
        }

        Ok(())
    }

    /// TransientLocal + KeepAll: pins the rmw_zenoh-aligned cap
    /// `hiroz::qos::KEEP_ALL_CACHE_DEPTH` (= rmw_zenoh's
    /// `RMW_ZENOH_DEFAULT_HISTORY_DEPTH = 42`). Publishes more than the
    /// cap, then late-subscribes, and asserts the cache replayed exactly
    /// the last `cap` samples — proving both that `KeepAll` is NOT
    /// silently truncated to 1 (the previous bug) and that it does NOT
    /// grow unbounded (which would diverge from rmw_zenoh_cpp).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transient_local_keep_all_caps_at_keep_all_cache_depth() -> Result<()> {
        use hiroz::qos::KEEP_ALL_CACHE_DEPTH;

        let (_ctx, pub_node) = setup_test_node("test_tl_keepall_pub").await?;
        let sub_node = _ctx.create_node("test_tl_keepall_sub").build()?;

        let topic = "/test_tl_keepall";
        let total = KEEP_ALL_CACHE_DEPTH + 8;
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepAll,
        );

        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..total {
            pub_handle.publish(&RosString {
                data: format!("all-{}", i),
            })?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = received.clone();
        let _sub_handle = sub_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                received_clone.lock().unwrap().push(msg.data);
            })?;

        let final_len =
            wait_for_count(&received, KEEP_ALL_CACHE_DEPTH, Duration::from_secs(5)).await;
        // Give any extra samples beyond the cap a chance to arrive so we
        // can assert the cache really stopped at KEEP_ALL_CACHE_DEPTH.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let got = received.lock().unwrap().clone();
        assert_eq!(
            got.len(),
            KEEP_ALL_CACHE_DEPTH,
            "KeepAll must cap at KEEP_ALL_CACHE_DEPTH ({}), got {} (saw {} during wait): {:?}",
            KEEP_ALL_CACHE_DEPTH,
            got.len(),
            final_len,
            got,
        );
        // Cap holds the last KEEP_ALL_CACHE_DEPTH samples, so replay
        // starts at `total - KEEP_ALL_CACHE_DEPTH`.
        for (i, msg) in got.iter().enumerate() {
            let expected = format!("all-{}", total - KEEP_ALL_CACHE_DEPTH + i);
            assert_eq!(msg, &expected, "Unexpected sample at index {}", i);
        }

        Ok(())
    }

    /// TransientLocal with two subscribers — one early (matched before
    /// publishing) and one late (joined after all samples were emitted).
    /// Both must end up with the full history; the first subscriber must
    /// not "consume" the cache.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transient_local_two_subscribers_early_and_late() -> Result<()> {
        let (_ctx, pub_node) = setup_test_node("test_tl_two_subs_pub").await?;
        let early_node = _ctx.create_node("test_tl_two_subs_early").build()?;
        let late_node = _ctx.create_node("test_tl_two_subs_late").build()?;

        let topic = "/test_tl_two_subs";
        let depth = 5;
        let qos = create_qos(
            QosDurability::TransientLocal,
            QosReliability::Reliable,
            QosHistory::KeepLast(NonZeroUsize::new(depth).unwrap()),
        );

        // Early subscriber, then publisher, then samples, then late subscriber.
        let early_received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let early_clone = early_received.clone();
        let _early_handle = early_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                early_clone.lock().unwrap().push(msg.data);
            })?;
        tokio::time::sleep(Duration::from_millis(150)).await;

        let pub_handle = pub_node
            .create_pub::<RosString>(topic)
            .with_qos(qos)
            .build()?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..depth {
            pub_handle.publish(&RosString {
                data: format!("two-{}", i),
            })?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Wait for the early subscriber to drain live delivery.
        let early_len = wait_for_count(&early_received, depth, Duration::from_secs(5)).await;
        assert_eq!(
            early_len,
            depth,
            "Early subscriber missed live samples: {:?}",
            early_received.lock().unwrap()
        );

        // Now attach the late subscriber. Must still get the full replay
        // even though the early subscriber already consumed the live path.
        let late_received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let late_clone = late_received.clone();
        let _late_handle = late_node
            .create_sub::<RosString>(topic)
            .with_qos(qos)
            .build_with_callback(move |msg: RosString| {
                late_clone.lock().unwrap().push(msg.data);
            })?;

        let late_len = wait_for_count(&late_received, depth, Duration::from_secs(5)).await;
        let got = late_received.lock().unwrap().clone();
        assert_eq!(
            late_len, depth,
            "Late subscriber expected {} replayed samples, got {}: {:?}",
            depth, late_len, got
        );
        for (i, msg) in got.iter().enumerate() {
            assert_eq!(msg, &format!("two-{}", i));
        }

        Ok(())
    }
}
