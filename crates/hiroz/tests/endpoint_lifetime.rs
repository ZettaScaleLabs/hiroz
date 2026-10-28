//! Endpoint ownership must outlast its creating node/context and end on Drop.
//! The opposite Zenoh endpoints below deliberately own only WeakSession, so they
//! cannot accidentally keep the Hiroz endpoint's session alive for this test.
use std::time::Duration;

use hiroz::{
    Builder,
    context::ZContextBuilder,
    msg::ZMessage,
    qos::{QosDurability, QosProfile},
};
use hiroz_msgs::{
    example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts},
    std_msgs::String as RosString,
};
use zenoh::Wait;

#[cfg(feature = "ffi")]
mod common;

fn context() -> hiroz::context::ZContext {
    ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("listen/endpoints", Vec::<String>::new())
        .with_json("scouting/gossip/enabled", false)
        .build()
        .unwrap()
}

fn qos(transient: bool) -> QosProfile {
    QosProfile {
        durability: if transient {
            QosDurability::TransientLocal
        } else {
            QosDurability::Volatile
        },
        ..QosProfile::default()
    }
}

fn wire_key(entity: &hiroz::entity::EndpointEntity) -> zenoh::key_expr::KeyExpr<'static> {
    (*hiroz_protocol::KeyExprFormat::RmwZenoh
        .topic_key_expr(entity)
        .unwrap())
    .clone()
}

fn service_key(node: &hiroz::node::ZNode) -> zenoh::key_expr::KeyExpr<'static> {
    use hiroz::{
        ServiceTypeInfo,
        entity::{EndpointEntity, EndpointKind},
    };
    wire_key(&EndpointEntity {
        id: 0,
        node: Some(node.node_entity().clone()),
        kind: EndpointKind::Service,
        topic: "/owned_service".into(),
        type_info: Some(AddTwoInts::service_type_info()),
        qos: Default::default(),
    })
}

#[test]
fn typed_publisher_survives_parents_until_endpoint_drop() {
    for transient in [false, true] {
        let context = context();
        let node = context
            .create_node("publisher_owner")
            .without_parameters()
            .build()
            .unwrap();
        let session = std::sync::Arc::downgrade(node.session());
        let publisher = node
            .create_pub::<RosString>("/owned_topic")
            .with_qos(qos(transient))
            .build()
            .unwrap();
        let subscriber = node
            .session()
            .declare_subscriber(wire_key(&publisher.entity))
            .wait()
            .unwrap();
        drop(node);
        drop(context);
        let message = RosString {
            data: "publisher after parent drop".into(),
        };
        publisher
            .publish(&message)
            .expect("live publisher lost its parent session");
        let sample = subscriber
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            RosString::deserialize(&sample.payload().to_bytes())
                .unwrap()
                .data,
            message.data
        );
        assert!(session.upgrade().is_some());
        drop(publisher);
        assert!(
            session.upgrade().is_none(),
            "publisher session guard leaked"
        );
    }
}

#[test]
fn typed_subscriber_survives_parents_until_endpoint_drop() {
    for transient in [false, true] {
        let context = context();
        let node = context
            .create_node("subscriber_owner")
            .without_parameters()
            .build()
            .unwrap();
        let session = std::sync::Arc::downgrade(node.session());
        let subscriber = node
            .create_sub::<RosString>("/owned_topic")
            .with_qos(qos(transient))
            .build()
            .unwrap();
        let publisher = node
            .session()
            .declare_publisher(wire_key(&subscriber.entity))
            .wait()
            .unwrap();
        drop(node);
        drop(context);
        let message = RosString {
            data: "subscriber after parent drop".into(),
        };
        publisher
            .put(message.serialize())
            .wait()
            .expect("live subscriber lost its parent session");
        assert_eq!(
            subscriber
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .data,
            message.data
        );
        assert!(session.upgrade().is_some());
        drop(subscriber);
        assert!(
            session.upgrade().is_none(),
            "subscriber session guard leaked"
        );
    }
}

#[test]
fn explicit_context_shutdown_still_closes_retained_endpoints() {
    let context = context();
    let node = context
        .create_node("explicit_shutdown")
        .without_parameters()
        .build()
        .unwrap();
    let publisher = node
        .create_pub::<RosString>("/shutdown_topic")
        .build()
        .unwrap();
    context.shutdown().unwrap();
    assert!(
        publisher
            .publish(&RosString {
                data: "closed".into()
            })
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_service_server_survives_parents_until_endpoint_drop() {
    let context = context();
    let node = context
        .create_node("service_owner")
        .without_parameters()
        .build()
        .unwrap();
    let session = std::sync::Arc::downgrade(node.session());
    let server = node
        .create_service::<AddTwoInts>("/owned_service")
        .build_with_callback(|query| {
            let request =
                AddTwoIntsRequest::deserialize(&query.payload().unwrap().to_bytes()).unwrap();
            query
                .reply(
                    query.key_expr().clone(),
                    AddTwoIntsResponse {
                        sum: request.a + request.b,
                    }
                    .serialize(),
                )
                .wait()
                .unwrap();
        })
        .unwrap();
    let client = node
        .session()
        .declare_querier(service_key(&node))
        .wait()
        .unwrap();
    drop(node);
    drop(context);
    let replies = client
        .get()
        .payload(AddTwoIntsRequest { a: 19, b: 23 }.serialize())
        .wait()
        .expect("live server lost its parent session");
    let sample = replies
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap()
        .into_result()
        .unwrap();
    let response = AddTwoIntsResponse::deserialize(&sample.payload().to_bytes()).unwrap();
    assert_eq!(response.sum, 42);
    assert!(session.upgrade().is_some());
    drop(server);
    assert!(session.upgrade().is_none(), "server session guard leaked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_service_client_survives_parents_until_endpoint_drop() {
    let context = context();
    let node = context
        .create_node("client_owner")
        .without_parameters()
        .build()
        .unwrap();
    let session = std::sync::Arc::downgrade(node.session());
    let _server = node
        .session()
        .declare_queryable(service_key(&node))
        .complete(true)
        .callback(|query| {
            let request =
                AddTwoIntsRequest::deserialize(&query.payload().unwrap().to_bytes()).unwrap();
            query
                .reply(
                    query.key_expr().clone(),
                    AddTwoIntsResponse {
                        sum: request.a + request.b,
                    }
                    .serialize(),
                )
                .wait()
                .unwrap();
        })
        .wait()
        .unwrap();
    let client = node
        .create_client::<AddTwoInts>("/owned_service")
        .build()
        .unwrap();
    drop(node);
    drop(context);
    let response = client
        .call_with_timeout(&AddTwoIntsRequest { a: 19, b: 23 }, Duration::from_secs(1))
        .await
        .expect("live client lost its parent session");
    assert_eq!(response.sum, 42);
    assert!(session.upgrade().is_some());
    drop(client);
    assert!(session.upgrade().is_none(), "client session guard leaked");
}

#[cfg(feature = "ffi")]
mod raw {
    use std::sync::mpsc;

    use hiroz::ros_msg::MessageTypeInfo;

    use super::*;

    #[test]
    fn raw_publisher_survives_parents_until_endpoint_drop() {
        for transient in [false, true] {
            let context = context();
            let node = context
                .create_node("raw_publisher_owner")
                .without_parameters()
                .build()
                .unwrap();
            let session = std::sync::Arc::downgrade(node.session());
            let publisher = node
                .create_raw_publisher_with_qos(
                    "/owned_topic",
                    RosString::type_name(),
                    &RosString::type_hash().to_rihs_string(),
                    Some(qos(transient)),
                )
                .unwrap();
            let subscriber = node
                .session()
                .declare_subscriber(publisher.inner.key_expr().clone())
                .wait()
                .unwrap();
            drop(node);
            drop(context);
            let bytes = RosString {
                data: "raw publisher after parent drop".into(),
            }
            .serialize();
            publisher
                .publish_bytes(&bytes)
                .expect("live raw publisher lost its parent session");
            let sample = subscriber
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(sample.payload().to_bytes().as_ref(), bytes.as_slice());
            assert!(session.upgrade().is_some());
            drop(publisher);
            assert!(
                session.upgrade().is_none(),
                "raw publisher session guard leaked"
            );
        }
    }

    #[test]
    fn raw_subscriber_survives_parents_until_endpoint_drop() {
        for transient in [false, true] {
            let context = context();
            let node = context
                .create_node("raw_subscriber_owner")
                .without_parameters()
                .build()
                .unwrap();
            let session = std::sync::Arc::downgrade(node.session());
            let (tx, rx) = mpsc::channel();
            let subscriber = node
                .create_raw_subscriber_with_qos(
                    "/owned_topic",
                    RosString::type_name(),
                    &RosString::type_hash().to_rihs_string(),
                    move |bytes| {
                        let _ = tx.send(bytes.to_vec());
                    },
                    Some(qos(transient)),
                )
                .unwrap();
            // Use the public typed endpoint identity only to construct its wire key.
            let identity = node
                .create_pub::<RosString>("/owned_topic")
                .build()
                .unwrap();
            let publisher = node
                .session()
                .declare_publisher(wire_key(&identity.entity))
                .wait()
                .unwrap();
            drop(identity);
            drop(node);
            drop(context);
            let bytes = RosString {
                data: "raw subscriber after parent drop".into(),
            }
            .serialize();
            publisher
                .put(bytes.clone())
                .wait()
                .expect("live raw subscriber lost its parent session");
            assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), bytes);
            assert!(session.upgrade().is_some());
            drop(subscriber);
            assert!(
                session.upgrade().is_none(),
                "raw subscriber session guard leaked"
            );
        }
    }

    #[test]
    fn queued_ffi_service_survives_parents_until_endpoint_drop() {
        use std::{ffi::CString, ptr};

        use hiroz::ffi::{context::*, node::*, serialize::hiroz_free_bytes, service::*};
        let router = common::TestRouter::new();
        let endpoint = CString::new(router.endpoint()).unwrap();
        let endpoints = [endpoint.as_ptr()];
        let config = CContextConfig {
            domain_id: 0,
            config_file: ptr::null(),
            connect_endpoints: endpoints.as_ptr(),
            connect_endpoints_count: 1,
            mode: c"client".as_ptr(),
            disable_multicast_scouting: true,
            connect_to_local_zenohd: false,
            json_config: c"{\"scouting/gossip/enabled\":false}".as_ptr(),
            remap_rules: ptr::null(),
            remap_rules_count: 0,
            enable_logging: false,
            namespace: ptr::null(),
            shm_pool_bytes: 0,
        };
        const HASH: &std::ffi::CStr =
            c"RIHS01_0000000000000000000000000000000000000000000000000000000000000000";
        unsafe {
            let server_context = hiroz_context_create_with_config(&config);
            assert!(!server_context.is_null());
            let server_node =
                hiroz_node_create(server_context, c"ffi_server_owner".as_ptr(), ptr::null());
            assert!(!server_node.is_null());
            let server = hiroz_service_server_create_queued(
                server_node,
                c"/owned_service".as_ptr(),
                c"test::srv::dds_::Lifetime_".as_ptr(),
                HASH.as_ptr(),
                ptr::null(),
            );
            assert!(!server.is_null());
            let client_context = hiroz_context_create_with_config(&config);
            assert!(!client_context.is_null());
            let client_node =
                hiroz_node_create(client_context, c"ffi_client_peer".as_ptr(), ptr::null());
            assert!(!client_node.is_null());
            let client = hiroz_service_client_create(
                client_node,
                c"/owned_service".as_ptr(),
                c"test::srv::dds_::Lifetime_".as_ptr(),
                HASH.as_ptr(),
                c"test::srv::dds_::Lifetime_".as_ptr(),
                HASH.as_ptr(),
            );
            assert!(!client.is_null());
            assert_eq!(hiroz_service_client_wait_for_service(client, 2000), 0);
            assert_eq!(hiroz_node_destroy(server_node), 0);
            assert_eq!(hiroz_context_destroy(server_context), 0);

            let mut call = ptr::null_mut();
            assert_eq!(
                hiroz_service_client_start_call(client, b"request".as_ptr(), 7, 2000, &mut call),
                0
            );
            let mut request = ptr::null_mut();
            let mut data = ptr::null_mut();
            let mut len = 0;
            let mut id = std::mem::zeroed();
            let taken = hiroz_service_server_take_request(
                server,
                1000,
                &mut request,
                &mut data,
                &mut len,
                &mut id,
            );
            if taken == 0 {
                assert_eq!(std::slice::from_raw_parts(data, len), b"request");
                hiroz_free_bytes(data, len);
                assert_eq!(
                    hiroz_service_request_reply(request, b"response".as_ptr(), 8),
                    0
                );
                hiroz_service_request_destroy(request);
                assert_eq!(hiroz_service_call_poll(call, 1000, &mut data, &mut len), 0);
                assert_eq!(std::slice::from_raw_parts(data, len), b"response");
                hiroz_free_bytes(data, len);
            }
            hiroz_service_call_destroy(call);
            hiroz_service_server_destroy(server);
            hiroz_service_client_destroy(client);
            hiroz_node_destroy(client_node);
            hiroz_context_destroy(client_context);
            assert_eq!(taken, 0, "live FFI service lost its parent session");
        }
    }
}
