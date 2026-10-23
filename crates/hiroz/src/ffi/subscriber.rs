use super::node::{CNode, get_node_ref};
use super::qos::CQosProfile;
use super::{ErrorCode, cstr_to_str};
use std::ffi::c_char;
use zenoh_ext::AdvancedSubscriber;

/// Callback type for receiving messages
pub type MessageCallback = extern "C" fn(user_data: usize, data: *const u8, len: usize);

/// Raw subscriber wrapper that keeps the zenoh subscriptions and the liveliness token alive.
///
/// Dropping it stops them all. [`RawSubscriber::undeclare`] does the same and returns the first error.
pub struct RawSubscriber {
    /// Declared first, so it drops first: the token goes before the subscriptions it announces.
    token: Option<zenoh::liveliness::LivelinessToken>,
    base: AdvancedSubscriber<()>,
    /// The exact subscription on the group key, when the subscriber joins.
    group: Option<zenoh::pubsub::Subscriber<()>>,
}

impl RawSubscriber {
    pub(crate) fn new(
        token: Option<zenoh::liveliness::LivelinessToken>,
        base: AdvancedSubscriber<()>,
        group: Option<zenoh::pubsub::Subscriber<()>>,
    ) -> Self {
        Self { token, base, group }
    }

    /// Stop the whole subscription: the token first, then the subscription on the base key, then
    /// the subscription on the group key. Every handle is undeclared even when an earlier one
    /// fails. The result is the first error.
    pub fn undeclare(self) -> zenoh::Result<()> {
        use zenoh::Wait;
        let Self { token, base, group } = self;
        [
            token.map_or(Ok(()), |t| t.undeclare().wait()),
            base.undeclare().wait(),
            group.map_or(Ok(()), |g| g.undeclare().wait()),
        ]
        .into_iter()
        .collect()
    }
}

/// Opaque subscriber handle for FFI
#[repr(C)]
pub struct CSubscriber {
    inner: Box<RawSubscriber>,
}

/// Create a subscriber with callback (default QoS)
///
/// The subscriber joins rmw_zenoh's CPU buffer group, as `hiroz_subscriber_create_with_group`
/// does with group `2`.
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
/// The subscriber joins rmw_zenoh's CPU buffer group, as `hiroz_subscriber_create_with_group`
/// does with group `2`. Use that function with group `0` to stay out.
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
        create_subscriber(
            node,
            topic,
            type_name,
            type_hash,
            callback,
            user_data,
            qos,
            crate::BufferGroup::Auto,
        )
    }
}

/// Create a subscriber with callback, QoS profile and a buffer group choice
///
/// `group` is `0` to stay out of rmw_zenoh's CPU buffer group, `1` to join it, or `2` to
/// join it as `hiroz_subscriber_create_with_qos` does. A raw subscriber has no message
/// definition, so `1` and `2` join the same way. A subscriber that joins announces
/// `backends:cpu:` and receives from both the base key and the group key. Any other value is
/// an error: the function returns null and logs a warning. A QoS that rmw_zenoh does not
/// serve on the group key (transient-local) stays out whatever `group` says.
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. `callback` must be a valid function pointer.
/// `qos` may be null for default QoS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_create_with_group(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
    qos: *const CQosProfile,
    group: i32,
) -> *mut CSubscriber {
    let group = match group {
        0 => crate::BufferGroup::Off,
        1 => crate::BufferGroup::Cpu,
        2 => crate::BufferGroup::Auto,
        other => {
            tracing::warn!(
                "hiroz: Unknown buffer group {} for subscriber (expected 0, 1 or 2)",
                other
            );
            return std::ptr::null_mut();
        }
    };
    unsafe {
        create_subscriber(
            node, topic, type_name, type_hash, callback, user_data, qos, group,
        )
    }
}

/// The body shared by the two `with_` creators.
///
/// # Safety
/// The same as `hiroz_subscriber_create_with_qos`.
#[allow(clippy::too_many_arguments)]
unsafe fn create_subscriber(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
    qos: *const CQosProfile,
    group: crate::BufferGroup,
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

        match node_ref.create_raw_subscriber_with_group(
            topic_str,
            type_name_str,
            type_hash_str,
            move |data: &[u8]| {
                callback(user_data, data.as_ptr(), data.len());
            },
            qos_profile,
            group,
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
