//! ros::Host, HostSubscription, HostServiceClient implementations.

use std::sync::Arc;
use std::time::Duration;

use hiroz::dynamic::{
    DynamicMessage, DynamicValue, FieldType, MessageSchema,
    serialization::{deserialize_cdr, serialize_cdr},
};
use wasmtime::component::Resource;
use zenoh::Wait;

use crate::core::message_formatter::dynamic_message_to_json;

use super::super::state::{PluginState, ServiceClientData, SubscriptionData};
use super::hu;
use hu::plugin::types::PluginError;

/// The longest time that a guest-supplied service timeout may suspend the
/// epoch ticker.
///
/// `timeout-ms` crosses the WIT boundary as a `u32`, and the guest chooses its
/// value. The host holds a `HostBlockGuard` across the reply wait. Without a
/// clamp, a plugin picks how long the host stops preempting plugins. An
/// unclamped `u32` allows about 49 days.
///
/// The watchdog exists to preempt a guest that does not yield. The guest must
/// not control it. 60 s is far above any real service call. It is also far
/// below a disabled ticker.
const MAX_GUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Caps a guest-supplied timeout. The host writes a warning when it reduces the
/// value, so that the plugin author sees why the timeout changed.
fn clamp_guest_timeout(timeout_ms: u32) -> Duration {
    let requested = Duration::from_millis(timeout_ms as u64);
    if requested > MAX_GUEST_TIMEOUT {
        tracing::warn!(
            "plugin asked for a {}ms service timeout; clamping to {}ms",
            timeout_ms,
            MAX_GUEST_TIMEOUT.as_millis()
        );
        MAX_GUEST_TIMEOUT
    } else {
        requested
    }
}

impl PluginState {
    /// The message type advertised by a live publisher or subscriber on `topic`,
    /// if any. Used both to build the concrete publish key (`resolve_topic_ke`)
    /// and to reject a disk-resolved type that conflicts with what the topic
    /// actually carries (`encode_yaml_to_cdr`).
    fn live_topic_type_info(&self, topic: &str) -> Option<hiroz_protocol::TypeInfo> {
        use hiroz_protocol::{EndpointKind, Entity};
        [EndpointKind::Publisher, EndpointKind::Subscription]
            .into_iter()
            .find_map(|kind| {
                self.engine
                    .graph
                    .get_entities_by_topic(kind, topic)
                    .first()
                    .and_then(|ent| match ent.as_ref() {
                        Entity::Endpoint(ep) => ep.type_info.clone(),
                        _ => None,
                    })
            })
    }
}

impl hu::plugin::ros::Host for PluginState {
    fn resolve_topic_ke(&mut self, topic: String) -> Result<String, PluginError> {
        use hiroz_protocol::{KeyExprFormatter, RmwZenohFormatter};
        let domain_id = self.engine.domain_id;
        let topic_stripped = topic.trim_start_matches('/').to_string();

        let type_info = self.live_topic_type_info(&topic);

        Ok(match type_info {
            Some(ti) => {
                let type_escaped = RmwZenohFormatter::mangle_name(&ti.name);
                format!("{domain_id}/{topic_stripped}/{type_escaped}/{}", ti.hash)
            }
            None => format!("{domain_id}/{topic_stripped}/**"),
        })
    }

    fn subscribe(
        &mut self,
        topic: String,
    ) -> Result<Resource<hu::plugin::ros::Subscription>, PluginError> {
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
    ) -> Result<Resource<hu::plugin::ros::ServiceClient>, PluginError> {
        self.require_perm(hu::plugin::types::Permission::CallService)?;
        let domain_id = self.engine.domain_id;
        let svc_stripped = name.trim_start_matches('/').to_string();

        let ke = {
            use hiroz_protocol::{EndpointKind, Entity, KeyExprFormatter, RmwZenohFormatter};
            let entities = self
                .engine
                .graph
                .get_entities_by_service(EndpointKind::Service, &name);
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

    // Resolve the message schema for `type_name`, preferring `.msg` files on disk
    // (via HIROZ_MSG_PATH) so publishing works even on a topic with no live node,
    // like `ros2 topic pub`. When the type isn't on disk, fall back to live
    // discovery from a node already on the topic (same approach as
    // HostServiceClient::call). Either way, if the topic already carries a
    // different wire type, that mismatch is reported rather than published with
    // the wrong type.
    fn encode_yaml_to_cdr(
        &mut self,
        topic: String,
        yaml: String,
        type_name: String,
    ) -> Result<Vec<u8>, PluginError> {
        self.require_perm(hu::plugin::types::Permission::PublishTopic)?;

        let schema = match hiroz::dynamic::load_schema(&type_name) {
            Some(schema) => {
                // The publish key is built by `resolve_topic_ke` from a live
                // endpoint's type. If the topic already carries a *different*
                // type, encoding the requested type here would put incompatible
                // bytes onto that endpoint's key — reject instead (same guard as
                // the discovery branch below). An empty topic has no live type,
                // so publishing to it still works, like `ros2 topic pub`.
                if let Some(live) = self.live_topic_type_info(&topic)
                    && live.name != type_name
                {
                    return Err(PluginError::Invalid(format!(
                        "topic {topic} carries {}, not the requested {type_name}",
                        live.name
                    )));
                }
                schema
            }
            None => {
                let node = self.engine.node.clone();
                // Budget discovery independently; a slow/failed round-trip
                // shouldn't look like a generic encode failure.
                const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(2000);
                let discovered = {
                    // See `subscribe`: wall-clock waits are charged to the
                    // guest's epoch budget unless the ticker is suspended.
                    let _epoch = super::super::HostBlockGuard::enter();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(
                            node.discover_topic_schema_including_subscribers(
                                &topic,
                                DISCOVERY_TIMEOUT,
                            ),
                        )
                    })
                }
                .map_err(|_| PluginError::NotFound)?;
                if discovered.schema.type_name != type_name {
                    return Err(PluginError::Invalid(format!(
                        "topic {topic} carries {}, not the requested {type_name}",
                        discovered.schema.type_name
                    )));
                }
                discovered.schema
            }
        };

        let value = parse_yaml_or_json(&yaml).map_err(PluginError::Invalid)?;
        let msg = json_to_dynamic_message(&value, &schema).map_err(PluginError::Invalid)?;
        serialize_cdr(&msg).map_err(|e| PluginError::Invalid(e.to_string()))
    }

    fn measure_hz(
        &mut self,
        topic: String,
        window_ms: u32,
    ) -> Result<hu::plugin::ros::HzMeasurement, PluginError> {
        let (count, _, window_s) = self.get_tracker_snapshot(&topic, window_ms)?;
        Ok(hu::plugin::ros::HzMeasurement {
            topic,
            rate_hz: count as f64 / window_s,
            sample_count: count as u32,
        })
    }

    fn measure_bw(
        &mut self,
        topic: String,
        window_ms: u32,
    ) -> Result<hu::plugin::ros::BwMeasurement, PluginError> {
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
    ) -> Result<String, PluginError> {
        let rep = res.rep();
        let Some(data) = self.service_clients.get(&rep) else {
            return Err(PluginError::Invalid("service client not found".to_string()));
        };
        let session = data.session.clone();
        let ke = data.ke.clone();
        let service_name = data.name.clone();
        // `data.type_name` is the service-level type (e.g.
        // "example_interfaces/srv/AddTwoInts"), not a Request/Response message
        // type. Resolve the actual schemas via live discovery, not the
        // (never-populated for services) SchemaRegistry — see
        // ZNode::discover_service_schema.
        let (req_type, resp_type) = service_request_response_type_names(&data.type_name);
        let node = self.engine.node.clone();

        // Budget schema discovery separately from the caller's own --timeout so a
        // slow/failed discovery round-trip (two get_type_description queries)
        // can't consume the entire per-call timeout budget.
        const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(2000);
        let (req_schema, resp_schema) = {
            // See `subscribe`: suspend the epoch ticker across the wait.
            let _epoch = super::super::HostBlockGuard::enter();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(node.discover_service_schema(
                    &service_name,
                    &req_type,
                    &resp_type,
                    DISCOVERY_TIMEOUT,
                ))
            })
        }
        .map_err(|_| PluginError::NotFound)?;

        let req_value = parse_yaml_or_json(&request_json).map_err(PluginError::Invalid)?;
        let req_msg =
            json_to_dynamic_message(&req_value, &req_schema).map_err(PluginError::Invalid)?;
        let req_cdr = serialize_cdr(&req_msg).map_err(|e| PluginError::Invalid(e.to_string()))?;

        // The hiroz service queryable requires an RMW-style attachment (seqnum
        // + timestamp + writer GID) on every query to correlate request and
        // response; without one the query is silently never answered. See
        // `ZClient::call_sample` / `ZServer` in hiroz/src/service.rs.
        let gid: hiroz::GidArray = session.zid().to_le_bytes();
        let sn = self.alloc_rep() as i64;
        let attachment = hiroz::attachment::Attachment::new(sn, gid);

        let timeout = clamp_guest_timeout(timeout_ms);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(req_cdr))
            .attachment(attachment)
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;

        let reply = {
            // The caller's own --timeout can exceed the guest's epoch budget, so
            // suspend the ticker across the wait (see `subscribe`).
            let _epoch = super::super::HostBlockGuard::enter();
            replies.recv()
        }
        .map_err(|_| PluginError::Timeout)?;
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
    ) -> Result<Vec<u8>, PluginError> {
        let rep = res.rep();
        let Some(data) = self.service_clients.get(&rep) else {
            return Err(PluginError::Invalid("service client not found".to_string()));
        };
        let session = data.session.clone();
        let ke = data.ke.clone();

        // See the identical comment in `call` -- the queryable requires an
        // RMW-style attachment or it never replies.
        let gid: hiroz::GidArray = session.zid().to_le_bytes();
        let sn = self.alloc_rep() as i64;
        let attachment = hiroz::attachment::Attachment::new(sn, gid);

        let timeout = clamp_guest_timeout(timeout_ms);
        let replies = session
            .get(&ke)
            .payload(zenoh::bytes::ZBytes::from(payload))
            .attachment(attachment)
            .timeout(timeout)
            .wait()
            .map_err(|e| e.to_string())?;

        let reply = {
            // The caller's own --timeout can exceed the guest's epoch budget, so
            // suspend the ticker across the wait (see `subscribe`).
            let _epoch = super::super::HostBlockGuard::enter();
            replies.recv()
        }
        .map_err(|_| PluginError::Timeout)?;
        let sample = reply.result().map_err(|e| e.to_string())?;
        Ok(sample.payload().to_bytes().into_owned())
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
        self.service_clients.remove(&res.rep());
        Ok(())
    }
}

/// Derive a service's Request/Response type names from its service-level type
/// name, matching `hiroz-codegen`'s convention: `{Name}Request`/`{Name}Response`
/// registered as ordinary messages under `{pkg}/msg/` (services get no schema
/// namespace of their own).
///
/// `service_type` may arrive as the abstract service type (`pkg/srv/Name`), a
/// request-side type (`pkg/srv/Name_Request`), or the DDS-mangled graph form
/// (`pkg::srv::dds_::Name_Request_` / `pkg::srv::dds_::Name_`). All normalize to
/// bare `pkg/srv/Name` first, so suffixes are never double-appended.
fn service_request_response_type_names(service_type: &str) -> (String, String) {
    let normalized = service_type.replace("dds_::", "").replace("::", "/");
    let normalized = normalized.strip_suffix('_').unwrap_or(&normalized);
    let normalized = normalized
        .strip_suffix("_Request")
        .or_else(|| normalized.strip_suffix("_Response"))
        .unwrap_or(normalized);
    match normalized.split_once("/srv/") {
        Some((pkg, name)) => (
            format!("{pkg}/msg/{name}Request"),
            format!("{pkg}/msg/{name}Response"),
        ),
        None => (
            format!("{normalized}_Request"),
            format!("{normalized}_Response"),
        ),
    }
}

// ─── YAML/JSON→CDR helpers ───────────────────────────────────────────────────

/// Parse a request/pub body that may be strict JSON or flow-style YAML (e.g.
/// `{a: 1, b: 2}`) — both are common in CLI `--yaml`/request strings. YAML is a
/// JSON superset here, so try JSON first (fast path, better errors on real
/// invalid JSON) and fall back to YAML re-expressed as `serde_json::Value`.
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
            i8::try_from(value.as_i64().ok_or("expected i8")?)
                .map_err(|_| "value out of range for i8")?,
        )),
        FieldType::Int16 => Ok(DynamicValue::Int16(
            i16::try_from(value.as_i64().ok_or("expected i16")?)
                .map_err(|_| "value out of range for i16")?,
        )),
        FieldType::Int32 => Ok(DynamicValue::Int32(
            i32::try_from(value.as_i64().ok_or("expected i32")?)
                .map_err(|_| "value out of range for i32")?,
        )),
        FieldType::Int64 => Ok(DynamicValue::Int64(value.as_i64().ok_or("expected i64")?)),
        FieldType::Uint8 => Ok(DynamicValue::Uint8(
            u8::try_from(value.as_u64().ok_or("expected u8")?)
                .map_err(|_| "value out of range for u8")?,
        )),
        FieldType::Uint16 => Ok(DynamicValue::Uint16(
            u16::try_from(value.as_u64().ok_or("expected u16")?)
                .map_err(|_| "value out of range for u16")?,
        )),
        FieldType::Uint32 => Ok(DynamicValue::Uint32(
            u32::try_from(value.as_u64().ok_or("expected u32")?)
                .map_err(|_| "value out of range for u32")?,
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

#[cfg(test)]
mod service_type_name_tests {
    use super::service_request_response_type_names;

    #[test]
    fn abstract_service_type() {
        assert_eq!(
            service_request_response_type_names("example_interfaces/srv/AddTwoInts"),
            (
                "example_interfaces/msg/AddTwoIntsRequest".to_string(),
                "example_interfaces/msg/AddTwoIntsResponse".to_string(),
            )
        );
    }

    #[test]
    fn caller_supplied_request_suffixed_type() {
        assert_eq!(
            service_request_response_type_names("example_interfaces/srv/AddTwoInts_Request"),
            (
                "example_interfaces/msg/AddTwoIntsRequest".to_string(),
                "example_interfaces/msg/AddTwoIntsResponse".to_string(),
            )
        );
    }

    #[test]
    fn raw_dds_mangled_request_type() {
        assert_eq!(
            service_request_response_type_names(
                "example_interfaces::srv::dds_::AddTwoInts_Request_"
            ),
            (
                "example_interfaces/msg/AddTwoIntsRequest".to_string(),
                "example_interfaces/msg/AddTwoIntsResponse".to_string(),
            )
        );
    }

    #[test]
    fn raw_dds_mangled_service_type() {
        assert_eq!(
            service_request_response_type_names("example_interfaces::srv::dds_::AddTwoInts_"),
            (
                "example_interfaces/msg/AddTwoIntsRequest".to_string(),
                "example_interfaces/msg/AddTwoIntsResponse".to_string(),
            )
        );
    }
}

#[cfg(test)]
mod guest_timeout_tests {
    use super::{MAX_GUEST_TIMEOUT, clamp_guest_timeout};
    use std::time::Duration;

    // This clamp stops a guest-supplied `timeout-ms` from deciding how long the
    // host suspends the epoch ticker. That suspension covers every plugin in
    // the process. A later edit can delete the clamp in one line.
    #[test]
    fn a_reasonable_timeout_passes_through() {
        assert_eq!(clamp_guest_timeout(2_000), Duration::from_millis(2_000));
    }

    #[test]
    fn the_maximum_itself_is_not_clamped() {
        assert_eq!(
            clamp_guest_timeout(MAX_GUEST_TIMEOUT.as_millis() as u32),
            MAX_GUEST_TIMEOUT
        );
    }

    #[test]
    fn an_absurd_timeout_is_clamped() {
        // ~49 days, the worst a u32 can ask for.
        assert_eq!(clamp_guest_timeout(u32::MAX), MAX_GUEST_TIMEOUT);
    }
}
