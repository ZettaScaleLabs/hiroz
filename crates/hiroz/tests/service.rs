use std::{thread, time::Duration};

use hiroz::{
    Builder, MessageTypeInfo, ServiceTypeInfo, context::ZContextBuilder, entity::TypeHash,
    msg::ZService,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

// Simple test service request
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
struct AddTwoIntsRequest {
    a: i64,
    b: i64,
}

impl MessageTypeInfo for AddTwoIntsRequest {
    fn type_name() -> &'static str {
        "test_msgs::srv::dds_::AddTwoInts_Request_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl hiroz::WithTypeInfo for AddTwoIntsRequest {}

impl hiroz::msg::ZMessage for AddTwoIntsRequest {
    type Serdes = hiroz::msg::SerdeCdrSerdes<AddTwoIntsRequest>;
}

// Simple test service response
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
struct AddTwoIntsResponse {
    sum: i64,
}

impl MessageTypeInfo for AddTwoIntsResponse {
    fn type_name() -> &'static str {
        "test_msgs::srv::dds_::AddTwoInts_Response_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl hiroz::WithTypeInfo for AddTwoIntsResponse {}

impl hiroz::msg::ZMessage for AddTwoIntsResponse {
    type Serdes = hiroz::msg::SerdeCdrSerdes<AddTwoIntsResponse>;
}

// Service type definition
struct AddTwoInts;

impl ServiceTypeInfo for AddTwoInts {
    fn service_type_info() -> hiroz::entity::TypeInfo {
        hiroz::entity::TypeInfo::new("test_msgs::srv::dds_::AddTwoInts_", TypeHash::zero())
    }
}

impl ZService for AddTwoInts {
    type Request = AddTwoIntsRequest;
    type Response = AddTwoIntsResponse;
}

#[test]
fn test_basic_service_request_response() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .expect("Failed to create context");

    let server_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("test_server")
                .build()
                .expect("Failed to create node");

            let mut server = node
                .create_service::<AddTwoInts>("add_two_ints")
                .build()
                .expect("Failed to create server");

            // Wait for request
            let request = server.take_request().expect("Failed to take request");
            assert_eq!(request.message().a, 10);
            assert_eq!(request.message().b, 32);

            let response = AddTwoIntsResponse {
                sum: request.message().a + request.message().b,
            };
            request
                .reply_blocking(&response)
                .expect("Failed to send response");
        }
    });

    let client_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("test_client")
                .build()
                .expect("Failed to create node");

            let client = node
                .create_client::<AddTwoInts>("add_two_ints")
                .build()
                .expect("Failed to create client");

            // Give server time to start
            thread::sleep(Duration::from_millis(100));

            let request = AddTwoIntsRequest { a: 10, b: 32 };

            let response = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async {
                    client
                        .call_with_timeout(&request, Duration::from_secs(2))
                        .await
                })
                .expect("Failed to receive response");

            assert_eq!(response.sum, 42);
        }
    });

    server_handle.join().expect("Server thread panicked");
    client_handle.join().expect("Client thread panicked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_service_request_response() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .expect("Failed to create context");

    let server_ctx = ctx.clone();
    let server_handle = tokio::spawn(async move {
        let node = server_ctx
            .create_node("async_server")
            .build()
            .expect("Failed to create node");

        let mut server = node
            .create_service::<AddTwoInts>("async_add")
            .build()
            .expect("Failed to create server");

        // Wait for request asynchronously
        let request = server
            .async_take_request()
            .await
            .expect("Failed to take request");
        let response = AddTwoIntsResponse {
            sum: request.message().a + request.message().b,
        };

        request
            .reply(&response)
            .await
            .expect("Failed to send response");
    });

    let client_ctx = ctx.clone();
    let client_handle = tokio::spawn(async move {
        let node = client_ctx
            .create_node("async_client")
            .build()
            .expect("Failed to create node");

        let client = node
            .create_client::<AddTwoInts>("async_add")
            .build()
            .expect("Failed to create client");

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let request = AddTwoIntsRequest { a: 100, b: 23 };
        let response = client
            .call(&request)
            .await
            .expect("Failed to receive response");

        assert_eq!(response.sum, 123);
    });

    let (server_result, client_result) = tokio::join!(server_handle, client_handle);
    server_result.expect("Server task panicked");
    client_result.expect("Client task panicked");
}

#[test]
fn test_multiple_service_requests() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .expect("Failed to create context");

    let server_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("multi_server")
                .build()
                .expect("Failed to create node");

            let mut server = node
                .create_service::<AddTwoInts>("multi_add")
                .build()
                .expect("Failed to create server");

            // Handle 3 requests
            for expected_a in [1, 2, 3] {
                let request = server.take_request().expect("Failed to take request");
                assert_eq!(request.message().a, expected_a);
                assert_eq!(request.message().b, 10);

                let response = AddTwoIntsResponse {
                    sum: request.message().a + request.message().b,
                };
                request
                    .reply_blocking(&response)
                    .expect("Failed to send response");
            }
        }
    });

    let client_handle = thread::spawn({
        let ctx = ctx.clone();
        move || {
            let node = ctx
                .create_node("multi_client")
                .build()
                .expect("Failed to create node");

            let client = node
                .create_client::<AddTwoInts>("multi_add")
                .build()
                .expect("Failed to create client");

            // Give server time to start
            thread::sleep(Duration::from_millis(100));

            let rt = tokio::runtime::Runtime::new().unwrap();

            // Send 3 requests sequentially
            for a in [1, 2, 3] {
                let request = AddTwoIntsRequest { a, b: 10 };

                let response = rt
                    .block_on(async {
                        client
                            .call_with_timeout(&request, Duration::from_secs(2))
                            .await
                    })
                    .expect("Failed to receive response");

                assert_eq!(response.sum, a + 10);
            }
        }
    });

    server_handle.join().expect("Server thread panicked");
    client_handle.join().expect("Client thread panicked");
}

#[test]
fn test_try_take_request_non_blocking() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .expect("Failed to create context");

    let node = ctx.create_node("try_take_server").build().unwrap();
    let mut server = node
        .create_service::<AddTwoInts>("try_add")
        .build()
        .unwrap();

    // No request yet — must return None immediately.
    let result = server.try_take_request().expect("try_take_request failed");
    assert!(result.is_none(), "expected None when no request pending");

    // Fire a client request in the background.
    let client_node = ctx.create_node("try_take_client").build().unwrap();
    let client = client_node
        .create_client::<AddTwoInts>("try_add")
        .build()
        .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.spawn(async move {
        let _ = client
            .call_with_timeout(&AddTwoIntsRequest { a: 3, b: 4 }, Duration::from_secs(2))
            .await;
    });

    // Poll until the request arrives (up to 2s), then reply via into_parts().
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let request = loop {
        if let Some(req) = server.try_take_request().expect("try_take failed") {
            break req;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for request"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let (msg, reply) = request.into_parts();
    assert_eq!(msg.a, 3);
    assert_eq!(msg.b, 4);
    reply
        .reply_blocking(&AddTwoIntsResponse { sum: msg.a + msg.b })
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_call_with_timeout_expires() {
    let ctx = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("connect/endpoints", json!([]))
        .build()
        .unwrap();

    let node = ctx.create_node("timeout_client").build().unwrap();
    let client = node
        .create_client::<AddTwoInts>("nonexistent_service")
        .build()
        .unwrap();

    let result = client
        .call_with_timeout(
            &AddTwoIntsRequest { a: 1, b: 2 },
            Duration::from_millis(200),
        )
        .await;
    assert!(result.is_err(), "expected error when no server is present");
}

#[test]
fn test_request_id_hash_and_eq_ignore_timestamp() {
    use std::collections::HashMap;

    use hiroz::service::RequestId;

    let id1 = RequestId {
        sequence_number: 7,
        writer_guid: [1u8; 16],
        source_timestamp: 100,
    };
    let id2 = RequestId {
        sequence_number: 7,
        writer_guid: [1u8; 16],
        source_timestamp: 999,
    };
    let id3 = RequestId {
        sequence_number: 8,
        writer_guid: [1u8; 16],
        source_timestamp: 100,
    };

    // Same (sn, guid) but different timestamp → equal.
    assert_eq!(id1, id2);
    // Different sn → not equal.
    assert_ne!(id1, id3);

    // HashMap uses (sn, guid) as key — different timestamps overwrite.
    let mut map: HashMap<RequestId, &str> = HashMap::new();
    map.insert(id1.clone(), "first");
    map.insert(id2.clone(), "second");
    assert_eq!(map.len(), 1);
    assert_eq!(map[&id2], "second");
    map.insert(id3, "third");
    assert_eq!(map.len(), 2);
}
