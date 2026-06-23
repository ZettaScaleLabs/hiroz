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
        let rep = self.next_sub_rep;
        self.next_sub_rep += 1;

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
            use hiroz_protocol::{EndpointKind, Entity};
            let entities = self
                .engine
                .graph
                .lock()
                .get_entities_by_topic(EndpointKind::Service, &name);
            if let Some(ent) = entities.first() {
                if let Entity::Endpoint(ep) = ent.as_ref() {
                    if let Some(ti) = &ep.type_info {
                        let type_escaped = ti.name.replace('/', "%");
                        format!("{domain_id}/{svc_stripped}/{type_escaped}/{}", ti.hash)
                    } else {
                        let type_escaped = type_name.replace('/', "%");
                        format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
                    }
                } else {
                    let type_escaped = type_name.replace('/', "%");
                    format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
                }
            } else {
                let type_escaped = type_name.replace('/', "%");
                format!("{domain_id}/{svc_stripped}/{type_escaped}/**")
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
            },
        );
        Ok(Resource::new_own(rep))
    }

    fn measure_hz(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        self.require_perm(hu::plugin::types::Permission::MeasureMetrics)?;
        self.ensure_rate_tracker(&topic)?;
        let tracker = self.rate_trackers.get_mut(&topic).unwrap();
        tracker.drain_and_trim(window_ms);
        let count = tracker.arrivals.len() as f64;
        let window_s = window_ms as f64 / 1000.0;
        Ok(count / window_s)
    }

    fn measure_bw(&mut self, topic: String, window_ms: u32) -> Result<f64, String> {
        self.require_perm(hu::plugin::types::Permission::MeasureMetrics)?;
        self.ensure_rate_tracker(&topic)?;
        let tracker = self.rate_trackers.get_mut(&topic).unwrap();
        tracker.drain_and_trim(window_ms);
        let total_bytes: usize = tracker.arrivals.iter().map(|(_, b)| b).sum();
        let window_s = window_ms as f64 / 1000.0;
        Ok(total_bytes as f64 / 1024.0 / window_s)
    }

    fn encode_yaml_to_cdr(&mut self, yaml: String, type_name: String) -> Result<Vec<u8>, String> {
        self.require_perm(hu::plugin::types::Permission::PublishTopic)?;
        let schema = get_schema(&type_name)
            .ok_or_else(|| format!("schema for '{type_name}' not found in registry"))?;
        let value: serde_json::Value =
            serde_json::from_str(&yaml).map_err(|e| format!("failed to parse YAML/JSON: {e}"))?;
        let msg = json_to_dynamic_message(&value, &schema)?;
        serialize_cdr(&msg).map_err(|e| e.to_string())
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
        let req_type = data.type_name.clone();

        let req_schema =
            get_schema(&req_type).ok_or_else(|| format!("schema for '{req_type}' not found"))?;
        let req_value: serde_json::Value = serde_json::from_str(&request_json)
            .map_err(|e| format!("failed to parse request JSON: {e}"))?;
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

        let resp_type = req_type.replace("_Request", "_Response");
        if let Some(resp_schema) = get_schema(&resp_type) {
            match deserialize_cdr(&resp_cdr, &resp_schema) {
                Ok(msg) => return Ok(dynamic_message_to_json(&msg).to_string()),
                Err(e) => tracing::warn!("failed to decode service response: {e}"),
            }
        }
        Ok(format!(
            "{{\"raw\":\"{}\"}}",
            resp_cdr
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ))
    }

    fn drop(&mut self, res: Resource<hu::plugin::ros::ServiceClient>) -> wasmtime::Result<()> {
        self.service_clients.remove(&res.rep());
        Ok(())
    }
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
