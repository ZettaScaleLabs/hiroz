//! Thread-local last-error message for the C interface.
//!
//! Error codes cannot say why a call failed, and several failures share one code.
//! A failing FFI call records a message here.
//! The caller reads it on the same OS thread, straight after the failing call.

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

/// Return the last error message of the calling thread, or NULL if none is recorded. The pointer stays valid until the next failing hiroz call or `hiroz_last_error_clear` on the same thread. Do not free it.
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
