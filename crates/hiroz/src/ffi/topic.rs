//! Topic options, serialized samples, and graph inspection for language bindings.
use std::{
    ffi::{CString, c_char},
    sync::Arc,
};

use serde_json::{Value, json};

use super::{
    ErrorCode,
    context::{CContext, get_context_ref},
    cstr_to_str,
    node::{CNode, get_node_ref},
    publisher::CPublisher,
    qos::CQosProfile,
};
use crate::entity::{EndpointEntity, EndpointKind, Entity};

pub(crate) fn endpoint_json(ep: &EndpointEntity) -> Value {
    use hiroz_protocol::qos::{QosHistory, QosReliability};
    let (history, depth) = match ep.qos.history {
        QosHistory::KeepLast(n) => (0, n),
        QosHistory::KeepAll => (1, 0),
    };
    json!({"ID":ep.id, "Kind": match ep.kind { EndpointKind::Publisher => 0, EndpointKind::Subscription => 1, EndpointKind::Service => 2, EndpointKind::Client => 3 },
        "Name":ep.topic, "TypeName":ep.type_info.as_ref().map(|t| &t.name),
        "TypeHash":ep.type_info.as_ref().map(|t| t.hash.to_rihs_string()),
        "GID":crate::entity::endpoint_gid(ep),
        "Node":ep.node.as_ref().map(|n| json!({"Name":n.name,"Namespace":n.namespace,"Enclave":n.enclave,"SessionID":n.z_id.to_string()})),
        "QoS":{"Reliability":if ep.qos.reliability == QosReliability::Reliable {0} else {1}, "Durability":ep.qos.durability as u8,"History":history,"HistoryDepth":depth}})
}

pub(crate) fn json_string(value: &Value) -> *mut c_char {
    CString::new(value.to_string()).unwrap().into_raw()
}

/// Free a JSON string returned by the topic, dynamic, or graph inspection API.
/// # Safety
/// `value` must be null or a string returned by one of those functions, once only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_json_destroy(value: *mut c_char) {
    if !value.is_null() {
        unsafe {
            drop(CString::from_raw(value));
        }
    }
}

fn kind(value: i32) -> Option<EndpointKind> {
    match value {
        0 => Some(EndpointKind::Publisher),
        1 => Some(EndpointKind::Subscription),
        2 => Some(EndpointKind::Service),
        3 => Some(EndpointKind::Client),
        _ => None,
    }
}

/// Inspect endpoints. Empty node name queries by fully qualified topic/service;
/// nonempty node name queries by node (namespace `/` is normalized).
/// # Safety
/// All pointers must be live; name, node_name and namespace are C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_endpoints_json(
    ctx: *mut CContext,
    endpoint_kind: i32,
    name: *const c_char,
    node_name: *const c_char,
    namespace: *const c_char,
) -> *mut c_char {
    unsafe {
        let Some(ctx) = get_context_ref(ctx) else {
            return std::ptr::null_mut();
        };
        let Some(kind) = kind(endpoint_kind) else {
            return std::ptr::null_mut();
        };
        let (Ok(name), Ok(node), Ok(ns)) = (
            cstr_to_str(name),
            cstr_to_str(node_name),
            cstr_to_str(namespace),
        ) else {
            return std::ptr::null_mut();
        };
        let graph = ctx.graph();
        let endpoints: Vec<Value> = if !node.is_empty() {
            graph
                .get_entities_by_node(
                    kind,
                    (crate::entity::normalize_node_namespace(ns), node.to_owned()),
                )
                .iter()
                .map(endpoint_json)
                .collect()
        } else {
            let entities = match kind {
                EndpointKind::Publisher | EndpointKind::Subscription => {
                    graph.get_entities_by_topic(kind, name)
                }
                _ => graph.get_entities_by_service(kind, name),
            };
            entities
                .iter()
                .filter_map(|e| match &**e {
                    Entity::Endpoint(e) => Some(endpoint_json(e)),
                    _ => None,
                })
                .collect()
        };
        json_string(&json!(endpoints))
    }
}

/// Action and node discovery. kind: 0=all actions, 1=action clients by node,
/// 2=action servers by node, 3=nodes including enclave.
/// # Safety
/// Pointers must remain valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_details_json(
    ctx: *mut CContext,
    kind: i32,
    node_name: *const c_char,
    namespace: *const c_char,
) -> *mut c_char {
    unsafe {
        let Some(ctx) = get_context_ref(ctx) else {
            return std::ptr::null_mut();
        };
        let (Ok(node), Ok(ns)) = (cstr_to_str(node_name), cstr_to_str(namespace)) else {
            return std::ptr::null_mut();
        };
        let graph = ctx.graph();
        if kind == 3 {
            return json_string(&json!(
                graph
                    .get_node_names_with_enclaves()
                    .iter()
                    .map(
                        |(name, ns, enclave)| json!({"Name":name,"Namespace":ns,"Enclave":enclave})
                    )
                    .collect::<Vec<_>>()
            ));
        }
        let key = (crate::entity::normalize_node_namespace(ns), node.to_owned());
        let values = match kind {
            0 => graph.get_action_names_and_types(),
            1 => graph.get_action_client_names_and_types_by_node(key),
            2 => graph.get_action_server_names_and_types_by_node(key),
            _ => return std::ptr::null_mut(),
        };
        json_string(&json!(
            values
                .iter()
                .map(|(name, ty)| json!({"Name":name,"TypeName":ty}))
                .collect::<Vec<_>>()
        ))
    }
}

/// Publisher transport options. shm_mode: 0=inherit, 1=disabled, 2=private pool.
#[repr(C)]
pub struct CPublisherOptions {
    pub encoding: *const c_char,
    pub shm_mode: i32,
    pub shm_pool_bytes: usize,
    pub shm_threshold: usize,
}

/// Create a publisher with transport options.
/// # Safety
/// Node and strings must be valid; qos and options may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_create_with_options(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    qos: *const CQosProfile,
    options: *const CPublisherOptions,
) -> *mut CPublisher {
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        let (Ok(topic), Ok(name), Ok(hash)) = (
            cstr_to_str(topic),
            cstr_to_str(type_name),
            cstr_to_str(type_hash),
        ) else {
            return std::ptr::null_mut();
        };
        let (mut encoding, mut shm) = (None, None);
        if let Some(opt) = options.as_ref() {
            if !opt.encoding.is_null() {
                let Ok(enc) = cstr_to_str(opt.encoding) else {
                    return std::ptr::null_mut();
                };
                if !enc.is_empty() {
                    encoding = Some(zenoh::bytes::Encoding::from(enc.to_owned()));
                }
            }
            shm = match opt.shm_mode {
                0 => None,
                1 => Some(None),
                2 => {
                    if opt.shm_pool_bytes == 0 {
                        return std::ptr::null_mut();
                    }
                    let Ok(provider) =
                        crate::shm::ShmProviderBuilder::new(opt.shm_pool_bytes).build()
                    else {
                        return std::ptr::null_mut();
                    };
                    Some(Some(Arc::new(
                        crate::shm::ShmConfig::new(Arc::new(provider))
                            .with_threshold(opt.shm_threshold),
                    )))
                }
                _ => return std::ptr::null_mut(),
            };
        }
        match node.create_raw_publisher_with_options(
            topic,
            name,
            hash,
            CQosProfile::to_qos_profile(qos),
            encoding,
            shm,
        ) {
            Ok(inner) => Box::into_raw(Box::new(CPublisher {
                inner: Box::new(inner),
            })),
            Err(e) => {
                tracing::warn!("Publisher creation failed: {e}");
                std::ptr::null_mut()
            }
        }
    }
}

/// Publish serialized data with optional attachment, encoding, and Zenoh timestamp.
/// A null attachment generates normal ROS sequence/source identity metadata; a
/// nonnull attachment (including an empty one) is preserved exactly.
/// # Safety
/// The publisher, strings, data and optional attachment must be live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_publish_sample(
    publisher: *mut CPublisher,
    data: *const u8,
    len: usize,
    attachment: *const u8,
    attachment_len: usize,
    encoding: *const c_char,
    timestamp: *const c_char,
) -> i32 {
    unsafe {
        let Some(p) = publisher.as_ref() else {
            return ErrorCode::NullPointer as i32;
        };
        if data.is_null() && len != 0 {
            return ErrorCode::NullPointer as i32;
        }
        let data = if len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data, len)
        };
        let attachment = if attachment.is_null() {
            None
        } else {
            Some(zenoh::bytes::ZBytes::from(
                std::slice::from_raw_parts(attachment, attachment_len).to_vec(),
            ))
        };
        let encoding = if encoding.is_null() {
            None
        } else {
            let Ok(s) = cstr_to_str(encoding) else {
                return ErrorCode::InvalidUtf8 as i32;
            };
            Some(zenoh::bytes::Encoding::from(s.to_owned()))
        };
        let timestamp = if timestamp.is_null() {
            None
        } else {
            let Ok(s) = cstr_to_str(timestamp) else {
                return ErrorCode::InvalidUtf8 as i32;
            };
            let Ok(ts) = s.parse() else {
                return ErrorCode::SerializationFailed as i32;
            };
            Some(ts)
        };
        match p
            .inner
            .publish_sample(data, attachment, encoding, timestamp)
        {
            Ok(()) => 0,
            Err(_) => ErrorCode::PublishFailed as i32,
        }
    }
}

/// Inspect publisher metadata and currently discovered subscriptions.
/// # Safety
/// Publisher must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_info_json(publisher: *mut CPublisher) -> *mut c_char {
    unsafe {
        let Some(p) = publisher.as_ref() else {
            return std::ptr::null_mut();
        };
        json_string(
            &json!({"Endpoint":endpoint_json(&p.inner.entity),"PeerCount":p.inner.graph.count(EndpointKind::Subscription,&p.inner.entity.topic)}),
        )
    }
}

/// Sample delivery includes the raw payload/attachment and JSON metadata. All
/// pointers are borrowed until the callback returns; copy before retaining them.
pub type SampleCallback = extern "C" fn(usize, *const u8, usize, *const u8, usize, *const c_char);

/// Subscribe with sample metadata and locality (0=any, 1=session local, 2=remote).
/// Foreign callback ownership is transferred even when creation fails.
/// # Safety
/// Node, strings and callbacks must be valid; qos may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_create_samples(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: SampleCallback,
    user_data: usize,
    qos: *const CQosProfile,
    locality: i32,
    drop_callback: super::subscriber::SubscriberDropCallback,
) -> *mut super::subscriber::CSubscriber {
    use super::subscriber::{CSubscriber, SubscriberCallbackLifetime};
    let lifetime = SubscriberCallbackLifetime::new(user_data, Some(drop_callback));
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        let (Ok(topic), Ok(name), Ok(hash)) = (
            cstr_to_str(topic),
            cstr_to_str(type_name),
            cstr_to_str(type_hash),
        ) else {
            return std::ptr::null_mut();
        };
        let locality = match locality {
            0 => zenoh::sample::Locality::Any,
            1 => zenoh::sample::Locality::SessionLocal,
            2 => zenoh::sample::Locality::Remote,
            _ => return std::ptr::null_mut(),
        };
        let guard = lifetime.clone();
        let result=node.create_raw_subscriber_with_options(topic,name,hash,move |sample| {
  guard.deliver(|| {
  let payload=sample.payload().to_bytes();let raw_attachment=sample.attachment().map(|a|a.to_bytes());
  let attachment=sample.attachment().and_then(|a|crate::attachment::Attachment::try_from(a).ok());
  let metadata=json!({"Encoding":sample.encoding().to_string(),"Timestamp":sample.timestamp().map(|t|t.to_string()),
   "SequenceNumber":attachment.as_ref().map(|a|a.sequence_number),"SourceTimestamp":attachment.as_ref().map(|a|a.source_timestamp),"PublisherGID":attachment.as_ref().map(|a|a.source_gid)});
  let metadata=CString::new(metadata.to_string()).unwrap();
  let (att_ptr,att_len)=raw_attachment.as_ref().map(|a|(a.as_ptr(),a.len())).unwrap_or((std::ptr::null(),0));
  callback(user_data,payload.as_ptr(),payload.len(),att_ptr,att_len,metadata.as_ptr());
  });
 },CQosProfile::to_qos_profile(qos),locality);
        match result {
            Ok(inner) => Box::into_raw(Box::new(CSubscriber {
                inner: Box::new(inner),
                callbacks: lifetime,
            })),
            Err(e) => {
                tracing::warn!("Sample subscriber creation failed: {e}");
                std::ptr::null_mut()
            }
        }
    }
}

/// Inspect subscriber metadata and currently discovered publishers.
/// # Safety
/// Subscriber must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_info_json(
    subscriber: *mut super::subscriber::CSubscriber,
) -> *mut c_char {
    unsafe {
        let Some(s) = subscriber.as_ref() else {
            return std::ptr::null_mut();
        };
        json_string(
            &json!({"Endpoint":endpoint_json(&s.inner.entity),"PeerCount":s.inner.graph.count(EndpointKind::Publisher,&s.inner.entity.topic)}),
        )
    }
}
