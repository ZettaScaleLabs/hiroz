//! Discovery must expose responder failures without retrying or calling them timeouts.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use hiroz::{Builder, ServiceTypeInfo, context::ZContextBuilder, dynamic::GetTypeDescription};
use hiroz_msgs::std_msgs::String as RosString;
use zenoh::Wait;

mod common;
use common::TestRouter;

#[derive(Clone, Copy)]
enum Response {
    Malformed,
    None,
}

async fn assert_responder_failure(response: Response) {
    let router = TestRouter::new();
    let producer_context = ZContextBuilder::default()
        .with_domain_id(241)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap();
    let producer = producer_context
        .create_node("invalid_schema_producer")
        .without_parameters()
        .build()
        .unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let reply_failed = Arc::new(AtomicBool::new(false));
    let seen = requests.clone();
    let failed = reply_failed.clone();
    // Use the public service builder to advertise the real GetTypeDescription
    // endpoint. Its raw query callback lets a remote responder send invalid CDR
    // or finish a query without a reply, without stubbing any discovery code.
    let _service = producer
        .create_service_impl::<GetTypeDescription>(
            "~get_type_description",
            Some(GetTypeDescription::service_type_info()),
        )
        .build_with_callback(move |query| {
            seen.fetch_add(1, Ordering::SeqCst);
            if matches!(response, Response::Malformed) {
                // Invalid encapsulation: this binding requires CDR little-endian.
                if query
                    .reply(query.key_expr().clone(), vec![0, 2, 0, 0])
                    .wait()
                    .is_err()
                {
                    failed.store(true, Ordering::SeqCst);
                }
            }
            // Dropping the query sends QueryFinal, including the no-reply case.
        })
        .unwrap();
    let _publisher = producer
        .create_pub::<RosString>("/invalid_schema")
        .build()
        .unwrap();
    let observer_context = ZContextBuilder::default()
        .with_domain_id(241)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap();
    let observer = observer_context
        .create_node("invalid_schema_observer")
        .without_parameters()
        .build()
        .unwrap();

    let budget = Duration::from_secs(3);
    let start = Instant::now();
    let error = observer
        .discover_topic_schema("/invalid_schema", budget)
        .await
        .expect_err("invalid responder unexpectedly produced a schema");
    let elapsed = start.elapsed();
    let diagnostic = error.to_string();
    assert!(
        elapsed < budget,
        "responder failure waited until deadline: {elapsed:?}"
    );
    assert!(
        diagnostic.starts_with("type description service failed for node"),
        "responder error was misclassified: {diagnostic}"
    );
    assert!(
        diagnostic.contains("invalid_schema_producer")
            && diagnostic.contains("/invalid_schema_producer/get_type_description"),
        "responder identity was lost: {diagnostic}"
    );
    assert!(!diagnostic.contains("timed out"), "{diagnostic}");
    let expected_reason = match response {
        Response::Malformed => "Expected CDR_LE encapsulation",
        Response::None => "service query completed without a response",
    };
    assert!(
        diagnostic.contains(expected_reason),
        "responder failure cause was lost: {diagnostic}"
    );
    assert!(!reply_failed.load(Ordering::SeqCst), "fixture reply failed");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "discovery retried a responder failure"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_type_description_reply_is_not_retried_or_reported_as_timeout() {
    assert_responder_failure(Response::Malformed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_description_query_without_reply_is_not_retried_or_reported_as_timeout() {
    assert_responder_failure(Response::None).await;
}
