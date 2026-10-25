//! Thread-local last-error message for the C interface.
//!
//! Error codes cannot say why a call failed, and several failures share one code.
//! A message is recorded in only two cases: a warning that the library routes through its FFI warning macro, and a null or non-UTF-8 string argument rejected by the shared string conversion.
//! A warning logged by a plain `tracing::warn!` records nothing, and neither do other failing returns, such as a null handle or a failed publish.
//! A call that succeeds can also leave a message, because some recording paths do not fail the call.
//! A caller must check the return value first, and read the message only after a call has reported failure: a non-zero code, or NULL from a constructor.
//! The message is per OS thread. A NULL message means that none is recorded.

use std::cell::RefCell;
use std::ffi::{CString, c_char};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Record `msg` as this thread's last error. An interior NUL byte becomes `?`.
pub(crate) fn set(msg: &str) {
    let c = CString::new(msg.replace('\0', "?")).unwrap_or_default();
    let _ = LAST_ERROR.try_with(|slot| *slot.borrow_mut() = Some(c));
}

/// Return the last error message of the calling thread, or NULL if none is recorded.
///
/// A message is recorded in only two cases: a warning that the library routes through its FFI warning macro, and a null or non-UTF-8 string argument that a call rejects. Other failing calls record nothing, and so do warnings that the library logs without that macro.
///
/// These entry points record a message when they fail: `hiroz_context_create`, `hiroz_context_create_with_config`, `hiroz_node_create`, `hiroz_node_create_with_config`, `hiroz_publisher_create`, `hiroz_publisher_create_with_qos`, every `hiroz_subscriber_create*` variant except the unknown-group case below, `hiroz_service_client_create`, `hiroz_service_client_call`, `hiroz_service_server_create`, `hiroz_action_client_create`, `hiroz_action_client_send_goal`, `hiroz_action_client_get_result`, `hiroz_action_client_cancel_goal`, `hiroz_action_server_create` and `hiroz_action_server_publish_feedback`. `hiroz_graph_node_exists`, `hiroz_serialize` and `hiroz_deserialize` record a message only for a null or non-UTF-8 string argument.
///
/// These failures record nothing: an unknown buffer group in `hiroz_subscriber_create_with_group` (it returns NULL), a type that `hiroz_serialize` or `hiroz_deserialize` does not implement, a reply error that a service client receives, and every other failing return not named above.
///
/// Read the message only after a call has reported failure: a non-zero code, or NULL from a constructor. A message can also be present after a successful call. `hiroz_context_create_with_config` still succeeds, and records a message, when `json_config` is not a JSON object, when an optional string field holds invalid UTF-8, or when an element of `connect_endpoints` or `remap_rules` is NULL or invalid UTF-8. A successful call never clears the message, so check the return value first, and call `hiroz_last_error_clear` before a call if you want to ignore an older message. Treat NULL as "no message recorded".
///
/// The pointer stays valid until the next call on the same thread that records a message, or `hiroz_last_error_clear`. Do not free it.
#[unsafe(no_mangle)]
pub extern "C" fn hiroz_last_error_message() -> *const c_char {
    LAST_ERROR
        .try_with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |c| c.as_ptr())
        })
        .unwrap_or(std::ptr::null())
}

/// Discard the last error message of the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn hiroz_last_error_clear() {
    let _ = LAST_ERROR.try_with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn current() -> Option<String> {
        let ptr = hiroz_last_error_message();
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned())
        }
    }

    #[test]
    fn empty_on_a_fresh_thread() {
        std::thread::spawn(|| assert_eq!(current(), None))
            .join()
            .unwrap();
    }

    #[test]
    fn set_then_read() {
        std::thread::spawn(|| {
            set("boom");
            assert_eq!(current().as_deref(), Some("boom"));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn later_error_replaces_earlier() {
        std::thread::spawn(|| {
            set("first");
            set("second");
            assert_eq!(current().as_deref(), Some("second"));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn clear_removes_the_message() {
        std::thread::spawn(|| {
            set("boom");
            hiroz_last_error_clear();
            assert_eq!(current(), None);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn messages_are_per_thread() {
        std::thread::spawn(|| {
            set("mine");
            std::thread::spawn(|| assert_eq!(current(), None))
                .join()
                .unwrap();
            assert_eq!(current().as_deref(), Some("mine"));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn interior_nul_does_not_drop_the_message() {
        std::thread::spawn(|| {
            set("bad\0byte");
            let msg = current().expect("message must survive an interior NUL");
            assert!(msg.starts_with("bad") && msg.ends_with("byte"), "{msg}");
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cstr_to_str_null_records_a_message() {
        std::thread::spawn(|| {
            hiroz_last_error_clear();
            let r = unsafe { crate::ffi::cstr_to_str(std::ptr::null()) };
            assert!(r.is_err());
            assert!(current().unwrap().contains("null"), "{:?}", current());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cstr_to_str_invalid_utf8_records_a_message() {
        std::thread::spawn(|| {
            hiroz_last_error_clear();
            let bad = std::ffi::CString::new(vec![0xFF, 0xFE]).unwrap();
            let r = unsafe { crate::ffi::cstr_to_str(bad.as_ptr()) };
            assert!(r.is_err());
            assert!(current().unwrap().contains("UTF-8"), "{:?}", current());
        })
        .join()
        .unwrap();
    }
}
