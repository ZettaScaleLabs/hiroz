use std::{
    ffi::c_char,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use super::{
    ErrorCode, cstr_to_str,
    node::{CNode, get_node_ref},
    qos::CQosProfile,
};
use crate::pubsub::SubscriberTransport;

/// Callback type for receiving messages
pub type MessageCallback = extern "C" fn(user_data: usize, data: *const u8, len: usize);

/// Called once after the subscriber's last callback reference is released.
pub type SubscriberDropCallback = extern "C" fn(user_data: usize);

/// The callback closure and the handle share ownership, keeping foreign callback
/// resources alive until any delivery already in progress has finished.
pub(crate) struct SubscriberCallbackLifetime {
    user_data: usize,
    drop_callback: Option<SubscriberDropCallback>,
    open: AtomicBool,
    active_deliveries: AtomicUsize,
}

struct SubscriberDelivery<'a>(&'a AtomicUsize);

impl Drop for SubscriberDelivery<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SubscriberCallbackLifetime {
    pub(crate) fn new(
        user_data: usize,
        drop_callback: Option<SubscriberDropCallback>,
    ) -> Arc<Self> {
        Arc::new(Self {
            user_data,
            drop_callback,
            open: AtomicBool::new(true),
            active_deliveries: AtomicUsize::new(0),
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub(crate) fn deliver(&self, callback: impl FnOnce()) {
        // Register before checking open. Together with destroy's SeqCst
        // operations, a callback that observes open cannot be missed by close.
        self.active_deliveries.fetch_add(1, Ordering::SeqCst);
        let _active = SubscriberDelivery(&self.active_deliveries);
        if self.is_open() {
            callback();
        }
    }
}

impl Drop for SubscriberCallbackLifetime {
    fn drop(&mut self) {
        if let Some(drop_callback) = self.drop_callback {
            drop_callback(self.user_data);
        }
    }
}

/// Owns discovery, both transport subscriptions, and their parent session.
pub struct RawSubscriber {
    pub(crate) token: Option<zenoh::liveliness::LivelinessToken>,
    pub(crate) inner: SubscriberTransport,
    pub(crate) group: Option<zenoh::pubsub::Subscriber<()>>,
    pub(crate) entity: crate::entity::EndpointEntity,
    pub(crate) graph: Arc<crate::graph::Graph>,
    // Keep weak transport references usable until all endpoint fields drop.
    pub(crate) _session: Arc<zenoh::Session>,
}

impl RawSubscriber {
    /// Undeclare the token first, then both subscriptions, retaining the session
    /// until all teardown has completed. Return the first error after trying all handles.
    pub fn undeclare(self) -> zenoh::Result<()> {
        use zenoh::Wait;
        let Self {
            token,
            inner,
            group,
            entity: _,
            graph: _graph,
            _session,
        } = self;
        let token_result = token.map_or(Ok(()), |t| t.undeclare().wait());
        let base_result = match inner {
            SubscriberTransport::Volatile(sub) => sub.undeclare().wait(),
            SubscriberTransport::TransientLocal(sub) => sub.undeclare().wait(),
        };
        let group_result = group.map_or(Ok(()), |g| g.undeclare().wait());
        [token_result, base_result, group_result]
            .into_iter()
            .collect()
    }
}

/// Opaque subscriber handle for FFI
pub struct CSubscriber {
    pub(crate) inner: Box<RawSubscriber>,
    pub(crate) callbacks: Arc<SubscriberCallbackLifetime>,
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
        create_subscriber_owned(
            node, topic, type_name, type_hash, callback, user_data, qos, None,
        )
    }
}

/// Create a subscriber, taking ownership of its foreign callback resources.
/// `drop_callback` runs exactly once, including when creation fails. It runs only
/// after every callback that was already in progress has returned. Destroying
/// the subscriber disables new delivery but does not wait for a running callback,
/// allowing a callback to destroy its own subscriber.
///
/// # Safety
/// Arguments have the same requirements as `hiroz_subscriber_create_with_qos`.
/// `user_data` must remain valid until `drop_callback` is called, which may occur
/// on any thread. The caller must not release it separately after calling this.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_subscriber_create_owned(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
    qos: *const CQosProfile,
    drop_callback: SubscriberDropCallback,
) -> *mut CSubscriber {
    unsafe {
        create_subscriber_owned(
            node,
            topic,
            type_name,
            type_hash,
            callback,
            user_data,
            qos,
            Some(drop_callback),
        )
    }
}

unsafe fn create_subscriber_owned(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    callback: MessageCallback,
    user_data: usize,
    qos: *const CQosProfile,
    drop_callback: Option<SubscriberDropCallback>,
) -> *mut CSubscriber {
    let callbacks = SubscriberCallbackLifetime::new(user_data, drop_callback);
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

        let callback_lifetime = callbacks.clone();
        match node_ref.create_raw_subscriber_with_options(
            topic_str,
            type_name_str,
            type_hash_str,
            move |sample: zenoh::sample::Sample| {
                let data = sample.payload().to_bytes();
                let lifetime = callback_lifetime.clone();
                lifetime.deliver(|| {
                    callback(user_data, data.as_ptr(), data.len());
                });
            },
            qos_profile,
            zenoh::sample::Locality::Any,
        ) {
            Ok(raw_sub) => Box::into_raw(Box::new(CSubscriber {
                inner: Box::new(raw_sub),
                callbacks,
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
        let sub = Box::from_raw(sub);
        sub.callbacks.open.store(false, Ordering::SeqCst);
        if sub.callbacks.drop_callback.is_some()
            && sub.callbacks.active_deliveries.load(Ordering::SeqCst) != 0
        {
            // AdvancedSubscriber history/recovery callbacks hold a transport
            // state mutex outside our closure. Dropping it from that callback
            // would recursively acquire the mutex. The worker may wait for the
            // entire transport callback to unwind; the foreign callback cannot.
            // Keep legacy and inactive destruction synchronous.
            std::thread::spawn(move || drop(sub));
        } else {
            drop(sub);
        }
    }
    ErrorCode::Success as i32
}
