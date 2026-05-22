use std::{thread, time::Duration};

use hiroz::{
    Builder, TypeHash, ZBuf,
    attachment::Attachment,
    context::ZContextBuilder,
    ros_msg::MessageTypeInfo,
    time::{ZClock, ZDuration, ZTime},
};
use hiroz_msgs::std_msgs::ByteMultiArray;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zenoh_buffers::buffer::{Buffer, SplitBuffer};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestMessage {
    data: Vec<u8>,
    counter: u64,
}

impl MessageTypeInfo for TestMessage {
    fn type_name() -> &'static str {
        "test_msgs::msg::dds_::TestMessage_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl hiroz::ros_msg::WithTypeInfo for TestMessage {}

impl hiroz::msg::ZMessage for TestMessage {
    type Serdes = hiroz::msg::SerdeCdrSerdes<TestMessage>;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_basic_pubsub() {
    let ctx = ZContextBuilder::default()
        .build()
        .expect("Failed to create context");
    let node = ctx
        .create_node("test_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<TestMessage>("/test_topic")
        .build()
        .unwrap();

    let subscriber = node
        .create_sub::<TestMessage>("/test_topic")
        .build()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg = TestMessage {
        data: vec![1, 2, 3, 4, 5],
        counter: 42,
    };
    publisher.publish(&msg).unwrap();

    let received = subscriber.recv_timeout(Duration::from_secs(1));
    assert!(received.is_ok());
    let received_msg = received.unwrap();
    assert_eq!(received_msg, msg);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_multiple_messages() {
    let ctx = ZContextBuilder::default()
        .build()
        .expect("Failed to create context");
    let node = ctx
        .create_node("multi_msg_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<TestMessage>("/multi_topic")
        .build()
        .unwrap();

    let subscriber = node
        .create_sub::<TestMessage>("/multi_topic")
        .build()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    for i in 0..5 {
        let msg = TestMessage {
            data: vec![i as u8; 100],
            counter: i,
        };
        publisher.publish(&msg).unwrap();
    }

    for i in 0..5 {
        let received = subscriber.recv_timeout(Duration::from_secs(1));
        assert!(received.is_ok());
        let received_msg = received.unwrap();
        assert_eq!(received_msg.counter, i);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_large_payload() {
    let ctx = ZContextBuilder::default()
        .build()
        .expect("Failed to create context");
    let node = ctx
        .create_node("large_payload_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<TestMessage>("/large_topic")
        .build()
        .unwrap();

    let subscriber = node
        .create_sub::<TestMessage>("/large_topic")
        .build()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg = TestMessage {
        data: vec![0xAB; 1024 * 1024],
        counter: 999,
    };
    publisher.publish(&msg).unwrap();

    let received = subscriber.recv_timeout(Duration::from_secs(2));
    assert!(received.is_ok());
    let received_msg = received.unwrap();
    assert_eq!(received_msg.counter, 999);
    assert_eq!(received_msg.data.len(), 1024 * 1024);
    assert_eq!(received_msg.data[0], 0xAB);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_simulated_clock_is_used_for_attachment_timestamps() {
    let clock = ZClock::simulated(ZTime::zero());
    clock.advance(ZDuration::from_secs(5)).unwrap();

    let ctx = ZContextBuilder::default()
        .with_clock(clock.clone())
        .build()
        .expect("Failed to create context");
    let node = ctx
        .create_node("sim_clock_node")
        .build()
        .expect("Failed to create node");

    let publisher = node
        .create_pub::<TestMessage>("/sim_clock")
        .build()
        .unwrap();
    let subscriber = node
        .create_sub::<TestMessage>("/sim_clock")
        .build()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    publisher
        .publish(&TestMessage {
            data: vec![1],
            counter: 1,
        })
        .unwrap();

    let sample = subscriber
        .recv_serialized_timeout(Duration::from_secs(1))
        .expect("Failed to receive sample");
    let attachment = sample
        .attachment()
        .and_then(|att| Attachment::try_from(att).ok())
        .expect("Missing attachment");

    assert_eq!(attachment.source_time(), clock.now());
}

#[test]
fn test_bytemultiarray_pubsub_with_zbuf() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .expect("Failed to create context");

    let publisher_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("test_publisher")
                .build()
                .expect("Failed to create node");

            let publisher = node
                .create_pub::<ByteMultiArray>("zbuf_topic")
                .build()
                .expect("Failed to create publisher");

            let mut buffer = vec![0xAA; 16];
            buffer[0..8].copy_from_slice(&42u64.to_le_bytes());

            let msg = ByteMultiArray {
                data: ZBuf::from(buffer),
                ..Default::default()
            };

            // Give the subscriber time to declare before publishing
            thread::sleep(Duration::from_millis(500));
            publisher.publish(&msg).expect("Failed to publish");
            thread::sleep(Duration::from_millis(100));
        }
    });

    let subscriber_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("test_subscriber")
                .build()
                .expect("Failed to create node");

            let subscriber = node
                .create_sub::<ByteMultiArray>("zbuf_topic")
                .build()
                .expect("Failed to create subscriber");

            let received_msg = subscriber
                .recv_timeout(Duration::from_secs(2))
                .expect("Failed to receive message");

            assert_eq!(received_msg.data.len(), 16);
            let timestamp_bytes = &received_msg.data.contiguous()[0..8];
            let timestamp = u64::from_le_bytes(timestamp_bytes.try_into().unwrap());
            assert_eq!(timestamp, 42);
        }
    });

    publisher_handle.join().expect("Publisher thread panicked");
    subscriber_handle
        .join()
        .expect("Subscriber thread panicked");
}
