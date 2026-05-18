//! C-compatible FFI layer for Go/Python/etc. bindings

pub mod action;
pub mod context;
pub mod graph;
pub mod node;
pub mod publisher;
pub mod qos;
pub mod serialize;
pub mod service;
pub mod subscriber;

use std::ffi::{CStr, c_char};

/// Common error codes returned to foreign callers
#[repr(i32)]
#[derive(Debug)]
pub enum ErrorCode {
    Success = 0,
    NullPointer = -1,
    InvalidUtf8 = -2,
    SessionClosed = -3,
    PublishFailed = -4,
    SerializationFailed = -5,
    SubscribeFailed = -6,
    NodeCreationFailed = -7,
    ContextCreationFailed = -8,
    ServiceCallFailed = -9,
    ServiceTimeout = -10,
    ActionGoalRejected = -11,
    ActionCancelFailed = -12,
    ActionResultFailed = -13,
    ActionFeedbackFailed = -14,
    DeserializationFailed = -15,
    Unknown = -100,
}

/// Convert C string to Rust &str safely
pub(crate) unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Result<&'a str, ErrorCode> {
    if ptr.is_null() {
        return Err(ErrorCode::NullPointer);
    }
    unsafe {
        CStr::from_ptr(ptr)
            .to_str()
            .map_err(|_| ErrorCode::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cstr_to_str_valid() {
        let c_string = std::ffi::CString::new("hello world").unwrap();
        let ptr = c_string.as_ptr();

        unsafe {
            let result = cstr_to_str(ptr);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "hello world");
        }
    }

    #[test]
    fn test_cstr_to_str_null() {
        let result = unsafe { cstr_to_str(std::ptr::null()) };
        assert!(matches!(result, Err(ErrorCode::NullPointer)));
    }

    #[test]
    fn test_cstr_to_str_invalid_utf8() {
        // Create a C string with invalid UTF-8 bytes (but valid for C string - no null terminator in the middle)
        let invalid_utf8 = &[0xFF, 0xFE]; // Invalid UTF-8 sequence
        let c_string = std::ffi::CString::new(invalid_utf8).unwrap();
        let ptr = c_string.as_ptr();

        unsafe {
            let result = cstr_to_str(ptr);
            assert!(matches!(result, Err(ErrorCode::InvalidUtf8)));
        }
    }
}
