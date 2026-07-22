//! ros::Host, HostSubscription, HostServiceClient implementations.

use std::sync::Arc;
use std::time::Duration;

use hiroz::dynamic::{
    DynamicMessage, DynamicValue, FieldType, MessageSchema, get_schema,
    serialization::{deserialize_cdr, serialize_cdr},
};
use wasmtime::component::Resource;
use zenoh::Wait;

use crate::core::message_formatter::dynamic_message_to_json;

use super::super::state::{PluginState, ServiceClientData, SubscriptionData};
use super::hu;

impl hu::plugin::ros::Host for PluginState {
    fn subscribe(
        &mut self,
        topic: String,
    ) -> Result<Resource<hu::plugin::ros::Subscription>, String> {
        self.require_perm(hu::plugin::types::Permission::SubscribeTopic)?;
        let rep = self.alloc_rep();

        let (tx, rx) = flume::bounded::<String>(256);
        let node = self.engine.node.clone();
        let topic_clone = topic.clone();

        let handle = tokio::spawn(async move {
            let sub = match node
                .create_dyn_sub_auto(&topic_clone, Duration::from_secs(5))
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "WASM plugin: schema discovery failed for {}: {e}",
                        topic_clone
                    );
                    return;
                }
            };
            loop {
                match sub.try_recv() {
                    Some(Ok(msg)) => {
                        let json = dynamic_message_to_json(&msg).to_string();
                        if tx.send_async(json).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => {}
                    None => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        });

        self.subscriptions.insert(
            rep,
            SubscriptionData {
                topic,
                rx,
                _abort: handle.abort_handle(),
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn connect_service(
        &mut self,
        name: String,
        type_name: String,
    ) -> Result<Resource<hu::plugin::ros::ServiceClient>, String> {
        self.require_perm(hu::plugin::types::Permission::CallService)?;
        let domain_id = self.engine.domain_id;
        let svc_stripped = name.trim_start_matches('/').to_string();

        let ke = {
            use hiroz_protocol::{EndpointKind, Entity, KeyExprFormatter, RmwZenohFormatter};
            let entities = self
                .engine
                .graph
                .get_entities_by_topic(EndpointKind::Service, &name);
            let type_info = entities.first().and_then(|ent| match ent.as_ref() {
                Entity::Endpoint(ep) => ep.type_info.as_ref(),
                _ => None,
            });
            match type_info {
                Some(ti) => {
                    let type_escaped = RmwZenohFormatter::mangle_name(&ti.name);
                    format!("{domain_id}/{svc_stripped}/{type_escaped}/{}", ti.hash)
                }
                None => {
                    let type_escaped = RmwZenohFormatter::mangle_name(&type_name);
                    format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
                }
            }
        };

        let session = self.engine.session.clone();
        let rep = self.alloc_rep();
        self.service_clients.insert(
            rep,
            ServiceClientData {
                session,
                ke,
                type_name,
                name,
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn measure_hz(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        let (count, _, window_s) = self.get_tracker_snapshot(&topic, window_ms)?;
        Ok(count as f64 / window_s)
    }

    fn measure_bw(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        let (_, total_bytes, window_s) = self.get_tracker_snapshot(&topic, window_ms)?;
        Ok(total_bytes as f64 / 1024.0 / window_s)
    }

    // NOTE: still reads the global (structurally never-populated) SchemaRegistry --
    // NOT rerouted to discovery like HostServiceClient::call. This is a topic-pub
    // path (only caller is `hu meter pub`'s --msg-type/--yaml, via
    // encode-yaml-to-cdr), and the discovery mechanism that would fix it
    // (discover_topic_schema) is keyed by *topic*, not type name -- but this WIT
    // function's signature is `(yaml, type-name)` with no topic parameter to key
    // a discovery query on. Fixing this properly needs a WIT interface change
    // (add a topic param, or a topic-keyed variant) across all plugin copies of
    // wit/hu-plugin.wit, which is out of scope here. Left as a known, disclosed
    // gap (see pr-readiness.md's `pub_yaml_nested_twist` note) rather than
    // reroute to the wrong discovery primitive.
    fn encode_yaml_to_cdr(&mut self, yaml: String, type_name: String) -> Result<Vec<u8>, String> {
        self.require_perm(hu::plugin::types::Permission::PublishTopic)?;
        let schema = get_schema(&type_name)
            .ok_or_else(|| format!("schema for '{type_name}' not found in registry"))?;
        let value = parse_yaml_or_json(&yaml)?;
        let msg = json_to_dynamic_message(&value, &schema)?;
        serialize_cdr(&msg).map_err(|e| e.to_string())
    }

    fn measure_hz_typed(
        &mut self,
        topic: String,
        window_ms: u32,
    ) -> Result<hu::plugin::ros::HzMeasurement, String> {
        let (count, _, window_s) = self.get_tracker_snapshot(&topic, window_ms)?;
        Ok(hu::plugin::ros::HzMeasurement {
            topic,
            rate_hz: count as f64 / window_s,
            sample_count: count as u32,
        })
    }

    fn measure_bw_typed(
        &mut self,
        topic: String,
        window_ms: u32,
    ) -> Result<hu::plugin::ros::BwMeasurement, String> {
        let (count, total_bytes, window_s) = self.get_tracker_snapshot(&topic, window_ms)?;
        Ok(hu::plugin::ros::BwMeasurement {
            topic,
            rate_kbps: total_bytes as f64 / 1024.0 / window_s,
            sample_count: count as u32,
        })
    }
}

impl hu::plugin::ros::HostSubscription for PluginState {
    fn try_recv(&mut self, res: Resource<hu::plugin::ros::Subscription>) -> Option<String> {
        self.subscriptions
            .get(&res.rep())
            .and_then(|sub| sub.rx.try_recv().ok())
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::Subscription>) -> wasmtime::Result<()> {
        self.subscriptions.remove(&res.rep());
        Ok(())
    }
}

impl hu::plugin::ros::HostServiceClient for PluginState {
    fn call(
        &mut self,
        res: Resource<hu::plugin::ros::ServiceClient>,
        request_json: String,
        timeout_ms: u32,
    ) -> Result<String, String> {
        let rep = res.rep();
        let Some(data) = self.service_clients.get(&rep) else {
            return Err("service client not found".to_string());
        };
        let session = data.session.clone();
        let ke = data.ke.clone();
        let service_name = data.name.clone();
        // `data.type_name` is the *service*-level type (e.g.
        // "example_interfaces/srv/AddTwoInts" or "rcl_interfaces/srv/GetParameters"),
        // not a Request/Response message type name. Resolve the actual
        // Request/Response schemas via live discovery instead of the
        // (structurally never-populated for services) global SchemaRegistry --
        // see ZNode::discover_service_schema.
        let (req_type, resp_type) = service_request_response_type_names(&data.type_name);
        let node = self.engine.node.clone();

        // Budget schema discovery separately from the caller's own --timeout so a
        // slow/failed discovery round-trip (two get_type_description queries)
        // can't consume the entire per-call timeout budget.
        const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(2000);
        let (req_schema, resp_schema) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(node.discover_service_schema(
                &service_name,
                &req_type,
                &resp_type,
                DISCOVERY_TIMEOUT,
            ))
        })
        .map_err(|e| format!("schema discovery for service '{service_name}' failed: {e}"))?;

        let req_value = parse_yaml_or_json(&request_json)?;
        let req_msg = json_to_dynamic_message(&req_value, &req_schema)?;
        let req_cdr = serialize_cdr(&req_msg).map_err(|e| e.to_string())?;

        let timeout = Duration::from_millis(timeout_ms as u64);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(req_cdr))
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;

        let reply = replies
            .recv()
            .map_err(|_| "no reply within timeout".to_string())?;
        let sample = reply.result().map_err(|e| e.to_string())?;
        let resp_cdr = sample.payload().to_bytes().into_owned();

        match deserialize_cdr(&resp_cdr, &resp_schema) {
            Ok(msg) => Ok(dynamic_message_to_json(&msg).to_string()),
            Err(e) => {
                tracing::warn!("failed to decode service response: {e}");
                Ok(format!(
                    "{{\"raw\":\"{}\"}}",
                    resp_cdr
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ))
            }
        }
    }

    fn call_raw(
        &mut self,
        res: Resource<hu::plugin::ros::ServiceClient>,
        payload: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, String> {
        let rep = res.rep();
        let Some(data) = self.service_clients.get(&rep) else {
            return Err("service client not found".to_string());
        };
        let session = data.session.clone();
        let ke = data.ke.clone();

        let timeout = Duration::from_millis(timeout_ms as u64);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(payload))
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;

        let reply = replies
            .recv()
            .map_err(|_| "no reply within timeout".to_string())?;
        let sample = reply.result().map_err(|e| e.to_string())?;
        Ok(sample.payload().to_bytes().into_owned())
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
        self.service_clients.remove(&res.rep());
        Ok(())
    }
}

/// Derive a service's Request/Response type names from its service-level type
/// name, matching the naming convention `hiroz-codegen` uses for generated
/// service message types (`generate_service_impl`/`parse_srv_string`): the
/// Request/Response structs are generated as ordinary messages named
/// `{Name}Request`/`{Name}Response` in the *same package*, registered under
/// `{pkg}/msg/{Name}Request` (note: `/msg/`, not `/srv/` -- services don't get
/// their own schema namespace, their Request/Response are just messages).
///
/// `service_type` is expected in `pkg/srv/Name` form (as reported by the graph
/// and used throughout hu-meter, e.g. `"rcl_interfaces/srv/GetParameters"`).
/// Falls back to a `_Request`/`_Response` suffix on the input unchanged if it
/// doesn't match that shape (e.g. an already-DDS-mangled or malformed name) --
/// best effort, since a well-formed `pkg/srv/Name` is the documented contract
/// for what callers pass as `type_name` to `connect_service`.
fn service_request_response_type_names(service_type: &str) -> (String, String) {
    match service_type.split_once("/srv/") {
        Some((pkg, name)) => (
            format!("{pkg}/msg/{name}Request"),
            format!("{pkg}/msg/{name}Response"),
        ),
        None => (
            format!("{service_type}_Request"),
            format!("{service_type}_Response"),
        ),
    }
}

// ─── YAML/JSON→CDR helpers ───────────────────────────────────────────────────

/// Parse a request/pub body that may be either strict JSON (quoted keys) or
/// flow-style YAML (unquoted keys, e.g. `{a: 1, b: 2}`) — both are common in
/// CLI-supplied `--yaml`/request strings, and `--yaml`-named flags in
/// particular are documented as accepting YAML, not JSON. YAML is a JSON
/// superset for our purposes, so try JSON first (fast path, and gives JSON's
/// error messages when the caller really did pass invalid JSON) and fall
/// back to a YAML parse re-expressed as `serde_json::Value`.
fn parse_yaml_or_json(input: &str) -> Result<serde_json::Value, String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(input) {
        return Ok(v);
    }
    serde_yaml::from_str::<serde_json::Value>(input)
        .map_err(|e| format!("failed to parse YAML/JSON: {e}"))
}

// ─── JSON→CDR helpers ────────────────────────────────────────────────────────

fn json_to_dynamic_message(
    value: &serde_json::Value,
    schema: &Arc<MessageSchema>,
) -> Result<DynamicMessage, String> {
    let obj = value.as_object().ok_or("expected a JSON object")?;
    let mut msg = DynamicMessage::new(schema);
    for field in &schema.fields {
        if let Some(v) = obj.get(&field.name) {
            let dval = json_to_dynamic_value(v, &field.field_type)?;
            msg.set_dynamic(&field.name, dval)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(msg)
}

fn json_to_dynamic_value(
    value: &serde_json::Value,
    ty: &FieldType,
) -> Result<DynamicValue, String> {
    match ty {
        FieldType::Bool => Ok(DynamicValue::Bool(value.as_bool().ok_or("expected bool")?)),
        FieldType::Int8 => Ok(DynamicValue::Int8(
            value.as_i64().ok_or("expected i8")? as i8
        )),
        FieldType::Int16 => Ok(DynamicValue::Int16(
            value.as_i64().ok_or("expected i16")? as i16
        )),
        FieldType::Int32 => Ok(DynamicValue::Int32(
            value.as_i64().ok_or("expected i32")? as i32
        )),
        FieldType::Int64 => Ok(DynamicValue::Int64(value.as_i64().ok_or("expected i64")?)),
        FieldType::Uint8 => Ok(DynamicValue::Uint8(
            value.as_u64().ok_or("expected u8")? as u8
        )),
        FieldType::Uint16 => Ok(DynamicValue::Uint16(
            value.as_u64().ok_or("expected u16")? as u16
        )),
        FieldType::Uint32 => Ok(DynamicValue::Uint32(
            value.as_u64().ok_or("expected u32")? as u32
        )),
        FieldType::Uint64 => Ok(DynamicValue::Uint64(value.as_u64().ok_or("expected u64")?)),
        FieldType::Float32 => Ok(DynamicValue::Float32(
            value.as_f64().ok_or("expected f32")? as f32
        )),
        FieldType::Float64 => Ok(DynamicValue::Float64(value.as_f64().ok_or("expected f64")?)),
        FieldType::String | FieldType::BoundedString(_) => Ok(DynamicValue::String(
            value.as_str().ok_or("expected string")?.to_string(),
        )),
        FieldType::Message(inner_schema) => Ok(DynamicValue::Message(Box::new(
            json_to_dynamic_message(value, inner_schema)?,
        ))),
        FieldType::Array(inner, _)
        | FieldType::Sequence(inner)
        | FieldType::BoundedSequence(inner, _) => {
            let arr = value.as_array().ok_or("expected array")?;
            let items = arr
                .iter()
                .map(|v| json_to_dynamic_value(v, inner))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DynamicValue::Array(items))
        }
    }
}
