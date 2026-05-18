use super::node::{CNode, get_node_ref};
use super::qos::CQosProfile;
use super::{ErrorCode, cstr_to_str};
use std::ffi::c_char;
use zenoh_ext::AdvancedSubscriber;

/// Callback type for receiving messages
pub type MessageCallback = extern "C" fn(user_data: usize, data: *const u8, len: usize);

/// Raw subscriber wrapper that keeps the zenoh subscriber alive
pub struct RawSubscriber {
    pub inner: AdvancedSubscriber<()>,
}

/// Opaque subscriber handle for FFI
#[repr(C)]
pub struct CSubscriber {
    inner: Box<RawSubscriber>,
}

/// Create a subscriber with callback (default QoS)
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. `callback` must be a valid function pointer.
/// `user_data` is passed through to the callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_create(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
) -> *mut CSubscriber {
    unsafe {
        hiroz_subscriber_create_with_qos(
            node,
            topic,
            type_name,
            type_hash,
            callback,
            user_data,
            std::ptr::null(),
        )
    }
}

/// Create a subscriber with callback and QoS profile
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. `callback` must be a valid function pointer.
/// `qos` may be null for default QoS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_create_with_qos(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
    qos: *const CQosProfile,
) -> *mut CSubscriber {
    unsafe {
        let node_ref = match get_node_ref(node) {
            Some(n) => n,
            None => return std::ptr::null_mut(),
        };

        let topic_str = match cstr_to_str(topic) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_name_str = match cstr_to_str(type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_hash_str = match cstr_to_str(type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let qos_profile = CQosProfile::to_qos_profile(qos);

        match node_ref.create_raw_subscriber_with_qos(
            topic_str,
            type_name_str,
            type_hash_str,
            move |data: &[u8]| {
                callback(user_data, data.as_ptr(), data.len());
            },
            qos_profile,
        ) {
            Ok(raw_sub) => Box::into_raw(Box::new(CSubscriber {
                inner: Box::new(raw_sub),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create subscriber: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Destroy a subscriber
///
/// # Safety
/// `sub` must be a valid subscriber pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_destroy(sub: *mut CSubscriber) -> i32 {
    if sub.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(sub);
    }
    ErrorCode::Success as i32
}
