//! Runtime schema registration/discovery and CDR codecs.
use std::{ffi::c_char, sync::Arc, time::Duration};

use serde_json::{Value, json};

use super::{
    ErrorCode, cstr_to_str,
    node::{CNode, get_node_ref},
    topic::json_string,
};
use crate::dynamic::{
    DynamicMessage, DynamicValue, FieldType, MessageSchema, MessageSchemaTypeDescription,
    type_description_msg_to_schema,
};

fn schema(text: &str) -> Result<Arc<MessageSchema>, String> {
    let description: hiroz_schema::TypeDescriptionMsg =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    let schema = type_description_msg_to_schema(&description).map_err(|e| e.to_string())?;
    crate::dynamic::limits::validate_schema(&schema)?;
    Ok(schema)
}
fn schema_json(schema: &MessageSchema, hash: Option<&str>) -> Result<Value, String> {
    crate::dynamic::limits::validate_schema(schema)?;
    let description = schema
        .to_type_description_msg()
        .map_err(|e| e.to_string())?;
    let hash = match hash {
        Some(hash) => hash.to_owned(),
        None => schema
            .compute_type_hash()
            .map_err(|e| e.to_string())?
            .to_rihs_string(),
    };
    Ok(json!({
        "Description": description,
        "TypeHash": hash,
    }))
}

/// Validate a ROS TypeDescription and return normalized description and RIHS hash.
/// # Safety
/// description is a valid C string; free result with hiroz_json_destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_schema_validate(description: *const c_char) -> *mut c_char {
    unsafe {
        let result = cstr_to_str(description)
            .map_err(|e| format!("{e:?}"))
            .and_then(schema)
            .and_then(|s| schema_json(&s, None));
        json_string(&match result {
            Ok(v) => v,
            Err(e) => json!({"Error":e}),
        })
    }
}

/// Register a schema in the node's type description service and process registry.
/// Returns an error if the node was built without that service.
/// # Safety
/// Node and description must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_register_schema(
    node: *mut CNode,
    description: *const c_char,
) -> i32 {
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return ErrorCode::NullPointer as i32;
        };
        let Ok(text) = cstr_to_str(description) else {
            return ErrorCode::InvalidUtf8 as i32;
        };
        let Ok(schema) = schema(text) else {
            return ErrorCode::DeserializationFailed as i32;
        };
        let Some(service) = node.type_description_service() else {
            return ErrorCode::ServiceCallFailed as i32;
        };
        if service.register_schema(schema.clone()).is_err() {
            return ErrorCode::ServiceCallFailed as i32;
        }
        crate::dynamic::register_schema(schema);
        0
    }
}

/// Discover topic or service schemas. Empty request_type selects a topic;
/// otherwise request_type and response_type are ROS schema names.
/// Timeout covers the entire discovery, including all type description calls.
/// # Safety
/// Node and C strings must remain live. Free result with hiroz_json_destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_discover_schema(
    node: *mut CNode,
    name: *const c_char,
    request_type: *const c_char,
    response_type: *const c_char,
    timeout_ms: u64,
) -> *mut c_char {
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        let (Ok(name), Ok(req), Ok(resp)) = (
            cstr_to_str(name),
            cstr_to_str(request_type),
            cstr_to_str(response_type),
        ) else {
            return std::ptr::null_mut();
        };
        // The C caller can itself be a callback on a Zenoh/Tokio worker. Run the
        // blocking facade on a scoped thread so nested runtimes never panic.
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| -> Result<Value, String> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    let duration = Duration::from_millis(timeout_ms);
                    runtime.block_on(async {
                        tokio::time::timeout(duration, async {
                            if req.is_empty() {
                                let discovered = node
                                    .discover_topic_schema(name, duration)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                let mut result =
                                    schema_json(&discovered.schema, Some(&discovered.type_hash))?;
                                result["TransportTypeName"] = json!(discovered.transport_type_name);
                                Ok(result)
                            } else {
                                let (request, response) = node
                                    .discover_service_schema(name, req, resp, duration)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                let request = schema_json(&request, None)?;
                                let response = schema_json(&response, None)?;
                                Ok(json!({
                                    "Request": request,
                                    "Response": response,
                                }))
                            }
                        })
                        .await
                        .map_err(|_| "schema discovery timed out".to_owned())?
                    })
                })
                .join()
                .unwrap_or_else(|_| Err("schema discovery worker failed".to_owned()))
        });
        json_string(&match result {
            Ok(v) => v,
            Err(e) => json!({"Error":e}),
        })
    }
}

fn message_from_json(s: &Arc<MessageSchema>, value: &Value) -> Result<DynamicMessage, String> {
    let object = value.as_object().ok_or("message must be a JSON object")?;
    for key in object.keys() {
        if s.field(key).is_none() {
            return Err(format!("unknown field: {key}"));
        }
    }
    let mut message = DynamicMessage::new(s);
    for (i, field) in s.fields.iter().enumerate() {
        if let Some(v) = object.get(&field.name) {
            message.values_mut()[i] = value_from_json(&field.field_type, v)?;
        }
    }
    Ok(message)
}
fn value_from_json(t: &FieldType, v: &Value) -> Result<DynamicValue, String> {
    use DynamicValue as D;
    use FieldType as F;
    macro_rules! signed {
        ($variant:ident,$ty:ty) => {
            D::$variant(
                <$ty>::try_from(v.as_i64().ok_or("expected signed integer")?)
                    .map_err(|_| "integer out of range")?,
            )
        };
    }
    macro_rules! unsigned {
        ($variant:ident,$ty:ty) => {
            D::$variant(
                <$ty>::try_from(v.as_u64().ok_or("expected unsigned integer")?)
                    .map_err(|_| "integer out of range")?,
            )
        };
    }
    Ok(match t {
        F::Bool => D::Bool(v.as_bool().ok_or("expected boolean")?),
        F::Int8 => signed!(Int8, i8),
        F::Int16 => signed!(Int16, i16),
        F::Int32 => signed!(Int32, i32),
        F::Int64 => signed!(Int64, i64),
        F::Byte | F::Uint8 => unsigned!(Uint8, u8),
        F::Uint16 => unsigned!(Uint16, u16),
        F::Uint32 => unsigned!(Uint32, u32),
        F::Uint64 => unsigned!(Uint64, u64),
        F::Float32 => {
            let n = v.as_f64().ok_or("expected number")? as f32;
            if !n.is_finite() {
                return Err("float32 out of range".into());
            }
            D::Float32(n)
        }
        F::Float64 => D::Float64(v.as_f64().ok_or("expected number")?),
        F::String | F::BoundedString(_) => {
            let text = v.as_str().ok_or("expected string")?;
            if let F::BoundedString(limit) = t {
                if text.len() > *limit {
                    return Err("bounded string too long".into());
                }
            }
            D::String(text.to_owned())
        }
        F::Message(s) => D::Message(Box::new(message_from_json(s, v)?)),
        F::Array(inner, len) | F::BoundedSequence(inner, len) => {
            let values = v.as_array().ok_or("expected array")?;
            if matches!(t, F::Array(..)) && values.len() != *len || values.len() > *len {
                return Err("array length outside schema bounds".into());
            }
            D::Array(
                values
                    .iter()
                    .map(|v| value_from_json(inner, v))
                    .collect::<Result<_, _>>()?,
            )
        }
        F::Sequence(inner) => D::Array(
            v.as_array()
                .ok_or("expected array")?
                .iter()
                .map(|v| value_from_json(inner, v))
                .collect::<Result<_, _>>()?,
        ),
    })
}
fn message_json(msg: &DynamicMessage) -> Result<Value, String> {
    let fields = msg
        .iter()
        .map(|(name, value)| Ok((name.to_owned(), value_json(value)?)))
        .collect::<Result<serde_json::Map<String, Value>, String>>()?;
    Ok(Value::Object(fields))
}
fn value_json(value: &DynamicValue) -> Result<Value, String> {
    use DynamicValue as D;
    Ok(match value {
        D::Bool(n) => json!(n),
        D::Int8(n) => json!(n),
        D::Int16(n) => json!(n),
        D::Int32(n) => json!(n),
        D::Int64(n) => json!(n),
        D::Uint8(n) => json!(n),
        D::Uint16(n) => json!(n),
        D::Uint32(n) => json!(n),
        D::Uint64(n) => json!(n),
        D::Float32(n) if n.is_finite() => json!(n),
        D::Float64(n) if n.is_finite() => json!(n),
        D::Float32(_) | D::Float64(_) => {
            return Err("JSON dynamic values require finite floats".to_owned());
        }
        D::String(n) => json!(n),
        D::Bytes(n) => json!(n),
        D::Array(values) => Value::Array(values.iter().map(value_json).collect::<Result<_, _>>()?),
        D::Message(message) => message_json(message)?,
    })
}

/// Encode a JSON message using the supplied ROS TypeDescription and native CDR codec.
/// # Safety
/// Strings/out pointers must be valid. Free bytes with hiroz_free_bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_dynamic_encode(
    description: *const c_char,
    values: *const c_char,
    out: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        if out.is_null() || out_len.is_null() {
            return ErrorCode::NullPointer as i32;
        }
        *out = std::ptr::null_mut();
        *out_len = 0;
        let result = (|| -> Result<Vec<u8>, String> {
            let text = cstr_to_str(description).map_err(|e| format!("{e:?}"))?;
            let values = cstr_to_str(values).map_err(|e| format!("{e:?}"))?;
            let s = schema(text)?;
            let value: Value = serde_json::from_str(values).map_err(|e| e.to_string())?;
            message_from_json(&s, &value)?
                .to_cdr()
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(bytes) => {
                let bytes = bytes.into_boxed_slice();
                *out_len = bytes.len();
                *out = Box::into_raw(bytes) as *mut u8;
                0
            }
            Err(e) => {
                tracing::warn!("Dynamic encoding failed: {e}");
                ErrorCode::SerializationFailed as i32
            }
        }
    }
}

/// Decode CDR to a JSON object with exact integer numbers.
/// # Safety
/// Description and data must be valid; free result with hiroz_json_destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_dynamic_decode(
    description: *const c_char,
    data: *const u8,
    len: usize,
) -> *mut c_char {
    unsafe {
        if data.is_null() {
            return std::ptr::null_mut();
        }
        let result = (|| -> Result<Value, String> {
            let text = cstr_to_str(description).map_err(|e| format!("{e:?}"))?;
            let s = schema(text)?;
            let msg = DynamicMessage::from_cdr(std::slice::from_raw_parts(data, len), &s)
                .map_err(|e| e.to_string())?;
            Ok(json!({"Values":message_json(&msg)?}))
        })();
        json_string(&match result {
            Ok(v) => v,
            Err(e) => json!({"Error":e}),
        })
    }
}

/// Return whether schema service registration is available on a node.
/// # Safety
/// Node must be valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_has_type_description_service(node: *mut CNode) -> bool {
    unsafe { get_node_ref(node).is_some_and(|n| n.has_type_description_service()) }
}
