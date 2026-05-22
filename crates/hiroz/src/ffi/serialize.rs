//! FFI for serialization/deserialization
//! Go passes raw struct bytes, Rust serializes to CDR

use super::{ErrorCode, cstr_to_str};
use std::ffi::c_char;

/// Serialize a message to CDR format
/// Input: type_name (C string), raw message bytes from Go
/// Output: CDR serialized bytes via out_ptr/out_len
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_serialize(
    type_name: *const c_char,
    msg_data: *const u8,
    msg_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        let type_name = match cstr_to_str(type_name) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidUtf8 as i32,
        };

        if msg_data.is_null() || out_ptr.is_null() || out_len.is_null() {
            return ErrorCode::NullPointer as i32;
        }

        let input = std::slice::from_raw_parts(msg_data, msg_len);

        // Call into the generated serializer registry
        match serialize_by_type(type_name, input) {
            Ok(serialized) => {
                let boxed = serialized.into_boxed_slice();
                *out_len = boxed.len();
                *out_ptr = Box::into_raw(boxed) as *mut u8;
                ErrorCode::Success as i32
            }
            Err(_) => ErrorCode::SerializationFailed as i32,
        }
    }
}

/// Deserialize CDR bytes to raw format for Go
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_deserialize(
    type_name: *const c_char,
    cdr_data: *const u8,
    cdr_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        let type_name = match cstr_to_str(type_name) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidUtf8 as i32,
        };

        if cdr_data.is_null() || out_ptr.is_null() || out_len.is_null() {
            return ErrorCode::NullPointer as i32;
        }

        let input = std::slice::from_raw_parts(cdr_data, cdr_len);

        match deserialize_by_type(type_name, input) {
            Ok(raw) => {
                let boxed = raw.into_boxed_slice();
                *out_len = boxed.len();
                *out_ptr = Box::into_raw(boxed) as *mut u8;
                ErrorCode::Success as i32
            }
            Err(_) => ErrorCode::SerializationFailed as i32,
        }
    }
}

/// Free bytes allocated by serialize/deserialize
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(ptr, len));
        }
    }
}

// These functions will dispatch to generated code based on type name
fn serialize_by_type(type_name: &str, _input: &[u8]) -> Result<Vec<u8>, ()> {
    // This will be filled in by codegen or a registration mechanism
    // For now, placeholder
    tracing::warn!("serialize_by_type: {} (not implemented)", type_name);
    Err(())
}

fn deserialize_by_type(type_name: &str, _input: &[u8]) -> Result<Vec<u8>, ()> {
    tracing::warn!("deserialize_by_type: {} (not implemented)", type_name);
    Err(())
}
