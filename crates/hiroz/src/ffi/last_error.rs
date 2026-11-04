//! Thread-local last-error message for the C interface. The contract is on `hiroz_last_error_message`.

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
/// The message is a hint for logging. The return value of a call is the only contract for failure: a negative code, or NULL from a constructor. A NULL message never means that a call succeeded.
///
/// Two functions return a positive value that is not a failure. `hiroz_graph_node_exists` returns 1 when the node is found. `hiroz_action_server_is_cancel_requested` returns 1 when a cancel was requested, and 0 for a null handle.
///
/// The library records a message in two cases only. One is a warning that it routes through its FFI warning macro. The other is a null or non-UTF-8 string argument that a call rejects or skips.
///
/// Many failures record no message. Examples are a null context or node handle, a `NullPointer` return for a null client or null output pointer, and a goal that the action server rejects. Another example is an unknown buffer group in `hiroz_subscriber_create_with_group`.
///
/// A message can be present after a successful call. For example, `hiroz_context_create_with_config` succeeds and records a message when `json_config` is not a JSON object. It does the same when an optional string is invalid, or when an element of `connect_endpoints` or `remap_rules` is NULL or invalid. A successful call never clears the message. Call `hiroz_last_error_clear` to clear it.
///
/// The message is thread-local. The pointer stays valid until one of three events on the same thread: the next call that records a message, a call to `hiroz_last_error_clear`, or the exit of the thread. Copy the message before then. Do not free it.
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
            std::thread::spawn(|| {
                assert_eq!(current(), None);
                set("theirs");
                hiroz_last_error_clear();
                set("theirs again");
            })
            .join()
            .unwrap();
            assert_eq!(current().as_deref(), Some("mine"));
            hiroz_last_error_clear();
            assert_eq!(current(), None);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn interior_nul_does_not_drop_the_message() {
        std::thread::spawn(|| {
            set("bad\0byte");
            let msg = current().expect("message must survive an interior NUL");
            assert_eq!(msg, "bad?byte");
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
