//! Integration tests for ROS 2 parameter support.
//!
//! Basic API tests run without any feature flags.
//! Service client tests (using hiroz-msgs rcl_interfaces types) require `ros-msgs`.

mod common;

use std::{sync::Arc, thread, time::Duration};

use common::*;
use hiroz::{
    Builder,
    parameter::{
        Parameter, ParameterClient, ParameterDescriptor, ParameterTarget, ParameterType,
        ParameterValue, SetParametersResult,
        wire_types::{WireParameterEvent, parameter_event_type_info},
    },
};

// ── Local API tests (no hiroz-msgs required) ─────────────────────────────────

/// Test declare/get/set on a single node without going through services.
#[test]
fn test_parameter_local_api() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let node = ctx.create_node("param_test_node").build().expect("node");

    // Declare a parameter
    let desc = ParameterDescriptor::new("my_int", ParameterType::Integer);
    let initial = node
        .declare_parameter("my_int", ParameterValue::Integer(42), desc)
        .expect("declare");
    assert_eq!(initial, ParameterValue::Integer(42));

    // Get it back
    assert_eq!(
        node.get_parameter("my_int"),
        Some(ParameterValue::Integer(42))
    );

    // Set it
    node.set_parameter(Parameter::new("my_int", ParameterValue::Integer(100)))
        .expect("set should succeed");
    assert_eq!(
        node.get_parameter("my_int"),
        Some(ParameterValue::Integer(100))
    );

    // Cannot set wrong type
    let bad = node.set_parameter(Parameter::new("my_int", ParameterValue::Bool(true)));
    assert!(bad.is_err());

    // Undeclare
    node.undeclare_parameter("my_int").expect("undeclare");
    assert_eq!(node.get_parameter("my_int"), None);
}

/// Test that a validation callback can reject parameter changes.
#[test]
fn test_parameter_validation_callback() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let node = ctx.create_node("callback_test_node").build().expect("node");

    let desc = ParameterDescriptor::new("speed", ParameterType::Double);
    node.declare_parameter("speed", ParameterValue::Double(1.0), desc)
        .expect("declare");

    // Register callback that rejects values > 10.0
    node.on_set_parameters(|params| {
        for p in params {
            if let ParameterValue::Double(v) = &p.value
                && *v > 10.0
            {
                return SetParametersResult::failure(format!("speed {} exceeds maximum 10.0", v));
            }
        }
        SetParametersResult::success()
    });

    // Valid change
    node.set_parameter(Parameter::new("speed", ParameterValue::Double(5.0)))
        .expect("valid change");

    // Rejected by callback
    let bad = node
        .set_parameter(Parameter::new("speed", ParameterValue::Double(15.0)))
        .unwrap_err();
    assert!(bad.contains("maximum"));

    // Value unchanged after rejection
    assert_eq!(
        node.get_parameter("speed"),
        Some(ParameterValue::Double(5.0))
    );
}

/// Test read-only parameter enforcement.
#[test]
fn test_read_only_parameter() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let node = ctx.create_node("readonly_test_node").build().expect("node");

    let mut desc = ParameterDescriptor::new("fixed", ParameterType::String);
    desc.read_only = true;
    node.declare_parameter("fixed", ParameterValue::String("immutable".into()), desc)
        .expect("declare");

    let bad = node
        .set_parameter(Parameter::new(
            "fixed",
            ParameterValue::String("changed".into()),
        ))
        .unwrap_err();
    assert!(bad.contains("read-only"));

    // Value unchanged
    assert_eq!(
        node.get_parameter("fixed"),
        Some(ParameterValue::String("immutable".into()))
    );
}

/// Test parameter overrides applied at declaration time.
#[test]
fn test_parameter_overrides() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");

    let mut overrides = std::collections::HashMap::new();
    overrides.insert("count".to_string(), ParameterValue::Integer(99));

    let node = ctx
        .create_node("override_test_node")
        .with_parameter_overrides(overrides)
        .build()
        .expect("node");

    let desc = ParameterDescriptor::new("count", ParameterType::Integer);
    let initial = node
        .declare_parameter("count", ParameterValue::Integer(1), desc)
        .expect("declare");

    // Override wins over default
    assert_eq!(initial, ParameterValue::Integer(99));
    assert_eq!(
        node.get_parameter("count"),
        Some(ParameterValue::Integer(99))
    );
}

/// Test without_parameters() disables services.
#[test]
fn test_without_parameters() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx
        .create_node("no_params_node")
        .without_parameters()
        .build()
        .expect("node");

    assert!(!node.has_parameter_service());
    let result = node.declare_parameter(
        "x",
        ParameterValue::Integer(1),
        ParameterDescriptor::new("x", ParameterType::Integer),
    );
    assert!(result.is_err());
}

/// Test multiple parameter types.
#[test]
fn test_parameter_types() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let node = ctx.create_node("types_test_node").build().expect("node");

    let cases: Vec<(&str, ParameterType, ParameterValue)> = vec![
        ("b", ParameterType::Bool, ParameterValue::Bool(true)),
        ("i", ParameterType::Integer, ParameterValue::Integer(-10)),
        ("f", ParameterType::Double, ParameterValue::Double(2.5)),
        (
            "s",
            ParameterType::String,
            ParameterValue::String("hello".into()),
        ),
        (
            "ba",
            ParameterType::ByteArray,
            ParameterValue::ByteArray(vec![1, 2, 3]),
        ),
        (
            "ia",
            ParameterType::IntegerArray,
            ParameterValue::IntegerArray(vec![10, 20, 30]),
        ),
        (
            "fa",
            ParameterType::DoubleArray,
            ParameterValue::DoubleArray(vec![1.1, 2.2]),
        ),
        (
            "sa",
            ParameterType::StringArray,
            ParameterValue::StringArray(vec!["a".into(), "b".into()]),
        ),
    ];

    for (name, type_, val) in &cases {
        let desc = ParameterDescriptor::new(*name, *type_);
        let declared = node
            .declare_parameter(name, val.clone(), desc)
            .expect("declare");
        assert_eq!(&declared, val, "mismatch for {}", name);
        assert_eq!(node.get_parameter(name).as_ref(), Some(val));
    }
}

/// Test YAML parameter loading.
#[test]
fn test_yaml_parameter_loading() {
    use hiroz::parameter::yaml::load_parameter_string;

    let yaml = r#"
/**:
  ros__parameters:
    timeout: 5.0
    name: "robot"

/yaml_node:
  ros__parameters:
    max_speed: 1.5
"#;

    let params = load_parameter_string(yaml, "/yaml_node").unwrap();
    assert_eq!(params["timeout"], ParameterValue::Double(5.0));
    assert_eq!(params["name"], ParameterValue::String("robot".to_string()));
    assert_eq!(params["max_speed"], ParameterValue::Double(1.5));
    // Different node doesn't get /yaml_node params
    let other_params = load_parameter_string(yaml, "/other_node").unwrap();
    assert!(!other_params.contains_key("max_speed"));
    assert!(other_params.contains_key("timeout")); // wildcard still applies
}

/// Test the high-level parameter client, including get_types and atomic set.
#[test]
fn test_parameter_client_high_level_api() {
    let router = TestRouter::new();

    let endpoint = router.endpoint().to_string();
    thread::spawn(move || {
        let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
        let node = ctx
            .create_node("high_level_param_server")
            .build()
            .expect("node");

        for (name, value) in [("count", 7_i64), ("limit", 3_i64)] {
            let desc = ParameterDescriptor::new(name, ParameterType::Integer);
            node.declare_parameter(name, ParameterValue::Integer(value), desc)
                .expect("declare");
        }

        node.on_set_parameters(|params| {
            for param in params {
                if let ParameterValue::Integer(value) = param.value
                    && value > 10
                {
                    return SetParametersResult::failure(format!(
                        "{} exceeds maximum 10",
                        param.name
                    ));
                }
            }
            SetParametersResult::success()
        });

        thread::sleep(Duration::from_secs(30));
    });

    wait_for_ready(Duration::from_secs(2));

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let ctx = create_hiroz_context_with_router(&router).expect("ctx");
        let client_node = Arc::new(
            ctx.create_node("high_level_param_client")
                .build()
                .expect("node"),
        );
        let client = ParameterClient::new(
            client_node,
            ParameterTarget::from_fqn("/high_level_param_server").expect("target"),
        )
        .expect("client");

        tokio::time::sleep(Duration::from_millis(500)).await;

        assert_eq!(
            client.get(&["count"]).await.expect("get"),
            vec![ParameterValue::Integer(7)]
        );
        assert_eq!(
            client
                .get_types(&["count", "missing"])
                .await
                .expect("types"),
            vec![ParameterType::Integer, ParameterType::NotSet]
        );

        let listed = client.list(&[""], None).await.expect("list");
        assert!(listed.names.contains(&"count".to_string()));
        assert!(listed.names.contains(&"limit".to_string()));

        let described = client.describe(&["count"]).await.expect("describe");
        assert_eq!(described.len(), 1);
        assert_eq!(described[0].name, "count");
        assert_eq!(described[0].type_, ParameterType::Integer);

        let set_results = client
            .set(&[Parameter::new("count", ParameterValue::Integer(9))])
            .await
            .expect("set");
        assert_eq!(set_results, vec![SetParametersResult::success()]);
        assert_eq!(
            client.get(&["count"]).await.expect("get"),
            vec![ParameterValue::Integer(9)]
        );

        let atomic = client
            .set_atomically(&[
                Parameter::new("count", ParameterValue::Integer(11)),
                Parameter::new("limit", ParameterValue::Integer(4)),
            ])
            .await
            .expect("atomic set");
        assert!(!atomic.successful);
        assert!(atomic.reason.contains("maximum"));
        assert_eq!(
            client.get(&["count", "limit"]).await.expect("get"),
            vec![ParameterValue::Integer(9), ParameterValue::Integer(3)]
        );
    });
}

/// Test parameter events for declare, set, and undeclare operations.
#[test]
fn test_parameter_events_cover_lifecycle() {
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let server = ctx
        .create_node("param_event_server")
        .build()
        .expect("server");
    let observer = ctx
        .create_node("param_event_observer")
        .build()
        .expect("observer");

    let events = observer
        .create_sub_impl::<WireParameterEvent>(
            "/parameter_events",
            Some(parameter_event_type_info()),
        )
        .build()
        .expect("event subscriber");

    wait_for_ready(Duration::from_millis(500));

    server
        .declare_parameter(
            "count",
            ParameterValue::Integer(1),
            ParameterDescriptor::new("count", ParameterType::Integer),
        )
        .expect("declare");
    let declared = events
        .recv_timeout(Duration::from_secs(2))
        .expect("declare event");
    assert_eq!(declared.node, "/param_event_server");
    assert_eq!(declared.new_parameters.len(), 1);
    assert_eq!(declared.new_parameters[0].name, "count");
    assert!(declared.changed_parameters.is_empty());
    assert!(declared.deleted_parameters.is_empty());

    server
        .set_parameter(Parameter::new("count", ParameterValue::Integer(2)))
        .expect("set");
    let changed = events
        .recv_timeout(Duration::from_secs(2))
        .expect("change event");
    assert!(changed.new_parameters.is_empty());
    assert_eq!(changed.changed_parameters.len(), 1);
    assert_eq!(changed.changed_parameters[0].name, "count");
    assert!(changed.deleted_parameters.is_empty());

    server.undeclare_parameter("count").expect("undeclare");
    let deleted = events
        .recv_timeout(Duration::from_secs(2))
        .expect("delete event");
    assert!(deleted.new_parameters.is_empty());
    assert!(deleted.changed_parameters.is_empty());
    assert_eq!(deleted.deleted_parameters.len(), 1);
    assert_eq!(deleted.deleted_parameters[0].name, "count");
    assert_eq!(deleted.deleted_parameters[0].value.integer_value, 2);
}

/// Test that set_parameter publishes a ParameterEvent on /parameter_events.
///
/// Subscribes to the raw Zenoh key `rt/parameter_events` before triggering any
/// change, then sets a parameter and asserts the event arrives with the correct
/// node FQN and changed parameter name.
#[test]
fn test_parameter_event_published_on_set() {
    use std::{sync::mpsc, time::Duration};

    use hiroz::{
        msg::{SerdeCdrSerdes, ZDeserializer},
        parameter::wire_types::WireParameterEvent,
    };
    use zenoh::Wait;

    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("context");
    let node = ctx.create_node("event_test_node").build().expect("node");

    // Subscribe to /parameter_events via the raw session before any change.
    let (tx, rx) = mpsc::sync_channel::<WireParameterEvent>(8);
    // The publisher key is: {domain_id}/parameter_events/{type_name}/{type_hash}
    let _sub = node
        .session()
        .declare_subscriber("*/parameter_events/**")
        .callback(move |sample| {
            let bytes = sample.payload().to_bytes();
            if let Ok(event) = SerdeCdrSerdes::<WireParameterEvent>::deserialize(&bytes) {
                let _ = tx.send(event);
            }
        })
        .wait()
        .expect("declare subscriber");

    let desc = ParameterDescriptor::new("speed", ParameterType::Double);
    node.declare_parameter("speed", ParameterValue::Double(1.0), desc)
        .expect("declare");

    // Drain any event from declare (implementation may or may not publish one).
    let _ = rx.recv_timeout(Duration::from_millis(100));

    node.set_parameter(Parameter::new("speed", ParameterValue::Double(3.0)))
        .expect("set");

    let event = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("parameter event must arrive after set_parameter");

    assert!(
        event.node.contains("event_test_node"),
        "event.node should contain node name, got: {}",
        event.node
    );
    assert_eq!(event.changed_parameters.len(), 1);
    assert_eq!(event.changed_parameters[0].name, "speed");
    assert!(event.new_parameters.is_empty());
    assert!(event.deleted_parameters.is_empty());
}

// ── Service client tests (require ros-msgs feature) ────────────────────────

#[cfg(feature = "ros-msgs")]
mod service_tests {
    use std::{thread, time::Duration};

    use hiroz_msgs::rcl_interfaces::{
        self, DescribeParametersRequest, DescribeParametersResponse, GetParameterTypesRequest,
        GetParameterTypesResponse, GetParametersRequest, GetParametersResponse,
        ListParametersRequest, ListParametersResponse, SetParametersAtomicallyRequest,
        SetParametersAtomicallyResponse, SetParametersRequest, SetParametersResponse,
        srv::{
            DescribeParameters, GetParameterTypes, GetParameters, ListParameters, SetParameters,
            SetParametersAtomically,
        },
    };
    use zenoh_buffers::buffer::SplitBuffer;

    use super::*;

    /// Test parameter services via hiroz client: get/set/list/describe.
    #[test]
    fn test_parameter_services_get_and_set() {
        let router = TestRouter::new();

        let endpoint = router.endpoint().to_string();
        thread::spawn(move || {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
            let node = ctx.create_node("param_server").build().expect("node");

            let desc = ParameterDescriptor::new("value", ParameterType::Integer);
            node.declare_parameter("value", ParameterValue::Integer(7), desc)
                .expect("declare");

            thread::sleep(Duration::from_secs(30));
        });

        wait_for_ready(Duration::from_secs(2));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("ctx");
            let client_node = ctx.create_node("param_client").build().expect("node");

            tokio::time::sleep(Duration::from_millis(500)).await;

            // get_parameters
            let get_client = client_node
                .create_client::<GetParameters>("/param_server/get_parameters")
                .build()
                .expect("get client");

            let resp: GetParametersResponse = get_client
                .call_with_timeout(
                    &GetParametersRequest {
                        names: vec!["value".to_string()],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("response");

            assert_eq!(resp.values.len(), 1);
            assert_eq!(resp.values[0].r#type, 2); // PARAMETER_INTEGER
            assert_eq!(resp.values[0].integer_value, 7);

            // list_parameters
            let list_client = client_node
                .create_client::<ListParameters>("/param_server/list_parameters")
                .build()
                .expect("list client");

            let list_resp: ListParametersResponse = list_client
                .call_with_timeout(
                    &ListParametersRequest {
                        prefixes: vec![],
                        depth: 0,
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("list response");

            assert!(list_resp.result.names.contains(&"value".to_string()));

            // set_parameters
            let set_client = client_node
                .create_client::<SetParameters>("/param_server/set_parameters")
                .build()
                .expect("set client");

            let mut wire_value = rcl_interfaces::ParameterValue::default();
            wire_value.r#type = 2;
            wire_value.integer_value = 42;

            let set_resp: SetParametersResponse = set_client
                .call_with_timeout(
                    &SetParametersRequest {
                        parameters: vec![rcl_interfaces::Parameter {
                            name: "value".to_string(),
                            value: wire_value,
                        }],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("set response");

            assert_eq!(set_resp.results.len(), 1);
            assert!(
                set_resp.results[0].successful,
                "set failed: {}",
                set_resp.results[0].reason
            );

            // get_parameter_types
            let types_client = client_node
                .create_client::<GetParameterTypes>("/param_server/get_parameter_types")
                .build()
                .expect("types client");

            let types_resp: GetParameterTypesResponse = types_client
                .call_with_timeout(
                    &GetParameterTypesRequest {
                        names: vec!["value".to_string(), "nonexistent".to_string()],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("types response");

            let type_bytes = types_resp.types.contiguous();
            assert_eq!(type_bytes.len(), 2);
            assert_eq!(type_bytes[0], 2); // INTEGER
            assert_eq!(type_bytes[1], 0); // NOT_SET (undeclared)

            // describe_parameters
            let desc_client = client_node
                .create_client::<DescribeParameters>("/param_server/describe_parameters")
                .build()
                .expect("desc client");

            let desc_resp: DescribeParametersResponse = desc_client
                .call_with_timeout(
                    &DescribeParametersRequest {
                        names: vec!["value".to_string()],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("desc response");

            assert_eq!(desc_resp.descriptors.len(), 1);
            assert_eq!(desc_resp.descriptors[0].name, "value");
        });
    }

    /// Test that set_parameters_atomically rolls back all changes when the
    /// validation callback rejects the batch.
    ///
    /// Declares [a=1, b=2]. Registers a callback that forbids any batch
    /// touching "b". Calls set_parameters_atomically([a=10, b=20]) via the
    /// service, then verifies via get_parameters that both remain at their
    /// original values. This exercises validate_and_apply(atomic=true) with a
    /// user callback veto — a branch not covered by any other test.
    #[test]
    fn test_set_parameters_atomically_rollback_on_callback_rejection() {
        let router = TestRouter::new();

        let endpoint = router.endpoint().to_string();
        thread::spawn(move || {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
            let node = ctx.create_node("rollback_server").build().expect("node");

            for (name, val) in &[("a", 1i64), ("b", 2i64)] {
                let desc = ParameterDescriptor::new(*name, ParameterType::Integer);
                node.declare_parameter(*name, ParameterValue::Integer(*val), desc)
                    .expect("declare");
            }

            // Reject any batch that contains "b".
            node.on_set_parameters(|params| {
                if params.iter().any(|p| p.name == "b") {
                    return SetParametersResult::failure("b is forbidden".to_string());
                }
                SetParametersResult::success()
            });

            thread::sleep(Duration::from_secs(30));
        });

        wait_for_ready(Duration::from_secs(2));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("ctx");
            let client_node = ctx.create_node("rollback_client").build().expect("node");
            tokio::time::sleep(Duration::from_millis(500)).await;

            let make_int = |name: &str, v: i64| {
                let mut wire = rcl_interfaces::ParameterValue::default();
                wire.r#type = 2;
                wire.integer_value = v;
                rcl_interfaces::Parameter {
                    name: name.to_string(),
                    value: wire,
                }
            };

            // Attempt atomic set — callback veto should reject the entire batch.
            let atomic_client = client_node
                .create_client::<SetParametersAtomically>(
                    "/rollback_server/set_parameters_atomically",
                )
                .build()
                .expect("atomic client");

            let atomic_resp: SetParametersAtomicallyResponse = atomic_client
                .call_with_timeout(
                    &SetParametersAtomicallyRequest {
                        parameters: vec![make_int("a", 10), make_int("b", 20)],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("atomic response");

            assert!(
                !atomic_resp.result.successful,
                "expected rejection, got success"
            );
            assert!(
                atomic_resp.result.reason.contains("forbidden"),
                "unexpected reason: {}",
                atomic_resp.result.reason
            );

            // Verify both parameters are unchanged via get_parameters.
            let get_client = client_node
                .create_client::<GetParameters>("/rollback_server/get_parameters")
                .build()
                .expect("get client");

            let get_resp: GetParametersResponse = get_client
                .call_with_timeout(
                    &GetParametersRequest {
                        names: vec!["a".to_string(), "b".to_string()],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("get response");

            assert_eq!(get_resp.values.len(), 2);
            assert_eq!(get_resp.values[0].integer_value, 1, "a must be unchanged");
            assert_eq!(get_resp.values[1].integer_value, 2, "b must be unchanged");
        });
    }

    /// Test set_parameters_atomically.
    #[test]
    fn test_set_parameters_atomically() {
        let router = TestRouter::new();

        let endpoint = router.endpoint().to_string();
        thread::spawn(move || {
            let ctx = create_hiroz_context_with_endpoint(&endpoint).expect("ctx");
            let node = ctx.create_node("atomic_server").build().expect("node");

            for (name, val) in &[("a", 1i64), ("b", 2i64)] {
                let desc = ParameterDescriptor::new(*name, ParameterType::Integer);
                node.declare_parameter(*name, ParameterValue::Integer(*val), desc)
                    .expect("declare");
            }

            thread::sleep(Duration::from_secs(30));
        });

        wait_for_ready(Duration::from_secs(2));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = create_hiroz_context_with_router(&router).expect("ctx");
            let client_node = ctx.create_node("atomic_client").build().expect("node");
            tokio::time::sleep(Duration::from_millis(500)).await;

            let atomic_client = client_node
                .create_client::<SetParametersAtomically>(
                    "/atomic_server/set_parameters_atomically",
                )
                .build()
                .expect("atomic client");

            let make_int = |name: &str, v: i64| {
                let mut wire = rcl_interfaces::ParameterValue::default();
                wire.r#type = 2;
                wire.integer_value = v;
                rcl_interfaces::Parameter {
                    name: name.to_string(),
                    value: wire,
                }
            };

            let resp: SetParametersAtomicallyResponse = atomic_client
                .call_with_timeout(
                    &SetParametersAtomicallyRequest {
                        parameters: vec![make_int("a", 10), make_int("b", 20)],
                    },
                    Duration::from_secs(5),
                )
                .await
                .expect("atomic response");

            assert!(
                resp.result.successful,
                "atomic set failed: {}",
                resp.result.reason
            );
        });
    }
}
