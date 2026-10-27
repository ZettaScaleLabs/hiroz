use std::{sync::Arc, time::Duration};

use hiroz::{
    Builder,
    context::{ZContext, ZContextBuilder},
    entity::EndpointKind,
    node::ZNode,
};
use hiroz_msgs::std_msgs::String as RosString;

mod common;
use common::TestRouter;

#[test]
fn queued_schema_response_cannot_succeed_after_discovery_deadline() {
    use std::{future::Future, task::Wake};

    use hiroz::{
        ServiceTypeInfo,
        dynamic::{
            FieldType, GetTypeDescription, GetTypeDescriptionResponse, MessageSchema,
            schema_to_wire_type_description,
        },
        msg::ZMessage,
    };
    use zenoh::Wait;

    struct ReplyWake(std::sync::mpsc::Sender<()>);
    impl Wake for ReplyWake {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    let router = TestRouter::new();
    let producer_context = routed_context(&router, 244);
    let producer = producer_context
        .create_node("queued_schema_producer")
        .without_parameters()
        .build()
        .unwrap();
    let (requests_tx, requests) = flume::unbounded();
    let _service = producer
        .create_service_impl::<GetTypeDescription>(
            "~get_type_description",
            Some(GetTypeDescription::service_type_info()),
        )
        .build_with_callback(move |query| {
            requests_tx.send(query).unwrap();
        })
        .unwrap();
    let _publisher = producer
        .create_pub::<RosString>("/queued_schema")
        .build()
        .unwrap();
    let observer_context = routed_context(&router, 244);
    let observer = observer_context
        .create_node("queued_schema_observer")
        .without_parameters()
        .build()
        .unwrap();
    let schema = MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()
        .unwrap();
    let response = GetTypeDescriptionResponse {
        successful: true,
        type_description: schema_to_wire_type_description(&schema).unwrap(),
        ..Default::default()
    };
    let reply = |query: zenoh::query::Query| {
        query
            .reply(query.key_expr().clone(), response.serialize())
            .wait()
            .unwrap();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        wait_for_publishers(&observer, "/queued_schema", 1).await;
        // Establish the actual route and verify this fixture returns valid CDR.
        let (warm, ()) = tokio::join!(
            observer.discover_topic_schema("/queued_schema", Duration::from_secs(2)),
            async { reply(requests.recv_async().await.unwrap()) },
        );
        assert_eq!(warm.unwrap().schema.type_name, "std_msgs/msg/String");

        let budget = Duration::from_millis(200);
        let started = std::time::Instant::now();
        let mut discovery = Box::pin(observer.discover_topic_schema("/queued_schema", budget));
        let query = tokio::select! {
            result = &mut discovery => panic!("discovery completed before response: {result:?}"),
            query = requests.recv_async() => query.unwrap(),
        };
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let waker = std::task::Waker::from(Arc::new(ReplyWake(wake_tx)));
        let mut context = std::task::Context::from_waker(&waker);
        assert!(discovery.as_mut().poll(&mut context).is_pending());
        while wake_rx.try_recv().is_ok() {}
        reply(query);
        // Block this current-thread runtime: its timer driver cannot supply this
        // wake. The actual Zenoh response must wake the waiting call first.
        wake_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("response did not wake discovery");
        std::thread::sleep(budget);
        let result = discovery.await;
        eprintln!(
            "queued schema result after {:?}, budget {budget:?}: {result:?}",
            started.elapsed()
        );
        let error = result.expect_err("expired discovery accepted a queued valid schema");
        assert!(
            error
                .to_string()
                .contains("type description service timed out"),
            "{error}"
        );
        assert!(requests.is_empty(), "discovery retried the request");

        let (reused, ()) = tokio::join!(
            observer.discover_topic_schema("/queued_schema", Duration::from_secs(2)),
            async { reply(requests.recv_async().await.unwrap()) },
        );
        assert_eq!(reused.unwrap().schema.type_name, "std_msgs/msg/String");
        assert!(requests.is_empty());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrepresentable_discovery_deadline_remains_unbounded() {
    use hiroz_msgs::example_interfaces::srv::AddTwoInts;

    let router = TestRouter::new();
    let context = routed_context(&router, 245);
    let producer = context
        .create_node("unbounded_schema_producer")
        .without_parameters()
        .with_type_description_service()
        .build()
        .unwrap();
    let observer_context = routed_context(&router, 245);
    let observer = observer_context
        .create_node("unbounded_schema_observer")
        .without_parameters()
        .build()
        .unwrap();
    let publisher = producer
        .create_pub::<RosString>("/unbounded_schema")
        .build()
        .unwrap();
    wait_for_publishers(&observer, "/unbounded_schema", 1).await;
    let schema = tokio::time::timeout(
        Duration::from_secs(3),
        observer.discover_topic_schema_including_subscribers("/unbounded_schema", Duration::MAX),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(schema.schema.type_name, "std_msgs/msg/String");
    drop(publisher);

    // An unlimited budget must also work when graph discovery initially waits.
    let (discovered, _publisher) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(3),
            observer.discover_topic_schema("/late_unbounded_schema", Duration::MAX)
        ),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            producer
                .create_pub::<RosString>("/late_unbounded_schema")
                .build()
                .unwrap()
        },
    );
    assert_eq!(
        discovered.unwrap().unwrap().schema.type_name,
        "std_msgs/msg/String"
    );
    let _service = producer
        .create_service::<AddTwoInts>("/unbounded_service")
        .build()
        .unwrap();
    let (request, response) = tokio::time::timeout(
        Duration::from_secs(3),
        observer.discover_service_schema(
            "/unbounded_service",
            "example_interfaces/msg/AddTwoIntsRequest",
            "example_interfaces/msg/AddTwoIntsResponse",
            Duration::MAX,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        request.type_name,
        "example_interfaces/msg/AddTwoIntsRequest"
    );
    assert_eq!(
        response.type_name,
        "example_interfaces/msg/AddTwoIntsResponse"
    );
}

fn routed_context(router: &TestRouter, domain: usize) -> ZContext {
    ZContextBuilder::default()
        .with_domain_id(domain)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap()
}

async fn wait_for_publishers(observer: &ZNode, topic: &str, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while observer.graph().count(EndpointKind::Publisher, topic) < expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("publisher graph did not converge");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_description_route_expires_at_discovery_deadline() {
    let router = TestRouter::new();
    let producer_context = routed_context(&router, 241);
    let producer = producer_context
        .create_node("absent_schema_producer")
        .without_parameters()
        .build()
        .unwrap();
    let _publisher = producer
        .create_pub::<RosString>("/absent_schema")
        .build()
        .unwrap();
    let observer_context = routed_context(&router, 241);
    let observer = observer_context
        .create_node("absent_schema_observer")
        .without_parameters()
        .build()
        .unwrap();
    wait_for_publishers(&observer, "/absent_schema", 1).await;

    let budget = Duration::from_millis(250);
    let started = tokio::time::Instant::now();
    let error = tokio::time::timeout(
        budget + Duration::from_secs(1),
        observer.discover_topic_schema("/absent_schema", budget),
    )
    .await
    .expect("discovery exceeded its bound")
    .unwrap_err();
    assert!(
        started.elapsed() >= budget,
        "failed before route deadline: {error}"
    );
    assert!(
        error
            .to_string()
            .contains("type description service timed out"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_first_schema_candidate_does_not_block_healthy_candidate() {
    let router = TestRouter::new();
    let producer_context = routed_context(&router, 242);
    // Candidate order is deterministic by node name. The first publisher has
    // no type-description service; its route wait must not consume the healthy
    // second publisher's entire discovery budget.
    let unavailable = producer_context
        .create_node("aaa_unavailable_schema_producer")
        .without_parameters()
        .build()
        .unwrap();
    let healthy = producer_context
        .create_node("zzz_healthy_schema_producer")
        .without_parameters()
        .with_type_description_service()
        .build()
        .unwrap();
    let _unavailable_publisher = unavailable
        .create_pub::<RosString>("/multiple_schema_sources")
        .build()
        .unwrap();
    let _healthy_publisher = healthy
        .create_pub::<RosString>("/multiple_schema_sources")
        .build()
        .unwrap();
    let observer_context = routed_context(&router, 242);
    let observer = observer_context
        .create_node("multiple_schema_observer")
        .without_parameters()
        .build()
        .unwrap();
    wait_for_publishers(&observer, "/multiple_schema_sources", 2).await;

    let schema = observer
        .discover_topic_schema("/multiple_schema_sources", Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(schema.schema.type_name, "std_msgs/msg/String");
    assert_eq!(schema.schema.fields.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_wait_and_description_route_share_one_deadline() {
    let router = TestRouter::new();
    let producer_context = routed_context(&router, 243);
    let producer = producer_context
        .create_node("shared_deadline_producer")
        .without_parameters()
        .build()
        .unwrap();
    let observer_context = routed_context(&router, 243);
    let observer = observer_context
        .create_node("shared_deadline_observer")
        .without_parameters()
        .build()
        .unwrap();

    let budget = Duration::from_millis(600);
    let started = tokio::time::Instant::now();
    let discovery = observer.discover_topic_schema("/shared_deadline", budget);
    let late_publisher = async {
        // Leave less than half the total budget for the subsequent route wait.
        tokio::time::sleep(Duration::from_millis(400)).await;
        producer
            .create_pub::<RosString>("/shared_deadline")
            .build()
            .unwrap()
    };
    let (error, _publisher) = tokio::join!(discovery, late_publisher);
    let error = error.unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        error
            .to_string()
            .contains("type description service timed out"),
        "{error}"
    );
    assert!(elapsed >= budget, "failed before shared deadline: {error}");
    assert!(
        elapsed < Duration::from_millis(900),
        "graph and query received separate budgets: {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_waits_for_type_description_route_within_original_deadline() {
    zenoh::init_log_from_env_or("warn");
    let router = TestRouter::new();
    let context = ZContextBuilder::default()
        .with_domain_id(240)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap();
    let producer = context
        .create_node("delayed_schema_producer")
        .without_parameters()
        .build()
        .unwrap();
    let _publisher = producer
        .create_pub::<RosString>("/delayed_schema")
        .build()
        .unwrap();
    let observer_context = ZContextBuilder::default()
        .with_domain_id(240)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap();
    let observer = Arc::new(
        observer_context
            .create_node("delayed_schema_observer")
            .without_parameters()
            .build()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while observer
            .graph()
            .count(hiroz::entity::EndpointKind::Publisher, "/delayed_schema")
            == 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let discover = {
        let observer = observer.clone();
        tokio::spawn(async move {
            observer
                .discover_topic_schema("/delayed_schema", Duration::from_secs(2))
                .await
        })
    };
    // Model independent publisher-graph and service-route propagation. The
    // publisher is visible before its description endpoint becomes reachable.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let service = hiroz::dynamic::TypeDescriptionService::new_with_node(
        producer.session().clone(),
        producer.node_entity().clone(),
        producer.counter(),
        producer.clock(),
        hiroz_protocol::KeyExprFormat::default(),
    )
    .unwrap();
    let schema = hiroz::dynamic::MessageSchema::builder("std_msgs/msg/String")
        .field("data", hiroz::dynamic::FieldType::String)
        .build()
        .unwrap();
    service.register_schema(schema).unwrap();
    let discovered = discover.await.unwrap().unwrap();
    assert_eq!(discovered.schema.type_name, "std_msgs/msg/String");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_topic_schema_discovery_through_router() {
    zenoh::init_log_from_env_or("warn");
    let router = TestRouter::new();
    let producer_context = ZContextBuilder::default()
        .with_domain_id(239)
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .unwrap();
    let producer = producer_context
        .create_node("concurrent_schema_producer")
        .without_parameters()
        .with_type_description_service()
        .build()
        .unwrap();
    let _publisher = producer
        .create_pub::<RosString>("/concurrent_schema")
        .build()
        .unwrap();

    for round in 0..10 {
        let context = ZContextBuilder::default()
            .with_domain_id(239)
            .disable_multicast_scouting()
            .with_connect_endpoints([router.endpoint()])
            .with_mode("client")
            .build()
            .unwrap();
        let node = Arc::new(
            context
                .create_node(&format!("concurrent_schema_observer_{round}"))
                .without_parameters()
                .build()
                .unwrap(),
        );
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let node = node.clone();
            let barrier = barrier.clone();
            calls.spawn(async move {
                barrier.wait().await;
                node.discover_topic_schema("/concurrent_schema", Duration::from_secs(3))
                    .await
            });
        }
        let mut failures = Vec::new();
        while let Some(result) = calls.join_next().await {
            match result.unwrap() {
                Ok(discovered) => {
                    assert_eq!(discovered.schema.type_name, "std_msgs/msg/String");
                    assert_eq!(discovered.schema.fields.len(), 1);
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        assert!(failures.is_empty(), "round {round}: {failures:#?}");
    }
}
