//! ros::Host, HostSubscription, HostServiceClient implementations.

use std::sync::Arc;
use std::time::Duration;

use hiroz::{
    Builder,
    dynamic::{
        DynSub, DynamicMessage, DynamicValue, FieldType, MessageSchema,
        MessageSchemaTypeDescription,
        serialization::{deserialize_cdr, serialize_cdr},
    },
    graph::Graph,
    node::ZNode,
};
use wasmtime::component::Resource;
use zenoh::Wait;

use crate::core::message_formatter::dynamic_message_to_json;

use super::super::state::{PluginState, ServiceClientData, SubscriptionData};
use super::hu;
use hu::plugin::types::PluginError;

/// The message type advertised by a live publisher or subscriber on `topic`, if
/// any. Free-standing (rather than a `PluginState` method) so it can be tested
/// against a hand-built `Graph` without a wasmtime store.
fn live_topic_type_info(graph: &Graph, topic: &str) -> Option<hiroz_protocol::TypeInfo> {
    use hiroz_protocol::{EndpointKind, Entity};
    [EndpointKind::Publisher, EndpointKind::Subscription]
        .into_iter()
        .find_map(|kind| {
            graph
                .get_entities_by_topic(kind, topic)
                .first()
                .and_then(|ent| match ent.as_ref() {
                    Entity::Endpoint(ep) => ep.type_info.clone(),
                    _ => None,
                })
        })
}

/// The three outcomes of checking a local `.msg` against a topic's advertised
/// hash. "Not advertised" is deliberately distinct from "mismatch": it cannot be
/// verified either way, so it must not be refused.
#[derive(Debug, PartialEq, Eq)]
enum HashCheck {
    NotAdvertised,
    Match,
    Mismatch,
}

/// Compare two hashes by value.
///
/// Extracted so the three-way decision is testable without a router, a graph or
/// a `.msg` on disk. This guard has been wrong twice, both times in the
/// comparison rather than the surrounding plumbing, and both times found by
/// reading rather than by a test.
fn check_type_hash(local: &hiroz::TypeHash, advertised: &hiroz::TypeHash) -> HashCheck {
    if *advertised == hiroz::TypeHash::zero() {
        HashCheck::NotAdvertised
    } else if local == advertised {
        HashCheck::Match
    } else {
        HashCheck::Mismatch
    }
}

/// Render a `hiroz_protocol::TypeHash` as RIHS, without the `no-type-hash` gate.
///
/// `TypeHash::to_rihs_string` collapses to the constant "TypeHashNotSupported"
/// under that feature. That is right for the wire, where the constant *is* the
/// representation, and wrong for a diagnostic, which must show what the peer
/// actually advertised.
fn rihs_ungated(hash: &hiroz::TypeHash) -> String {
    let hex: String = hash.value.iter().map(|b| format!("{b:02x}")).collect();
    format!("RIHS{:02x}_{hex}", hash.version)
}

/// Build a dynamic subscriber for `topic` from a `.msg` on `HIROZ_MSG_PATH`,
/// used only when live discovery has already failed. The type *name* is still
/// taken from the graph -- `subscribe(topic)` carries no type, so without a live
/// endpoint there is nothing to look up.
///
/// Returns the specific reason on failure rather than a bare `None`: the three
/// ways this can fail (no live endpoint, no `.msg` on disk, the subscriber
/// declaration itself failing) send the reader to three different places, and
/// collapsing them into one "check `HIROZ_MSG_PATH`" message is the same class
/// of misdirection this whole path exists to remove.
fn dyn_sub_from_local_msg(node: &ZNode, graph: &Graph, topic: &str) -> Result<DynSub, String> {
    let Some(ti) = live_topic_type_info(graph, topic) else {
        return Err(format!(
            "no publisher or subscriber on {topic} advertises a type, so there is no \
             type name to look up -- `subscribe` carries only a topic, and these \
             commands have no --type flag"
        ));
    };
    let canonical = hiroz::dynamic::ros_type_name_from_dds(&ti.name);
    let Some(schema) = hiroz::dynamic::load_schema(&canonical) else {
        return Err(format!(
            "no .msg for {canonical} (advertised by {topic}) was found on HIROZ_MSG_PATH"
        ));
    };

    // The subscriber's key expression carries the *publisher's* hash, so messages
    // arrive even when the local .msg disagrees. CDR is positional, so a skewed
    // schema yields plausible, wrong field values rather than a decode error.
    // Refuse instead.
    //
    // Compare VALUES, never rendered strings: hiroz_protocol's renderer is gated
    // on `no-type-hash` and then returns one constant for every value, so a
    // string comparison passes on anything. Convert through the RIHS01 string --
    // the schema-side renderer and protocol-side parser are both ungated.
    //
    // "No hash advertised" is a third state, not a mismatch: unverifiable either
    // way, so warn and continue.
    let local_schema_hash = schema
        .compute_type_hash()
        .map_err(|e| format!("could not hash the local .msg for {canonical}: {e}"))?;
    let local = hiroz::TypeHash::from_rihs_string(&local_schema_hash.to_rihs_string())
        .unwrap_or_else(hiroz::TypeHash::zero);

    match check_type_hash(&local, &ti.hash) {
        // A publisher that advertises no hash -- a Humble node, or any peer
        // built without type hashing. "Not advertised" is a third state, not a
        // mismatch: it cannot be verified either way. Refusing here told the
        // user to point HIROZ_MSG_PATH at definitions hashing to zero, which
        // no .msg does, and made every cross-distro case fail -- the headline
        // case for having a disk fallback at all.
        HashCheck::NotAdvertised => tracing::warn!(
            "{topic} advertises no type hash, so the local .msg for {canonical} \
             cannot be verified against it; decoding with the local definition"
        ),
        HashCheck::Mismatch => {
            return Err(format!(
                "local .msg for {canonical} hashes to {} but {topic} advertises \
                 {}; refusing to decode with a mismatched schema -- point \
                 HIROZ_MSG_PATH at the message definitions the publisher was built from",
                local_schema_hash.to_rihs_string(),
                // Render both sides through an ungated formatter. `ti.hash` is a
                // `hiroz_protocol::TypeHash`, whose `to_rihs_string` is
                // `#[cfg(feature = "no-type-hash")]`-gated: under that feature it
                // returns the constant "TypeHashNotSupported" for every value, so
                // this message would tell the reader the publisher advertises
                // nothing while we refuse *because* it advertises something else.
                rihs_ungated(&ti.hash)
            ));
        }
        HashCheck::Match => {}
    }

    // Order matters: `with_type_info` assigns unconditionally, so it must follow
    // `create_dyn_sub` (which recomputes the hash from the local .msg). With the
    // hashes now proven equal this is belt-and-braces, and it keeps the key
    // byte-identical to what `create_dyn_sub_auto` would have declared.
    let sub = node
        .create_dyn_sub(topic, schema)
        .with_type_info(ti)
        .build()
        .map_err(|e| {
            format!("declaring a dynamic subscriber for {topic} ({canonical}) failed: {e}")
        })?;

    tracing::info!("resolved schema for {topic} from a local .msg ({canonical})");
    Ok(sub)
}

impl PluginState {
    /// See [`live_topic_type_info`]. Used both to build the concrete publish key
    /// (`resolve_topic_ke`) and to reject a disk-resolved type that conflicts
    /// with what the topic actually carries (`encode_yaml_to_cdr`).
    fn live_topic_type_info(&self, topic: &str) -> Option<hiroz_protocol::TypeInfo> {
        live_topic_type_info(&self.engine.graph, topic)
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

        // Resolve the schema *before* returning a Subscription. Inside the spawned
        // task every failure was invisible: `subscribe` had already returned Ok,
        // so the plugin's error branch could never fire and a dropped sender read
        // as a permanently idle topic.
        //
        // `block_in_place` hands the worker to the blocking pool, so the zenoh
        // I/O this discovery depends on keeps progressing. `HostBlockGuard` stops
        // the epoch ticker: the guest runs under a ~3 s wall-clock budget, so
        // without it a long wait traps the guest before it can run that error
        // branch, and the command hangs instead of reporting.
        const SUB_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
        let node = self.engine.node.clone();
        let discovered = {
            let _epoch = super::super::HostBlockGuard::enter();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(node.create_dyn_sub_auto(&topic, SUB_DISCOVERY_TIMEOUT))
            })
        };
        let sub = match discovered {
            Ok(sub) => sub,
            Err(e) => {
                // Discovery is authoritative when it answers, but a node built
                // without `.with_type_description_service()` answers nothing --
                // so fall back to the same on-disk `.msg` lookup the publish
                // path uses (`encode_yaml_to_cdr`). The type *name* still comes
                // from the graph; only the schema body comes from disk.
                tracing::debug!("WASM plugin: schema discovery failed for {topic}: {e}");
                match dyn_sub_from_local_msg(&node, &self.engine.graph, &topic) {
                    Ok(sub) => sub,
                    // Both reasons are reported: which one matters depends on
                    // whether the user expected discovery or the disk to answer.
                    Err(fallback) => {
                        return Err(PluginError::Transport(format!(
                            "no schema for {topic}: discovery failed ({e}); {fallback}"
                        )));
                    }
                }
            }
        };

        // Mint the resource only once the subscription really exists, so a
        // failed subscribe doesn't burn a rep.
        let rep = self.alloc_rep();
        let (tx, rx) = flume::bounded::<String>(256);
        let topic_for_log = topic.clone();

        let handle = tokio::spawn(async move {
            // A stream that decodes to nothing is indistinguishable from an idle
            // one, which is the same silence this whole path had. Report the
            // first failure loudly and the rest at debug, so a fast topic whose
            // schema drifted announces itself without flooding stderr.
            let mut decode_errors: u64 = 0;
            loop {
                match sub.try_recv() {
                    Some(Ok(msg)) => {
                        let json = dynamic_message_to_json(&msg).to_string();
                        if tx.send_async(json).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        decode_errors += 1;
                        if decode_errors == 1 {
                            tracing::warn!(
                                topic = %topic_for_log,
                                "WASM plugin: message decode failed: {e} \
                                 (further errors logged at debug)"
                            );
                        } else {
                            tracing::debug!(
                                topic = %topic_for_log,
                                count = decode_errors,
                                "WASM plugin: message decode failed: {e}"
                            );
                        }
                    }
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

        let timeout = Duration::from_millis(timeout_ms as u64);
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

        let timeout = Duration::from_millis(timeout_ms as u64);
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
mod graph_type_name_tests {
    use hiroz::dynamic::ros_type_name_from_dds;

    // `dyn_sub_from_local_msg` feeds a graph-reported (DDS-mangled) type name
    // straight into `load_schema`, which only accepts the canonical form. Pin the
    // conversion at the crate boundary -- a regression here shows up as "no .msg
    // found" rather than as a type error.
    #[test]
    fn graph_type_names_normalize_for_schema_lookup() {
        assert_eq!(
            ros_type_name_from_dds("std_msgs::msg::dds_::String_"),
            "std_msgs/msg/String"
        );
        // Some publishers report the un-`dds_`-qualified form.
        assert_eq!(
            ros_type_name_from_dds("rcl_interfaces::msg::ParameterEvent_"),
            "rcl_interfaces/msg/ParameterEvent"
        );
        // Already-canonical names must survive unchanged.
        assert_eq!(
            ros_type_name_from_dds("std_msgs/msg/String"),
            "std_msgs/msg/String"
        );
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
mod type_hash_guard_tests {
    use super::{HashCheck, check_type_hash, rihs_ungated};
    use hiroz::TypeHash;

    fn hash(byte: u8) -> TypeHash {
        TypeHash::new(1, [byte; 32])
    }

    #[test]
    fn equal_hashes_match() {
        assert_eq!(check_type_hash(&hash(0xab), &hash(0xab)), HashCheck::Match);
    }

    #[test]
    fn different_hashes_mismatch() {
        assert_eq!(
            check_type_hash(&hash(0xab), &hash(0xcd)),
            HashCheck::Mismatch
        );
    }

    // The case the guard got wrong in the other direction: a peer that
    // advertises nothing is unverifiable, not mismatched. Refusing it made every
    // cross-distro subscribe fail, which is the headline case for a disk
    // fallback existing at all.
    #[test]
    fn absent_advertised_hash_is_not_a_mismatch() {
        assert_eq!(
            check_type_hash(&hash(0xab), &TypeHash::zero()),
            HashCheck::NotAdvertised
        );
    }

    // A local .msg cannot hash to zero, but pin the precedence anyway: the
    // "not advertised" arm is checked first, so two zeros are not a match.
    #[test]
    fn absent_beats_equality_when_both_are_zero() {
        assert_eq!(
            check_type_hash(&TypeHash::zero(), &TypeHash::zero()),
            HashCheck::NotAdvertised
        );
    }

    // The reason the comparison is by value and not by rendered string. Under
    // `no-type-hash` the protocol renderer returns one constant for every hash,
    // so any string comparison passes on anything. This asserts the property
    // that made string comparison wrong, and it holds on every build.
    #[test]
    fn distinct_hashes_stay_distinct_by_value() {
        let a = hash(0x01);
        let b = hash(0x02);
        assert_ne!(a, b);
        assert_eq!(check_type_hash(&a, &b), HashCheck::Mismatch);
    }

    // The diagnostic must show the advertised bytes. `to_rihs_string` is gated
    // and would print "TypeHashNotSupported" under `no-type-hash`, telling the
    // reader the publisher advertised nothing while we refuse because it
    // advertised something else.
    #[test]
    fn ungated_renderer_shows_the_bytes() {
        assert_eq!(
            rihs_ungated(&hash(0xab)),
            format!("RIHS01_{}", "ab".repeat(32))
        );
        assert_ne!(rihs_ungated(&hash(0xab)), "TypeHashNotSupported");
    }
}
