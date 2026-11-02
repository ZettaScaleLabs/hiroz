//! Reusable service clients must observe the query route before dispatch.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hiroz::{Builder, context::ZContextBuilder, entity::EndpointKind, msg::ZMessage};
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts};
use zenoh::Wait;

mod common;
use common::TestRouter;

fn context(router: &TestRouter) -> hiroz::context::ZContext {
    ZContextBuilder::default()
        .with_mode("client")
        .disable_multicast_scouting()
        .with_json("scouting/gossip/enabled", false)
        .with_connect_endpoints([router.endpoint()])
        .build()
        .unwrap()
}

fn serve(
    node: &hiroz::node::ZNode,
    name: &str,
    requests: Arc<AtomicUsize>,
) -> hiroz::service::ZServer<AddTwoInts, ()> {
    node.create_service::<AddTwoInts>(name)
        .build_with_callback(move |query| {
            requests.fetch_add(1, Ordering::SeqCst);
            let payload = query.payload().unwrap().to_bytes();
            let request = AddTwoIntsRequest::deserialize(&payload).unwrap();
            let response = AddTwoIntsResponse {
                sum: request.a + request.b,
            };
            query
                .reply(query.key_expr().clone(), response.serialize())
                .wait()
                .unwrap();
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_call_waits_for_delayed_route_and_sends_once() {
    let router = TestRouter::new();
    let server_context = context(&router);
    let server_node = server_context
        .create_node("delayed_route_server")
        .without_parameters()
        .build()
        .unwrap();
    let client_context = context(&router);
    let node = client_context
        .create_node("delayed_route_client")
        .without_parameters()
        .build()
        .unwrap();
    let client = node
        .create_client::<AddTwoInts>("/delayed_route")
        .build()
        .unwrap();
    let request = AddTwoIntsRequest { a: 19, b: 23 };
    let mut call = Box::pin(client.call_with_timeout(&request, Duration::from_secs(2)));
    tokio::select! {
        result = &mut call => panic!("call finished before the route existed: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(100)) => {},
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let _server = serve(&server_node, "/delayed_route", requests.clone());
    let response = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.sum, 42);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "readiness must not retry requests"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_route_uses_original_call_timeout() {
    let router = TestRouter::new();
    let client_context = context(&router);
    let node = client_context
        .create_node("absent_route_client")
        .without_parameters()
        .build()
        .unwrap();
    let client = node
        .create_client::<AddTwoInts>("/absent_route")
        .build()
        .unwrap();
    let budget = Duration::from_millis(250);
    let started = tokio::time::Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        client.call_with_timeout(&AddTwoIntsRequest { a: 1, b: 2 }, budget),
    )
    .await
    .expect("route wait exceeded caller timeout")
    .unwrap_err();
    assert!(
        hiroz::error::is_timeout(error.as_ref()),
        "wrong error: {error}"
    );
    assert!(
        started.elapsed() >= budget,
        "route absence failed prematurely"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_and_response_wait_share_one_timeout() {
    let router = TestRouter::new();
    let server_context = context(&router);
    let server_node = server_context
        .create_node("shared_route_budget_server")
        .without_parameters()
        .build()
        .unwrap();
    let client_context = context(&router);
    let node = client_context
        .create_node("shared_route_budget_client")
        .without_parameters()
        .build()
        .unwrap();
    let client = node
        .create_client::<AddTwoInts>("/shared_route_budget")
        .build()
        .unwrap();
    let request = AddTwoIntsRequest { a: 1, b: 2 };
    let budget = Duration::from_millis(400);
    let started = tokio::time::Instant::now();
    let mut call = Box::pin(client.call_with_timeout(&request, budget));
    tokio::select! {
        result = &mut call => panic!("call finished before its route existed: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(250)) => {},
    }
    let (requests, received) = flume::unbounded();
    let _server = server_node
        .create_service::<AddTwoInts>("/shared_route_budget")
        .build_with_callback(move |query| {
            requests.send(query).unwrap();
        })
        .unwrap();
    let held_request = tokio::select! {
        result = &mut call => panic!("request did not reach the ready server: {result:?}"),
        request = received.recv_async() => request.unwrap(),
    };
    let error = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        hiroz::error::is_timeout(error.as_ref()),
        "wrong error: {error}"
    );
    assert!(started.elapsed() >= budget);
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "readiness reset the response budget"
    );
    assert!(received.try_recv().is_err(), "request was retried");
    drop(held_request);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_fresh_queriers_dispatch_after_graph_visibility() {
    zenoh::init_log_from_env_or("warn");
    let router = TestRouter::new();
    let server_context = context(&router);
    let server_node = server_context
        .create_node("querier_churn_server")
        .without_parameters()
        .build()
        .unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let _server = serve(&server_node, "/querier_churn", requests.clone());
    let client_context = context(&router);
    let node = Arc::new(
        client_context
            .create_node("querier_churn_client")
            .without_parameters()
            .build()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while node
            .graph()
            .count_by_service(EndpointKind::Service, "/querier_churn")
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("server graph announcement missing");
    let mut workers = tokio::task::JoinSet::new();
    for worker in 0..4 {
        let node = node.clone();
        workers.spawn(async move {
            for sequence in 0..32 {
                let client = node
                    .create_client::<AddTwoInts>("/querier_churn")
                    .build()
                    .unwrap();
                let request = AddTwoIntsRequest {
                    a: worker,
                    b: sequence,
                };
                let response = client
                    .call_with_timeout(&request, Duration::from_secs(2))
                    .await
                    .unwrap_or_else(|error| panic!("worker={worker} sequence={sequence}: {error}"));
                assert_eq!(response.sum, worker + sequence);
            }
        });
    }
    while let Some(result) = workers.join_next().await {
        result.unwrap();
    }
    assert_eq!(requests.load(Ordering::SeqCst), 128);
}
