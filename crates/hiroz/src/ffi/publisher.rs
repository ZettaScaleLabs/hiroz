use super::node::{CNode, get_node_ref};
use super::qos::{CQosProfile, CQosProfileV2};
use super::{ErrorCode, cstr_to_str};
use crate::attachment::{Attachment, GidArray};
use std::ffi::c_char;
use std::sync::atomic::{AtomicUsize, Ordering};
use zenoh::Wait;
use zenoh_ext::AdvancedPublisher;

/// Raw publisher for FFI (no type parameters)
pub struct RawPublisher {
    pub inner: AdvancedPublisher<'static>,
    sn: AtomicUsize,
    gid: GidArray,
    /// Liveliness token — kept alive so that rmw_zenoh_cpp subscribers can
    /// discover this publisher via Zenoh liveliness.
    pub(crate) _lv_token: zenoh::liveliness::LivelinessToken,
}

impl RawPublisher {
    pub fn new(
        publisher: AdvancedPublisher<'static>,
        gid: GidArray,
        lv_token: zenoh::liveliness::LivelinessToken,
    ) -> Self {
        Self {
            inner: publisher,
            sn: AtomicUsize::new(0),
            gid,
            _lv_token: lv_token,
        }
    }

    fn new_attachment(&self) -> Attachment {
        Attachment::new(self.sn.fetch_add(1, Ordering::AcqRel) as _, self.gid)
    }

    pub fn publish_bytes(&self, data: &[u8]) -> Result<(), zenoh::Error> {
        self.inner
            .put(data)
            .attachment(self.new_attachment())
            .wait()?;
        Ok(())
    }
}

/// Opaque publisher handle for FFI
#[repr(C)]
pub struct CPublisher {
    inner: Box<RawPublisher>,
}

/// Create a publisher (default QoS)
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. The returned pointer must be freed with `hiroz_publisher_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_create(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
) -> *mut CPublisher {
    unsafe { hiroz_publisher_create_with_qos(node, topic, type_name, type_hash, std::ptr::null()) }
}

/// Create a publisher with QoS profile
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. `qos` may be null for default QoS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_create_with_qos(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    qos: *const CQosProfile,
) -> *mut CPublisher {
    unsafe {
        create_publisher(
            node,
            topic,
            type_name,
            type_hash,
            CQosProfile::to_qos_profile(qos),
        )
    }
}

/// Create a publisher with a versioned QoS profile
///
/// The same as `hiroz_publisher_create_with_qos`, except that `qos` points to a
/// `hiroz_qos_profile_v2_t`. Returns null if `qos` has a `struct_size` below 4.
///
/// # Safety
/// `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
/// must be valid C strings. `qos` may be null for default QoS. Otherwise it must point to
/// `struct_size` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_create_with_qos_v2(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    qos: *const CQosProfileV2,
) -> *mut CPublisher {
    unsafe {
        match CQosProfileV2::to_qos_profile(qos) {
            Ok(qos_profile) => create_publisher(node, topic, type_name, type_hash, qos_profile),
            Err(e) => {
                tracing::warn!("hiroz: Invalid QoS struct for a publisher: {:?}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// The body shared by the `with_qos` creators.
///
/// # Safety
/// The same as `hiroz_publisher_create_with_qos`.
unsafe fn create_publisher(
    node: *mut CNode,
    topic: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    qos_profile: Option<crate::qos::QosProfile>,
) -> *mut CPublisher {
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

        match node_ref.create_raw_publisher_with_qos(
            topic_str,
            type_name_str,
            type_hash_str,
            qos_profile,
        ) {
            Ok(pub_inner) => Box::into_raw(Box::new(CPublisher {
                inner: Box::new(pub_inner),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create publisher: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Publish raw bytes (already CDR serialized)
///
/// # Safety
/// `pub_handle` must be a valid publisher pointer. `data` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_publish(
    pub_handle: *mut CPublisher,
    data: *const u8,
    len: usize,
) -> i32 {
    if pub_handle.is_null() || data.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let publisher = &(*pub_handle);
        let bytes = std::slice::from_raw_parts(data, len);

        match publisher.inner.publish_bytes(bytes) {
            Ok(_) => ErrorCode::Success as i32,
            Err(_) => ErrorCode::PublishFailed as i32,
        }
    }
}

/// Destroy a publisher
///
/// # Safety
/// `pub_handle` must be a valid publisher pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_publisher_destroy(pub_handle: *mut CPublisher) -> i32 {
    if pub_handle.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(pub_handle);
    }
    ErrorCode::Success as i32
}
