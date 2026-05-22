//! Unit tests for SHM pub/sub functionality

use std::{sync::Arc, time::Duration};

use hiroz::{
    Builder, ZBuf,
    context::ZContextBuilder,
    shm::{ShmConfig, ShmProviderBuilder},
};
use hiroz_msgs::std_msgs::ByteMultiArray;
use zenoh_buffers::buffer::{Buffer, SplitBuffer};

#[test]
fn test_shm_pubsub_large_message() {
    // Setup context with SHM enabled
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(512 * 1024) // 512KB is enough for tests
        .expect("Failed to enable SHM")
        .with_shm_threshold(1000)
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<ByteMultiArray>("shm_test_topic")
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("shm_test_topic")
        .build()
        .expect("Failed to create subscriber");

    // Give some time for discovery
    std::thread::sleep(Duration::from_millis(500));

    // Create message larger than threshold (10KB > 1KB threshold)
    let large_data = vec![0xAA; 10_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(large_data.clone()),
        ..Default::default()
    };

    // Publish
    publisher.publish(&msg).expect("Failed to publish");

    // Receive
    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    // Verify
    assert_eq!(received.data.len(), 10_000, "Data length mismatch");

    // Verify content
    let received_bytes = received.data.contiguous();
    assert_eq!(
        &received_bytes[..],
        &large_data[..],
        "Data content mismatch"
    );
}

#[test]
fn test_shm_pubsub_small_message() {
    // Setup context with SHM enabled but high threshold
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(512 * 1024) // 512KB is enough for tests
        .expect("Failed to enable SHM")
        .with_shm_threshold(10_000) // 10KB threshold
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<ByteMultiArray>("small_topic")
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("small_topic")
        .build()
        .expect("Failed to create subscriber");

    std::thread::sleep(Duration::from_millis(500));

    // Create message smaller than threshold (1KB < 10KB threshold)
    let small_data = vec![0xBB; 1_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(small_data.clone()),
        ..Default::default()
    };

    // Publish (should use regular memory, not SHM)
    publisher.publish(&msg).expect("Failed to publish");

    // Receive
    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    // Verify
    assert_eq!(received.data.len(), 1_000, "Data length mismatch");

    let received_bytes = received.data.contiguous();
    assert_eq!(
        &received_bytes[..],
        &small_data[..],
        "Data content mismatch"
    );
}

#[test]
fn test_shm_threshold_boundary() {
    // Test message exactly at threshold
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(512 * 1024) // 512KB is enough for tests
        .expect("Failed to enable SHM")
        .with_shm_threshold(5_000) // Exactly 5KB
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<ByteMultiArray>("boundary_topic")
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("boundary_topic")
        .build()
        .expect("Failed to create subscriber");

    std::thread::sleep(Duration::from_millis(500));

    // Message exactly at threshold (including CDR header)
    let boundary_data = vec![0xCC; 5_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(boundary_data.clone()),
        ..Default::default()
    };

    publisher.publish(&msg).expect("Failed to publish");

    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    assert_eq!(received.data.len(), 5_000, "Data length mismatch");
}

#[test]
fn test_shm_config_hierarchy_node_override() {
    // Context has SHM with one config, node overrides with another
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(512 * 1024) // 512KB is enough for tests
        .expect("Failed to enable SHM")
        .with_shm_threshold(10_000) // 10KB at context level
        .build()
        .expect("Failed to create context");

    // Node overrides with lower threshold
    let provider = Arc::new(
        ShmProviderBuilder::new(512 * 1024) // 512KB is enough for test
            .build()
            .expect("Failed to create SHM provider"),
    );
    let node_shm_config = ShmConfig::new(provider).with_threshold(2_000); // 2KB

    let node = ctx
        .create_node("test_node")
        .with_shm_config(node_shm_config)
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<ByteMultiArray>("hierarchy_topic")
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("hierarchy_topic")
        .build()
        .expect("Failed to create subscriber");

    std::thread::sleep(Duration::from_millis(500));

    // Message between node threshold (2KB) and context threshold (10KB)
    let data = vec![0xDD; 5_000]; // 5KB - should use node's SHM config
    let msg = ByteMultiArray {
        data: ZBuf::from(data.clone()),
        ..Default::default()
    };

    publisher.publish(&msg).expect("Failed to publish");

    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    assert_eq!(received.data.len(), 5_000, "Data length mismatch");
}

#[test]
fn test_without_shm() {
    // Context has SHM enabled, but publisher explicitly disables it
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(512 * 1024) // 512KB is enough for tests
        .expect("Failed to enable SHM")
        .with_shm_threshold(1_000)
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<ByteMultiArray>("no_shm_topic")
        .without_shm() // Explicitly disable SHM
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("no_shm_topic")
        .build()
        .expect("Failed to create subscriber");

    std::thread::sleep(Duration::from_millis(500));

    // Large message that would normally use SHM
    let data = vec![0xEE; 10_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(data.clone()),
        ..Default::default()
    };

    // Should publish without SHM despite being large
    publisher.publish(&msg).expect("Failed to publish");

    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    assert_eq!(received.data.len(), 10_000, "Data length mismatch");
}

#[test]
fn test_publisher_shm_override() {
    // Context has no SHM, but publisher enables it
    let ctx = ZContextBuilder::default()
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    // Publisher has its own SHM config
    let provider = Arc::new(
        ShmProviderBuilder::new(512 * 1024) // 512KB is enough for test
            .build()
            .expect("Failed to create SHM provider"),
    );
    let pub_shm_config = ShmConfig::new(provider).with_threshold(2_000);

    let publisher = node
        .create_pub::<ByteMultiArray>("pub_shm_topic")
        .with_shm_config(pub_shm_config)
        .build()
        .expect("Failed to create publisher");

    let subscriber = node
        .create_sub::<ByteMultiArray>("pub_shm_topic")
        .build()
        .expect("Failed to create subscriber");

    std::thread::sleep(Duration::from_millis(500));

    let data = vec![0xFF; 5_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(data.clone()),
        ..Default::default()
    };

    publisher.publish(&msg).expect("Failed to publish");

    let received = subscriber
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive message");

    assert_eq!(received.data.len(), 5_000, "Data length mismatch");
}

#[test]
fn test_multiple_publishers_different_thresholds() {
    // Multiple publishers with different SHM configs
    // Use smaller pool sizes to avoid exhausting system SHM limits
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(1024 * 1024) // 1MB is enough for test
        .expect("Failed to enable SHM")
        .with_shm_threshold(5_000)
        .build()
        .expect("Failed to create context");

    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    // Publisher 1: uses context default
    let pub1 = node
        .create_pub::<ByteMultiArray>("topic1")
        .build()
        .expect("Failed to create publisher 1");

    // Publisher 2: lower threshold
    let provider2 = Arc::new(
        ShmProviderBuilder::new(512 * 1024) // 512KB is enough for test
            .build()
            .expect("Failed to create SHM provider 2"),
    );
    let pub2 = node
        .create_pub::<ByteMultiArray>("topic2")
        .with_shm_config(ShmConfig::new(provider2).with_threshold(1_000))
        .build()
        .expect("Failed to create publisher 2");

    let sub1 = node
        .create_sub::<ByteMultiArray>("topic1")
        .build()
        .expect("Failed to create subscriber 1");

    let sub2 = node
        .create_sub::<ByteMultiArray>("topic2")
        .build()
        .expect("Failed to create subscriber 2");

    std::thread::sleep(Duration::from_millis(500));

    // Message of 3KB - uses SHM on pub2 but not pub1
    let data = vec![0x11; 3_000];
    let msg = ByteMultiArray {
        data: ZBuf::from(data.clone()),
        ..Default::default()
    };

    pub1.publish(&msg).expect("Failed to publish 1");
    pub2.publish(&msg).expect("Failed to publish 2");

    let recv1 = sub1
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive from topic 1");
    let recv2 = sub2
        .recv_timeout(Duration::from_secs(2))
        .expect("Failed to receive from topic 2");

    assert_eq!(recv1.data.len(), 3_000);
    assert_eq!(recv2.data.len(), 3_000);
}
