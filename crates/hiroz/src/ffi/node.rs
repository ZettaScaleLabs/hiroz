use super::context::{CContext, get_context_ref};
use super::{ErrorCode, cstr_to_str};
use crate::Builder;
use crate::node::ZNode;
use std::ffi::c_char;

/// Opaque node handle for FFI
#[repr(C)]
pub struct CNode {
    inner: Box<ZNode>,
}

/// Node configuration for FFI
#[repr(C)]
pub struct CNodeConfig {
    pub name: *const c_char,
    pub namespace: *const c_char,
    pub enable_type_description_service: bool,
}

/// Create a new node (simple API)
///
/// # Safety
/// `ctx` must be a valid context pointer. `name` must be a valid C string.
/// `namespace` may be null. The returned pointer must be freed with `hiroz_node_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_create(
    ctx: *mut CContext,
    name: *const c_char,
    namespace: *const c_char,
) -> *mut CNode {
    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return std::ptr::null_mut(),
        };

        let name_str = match cstr_to_str(name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let mut builder = ctx_ref.create_node(name_str);
        if !namespace.is_null() {
            let namespace_str = match cstr_to_str(namespace) {
                Ok(s) => s,
                Err(_) => return std::ptr::null_mut(),
            };
            builder = builder.with_namespace(namespace_str);
        }

        match builder.build() {
            Ok(node) => Box::into_raw(Box::new(CNode {
                inner: Box::new(node),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create node: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Create a new node with full configuration
///
/// # Safety
/// `ctx` must be a valid context pointer. `config` must be a valid pointer to
/// `CNodeConfig` or null. String fields in config must be valid C strings or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_create_with_config(
    ctx: *mut CContext,
    config: *const CNodeConfig,
) -> *mut CNode {
    if config.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let ctx_ref = match get_context_ref(ctx) {
            Some(c) => c,
            None => return std::ptr::null_mut(),
        };

        let cfg = &*config;

        let name_str = match cstr_to_str(cfg.name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let mut builder = ctx_ref.create_node(name_str);
        if !cfg.namespace.is_null() {
            let namespace_str = match cstr_to_str(cfg.namespace) {
                Ok(s) => s,
                Err(_) => return std::ptr::null_mut(),
            };
            builder = builder.with_namespace(namespace_str);
        }

        if cfg.enable_type_description_service {
            builder = builder.with_type_description_service();
        }

        match builder.build() {
            Ok(node) => Box::into_raw(Box::new(CNode {
                inner: Box::new(node),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create node: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Destroy a node
///
/// # Safety
/// `node` must be a valid pointer returned by `hiroz_node_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_node_destroy(node: *mut CNode) -> i32 {
    if node.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(node);
    }
    ErrorCode::Success as i32
}

pub(crate) unsafe fn get_node_ref<'a>(node: *mut CNode) -> Option<&'a ZNode> {
    if node.is_null() {
        None
    } else {
        unsafe { Some(&(*node).inner) }
    }
}
