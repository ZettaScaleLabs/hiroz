//! A warning that goes through the FFI warning macro must reach `hiroz_last_error_message`.
//!
//! An integration test links the library once, so it runs where the lib unit-test binary does not link.

use std::ffi::CStr;

use hiroz::ffi::{
    context::hiroz_context_create_with_config,
    last_error::{hiroz_last_error_clear, hiroz_last_error_message},
};

fn current() -> Option<String> {
    let ptr = hiroz_last_error_message();
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned())
}

#[test]
fn null_config_records_a_message() {
    std::thread::spawn(|| {
        hiroz_last_error_clear();
        // Positive control: nothing is recorded before the call.
        assert_eq!(current(), None);

        let ctx = unsafe { hiroz_context_create_with_config(std::ptr::null()) };

        assert!(ctx.is_null());
        let msg = current().expect("the converted warning must record a message");
        assert!(msg.contains("Null config pointer"), "{msg}");
    })
    .join()
    .unwrap();
}

#[test]
fn clear_removes_a_recorded_message() {
    std::thread::spawn(|| {
        let _ = unsafe { hiroz_context_create_with_config(std::ptr::null()) };
        assert!(current().is_some());
        hiroz_last_error_clear();
        assert_eq!(current(), None);
    })
    .join()
    .unwrap();
}
