//! FFI for graph introspection (node/topic/service discovery)

use super::context::{CContext, get_context_ref};
use super::{ErrorCode, cstr_to_str};
use std::ffi::{CString, c_char};

/// Topic info returned to FFI callers
#[repr(C)]
pub struct CTopicInfo {
    pub name: *mut c_char,
    pub type_name: *mut c_char,
}

/// Node info returned to FFI callers
#[repr(C)]
pub struct CNodeInfo {
    pub name: *mut c_char,
    pub namespace: *mut c_char,
}

/// Service info returned to FFI callers
#[repr(C)]
pub struct CServiceInfo {
    pub name: *mut c_char,
    pub type_name: *mut c_char,
}

/// Get all topic names and types
///
/// # Safety
/// `ctx` must be a valid context pointer. `out_topics` and `out_count` must be
/// valid non-null pointers. The returned array must be freed with `hiroz_graph_free_topics`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_get_topic_names_and_types(
    ctx: *mut CContext,
    out_topics: *mut *mut CTopicInfo,
    out_count: *mut usize,
) -> i32 {
    if out_topics.is_null() || out_count.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return ErrorCode::NullPointer as i32,
        };

        let topics = ctx_ref.graph().get_topic_names_and_types();

        let count = topics.len();
        if count == 0 {
            *out_topics = std::ptr::null_mut();
            *out_count = 0;
            return ErrorCode::Success as i32;
        }

        let layout = std::alloc::Layout::array::<CTopicInfo>(count).unwrap();
        let ptr = std::alloc::alloc(layout) as *mut CTopicInfo;
        if ptr.is_null() {
            return ErrorCode::Unknown as i32;
        }

        for (i, (name, type_name)) in topics.into_iter().enumerate() {
            let c_name = CString::new(name).unwrap_or_default();
            let c_type = CString::new(type_name).unwrap_or_default();
            std::ptr::write(
                ptr.add(i),
                CTopicInfo {
                    name: c_name.into_raw(),
                    type_name: c_type.into_raw(),
                },
            );
        }

        *out_topics = ptr;
        *out_count = count;
        ErrorCode::Success as i32
    }
}

/// Free topic info array
///
/// # Safety
/// `topics` must be a pointer returned by `hiroz_graph_get_topic_names_and_types`,
/// or null. `count` must match the count returned by that function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_free_topics(topics: *mut CTopicInfo, count: usize) {
    if topics.is_null() || count == 0 {
        return;
    }

    unsafe {
        for i in 0..count {
            let topic = &*topics.add(i);
            if !topic.name.is_null() {
                let _ = CString::from_raw(topic.name);
            }
            if !topic.type_name.is_null() {
                let _ = CString::from_raw(topic.type_name);
            }
        }
        let layout = std::alloc::Layout::array::<CTopicInfo>(count).unwrap();
        std::alloc::dealloc(topics as *mut u8, layout);
    }
}

/// Get all node names and namespaces
///
/// # Safety
/// `ctx` must be a valid context pointer. `out_nodes` and `out_count` must be
/// valid non-null pointers. The returned array must be freed with `hiroz_graph_free_nodes`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_get_node_names(
    ctx: *mut CContext,
    out_nodes: *mut *mut CNodeInfo,
    out_count: *mut usize,
) -> i32 {
    if out_nodes.is_null() || out_count.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return ErrorCode::NullPointer as i32,
        };

        let nodes = ctx_ref.graph().get_node_names();

        let count = nodes.len();
        if count == 0 {
            *out_nodes = std::ptr::null_mut();
            *out_count = 0;
            return ErrorCode::Success as i32;
        }

        let layout = std::alloc::Layout::array::<CNodeInfo>(count).unwrap();
        let ptr = std::alloc::alloc(layout) as *mut CNodeInfo;
        if ptr.is_null() {
            return ErrorCode::Unknown as i32;
        }

        for (i, (name, namespace)) in nodes.into_iter().enumerate() {
            let c_name = CString::new(name).unwrap_or_default();
            let c_ns = CString::new(namespace).unwrap_or_default();
            std::ptr::write(
                ptr.add(i),
                CNodeInfo {
                    name: c_name.into_raw(),
                    namespace: c_ns.into_raw(),
                },
            );
        }

        *out_nodes = ptr;
        *out_count = count;
        ErrorCode::Success as i32
    }
}

/// Free node info array
///
/// # Safety
/// `nodes` must be a pointer returned by `hiroz_graph_get_node_names`,
/// or null. `count` must match the count returned by that function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_free_nodes(nodes: *mut CNodeInfo, count: usize) {
    if nodes.is_null() || count == 0 {
        return;
    }

    unsafe {
        for i in 0..count {
            let node = &*nodes.add(i);
            if !node.name.is_null() {
                let _ = CString::from_raw(node.name);
            }
            if !node.namespace.is_null() {
                let _ = CString::from_raw(node.namespace);
            }
        }
        let layout = std::alloc::Layout::array::<CNodeInfo>(count).unwrap();
        std::alloc::dealloc(nodes as *mut u8, layout);
    }
}

/// Get all service names and types
///
/// # Safety
/// `ctx` must be a valid context pointer. `out_services` and `out_count` must be
/// valid non-null pointers. The returned array must be freed with `hiroz_graph_free_services`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_get_service_names_and_types(
    ctx: *mut CContext,
    out_services: *mut *mut CServiceInfo,
    out_count: *mut usize,
) -> i32 {
    if out_services.is_null() || out_count.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return ErrorCode::NullPointer as i32,
        };

        let services = ctx_ref.graph().get_service_names_and_types();

        let count = services.len();
        if count == 0 {
            *out_services = std::ptr::null_mut();
            *out_count = 0;
            return ErrorCode::Success as i32;
        }

        let layout = std::alloc::Layout::array::<CServiceInfo>(count).unwrap();
        let ptr = std::alloc::alloc(layout) as *mut CServiceInfo;
        if ptr.is_null() {
            return ErrorCode::Unknown as i32;
        }

        for (i, (name, type_name)) in services.into_iter().enumerate() {
            let c_name = CString::new(name).unwrap_or_default();
            let c_type = CString::new(type_name).unwrap_or_default();
            std::ptr::write(
                ptr.add(i),
                CServiceInfo {
                    name: c_name.into_raw(),
                    type_name: c_type.into_raw(),
                },
            );
        }

        *out_services = ptr;
        *out_count = count;
        ErrorCode::Success as i32
    }
}

/// Free service info array
///
/// # Safety
/// `services` must be a pointer returned by `hiroz_graph_get_service_names_and_types`,
/// or null. `count` must match the count returned by that function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_free_services(services: *mut CServiceInfo, count: usize) {
    if services.is_null() || count == 0 {
        return;
    }

    unsafe {
        for i in 0..count {
            let svc = &*services.add(i);
            if !svc.name.is_null() {
                let _ = CString::from_raw(svc.name);
            }
            if !svc.type_name.is_null() {
                let _ = CString::from_raw(svc.type_name);
            }
        }
        let layout = std::alloc::Layout::array::<CServiceInfo>(count).unwrap();
        std::alloc::dealloc(services as *mut u8, layout);
    }
}

/// Check if a node exists in the graph
///
/// # Safety
/// `ctx` must be a valid context pointer. `name` must be a valid C string.
/// `namespace` may be null (defaults to "/").
/// Returns 1 if found, 0 if not found, or negative error code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_graph_node_exists(
    ctx: *mut CContext,
    name: *const c_char,
    namespace: *const c_char,
) -> i32 {
    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return ErrorCode::NullPointer as i32,
        };

        let name_str = match cstr_to_str(name) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidUtf8 as i32,
        };

        let namespace_str = if namespace.is_null() {
            "/"
        } else {
            match cstr_to_str(namespace) {
                Ok(s) => s,
                Err(_) => return ErrorCode::InvalidUtf8 as i32,
            }
        };

        let node_key: crate::entity::NodeKey = (namespace_str.to_string(), name_str.to_string());

        if ctx_ref.graph().node_exists(node_key) {
            1
        } else {
            0
        }
    }
}
