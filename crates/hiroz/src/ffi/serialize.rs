//! FFI for serialization/deserialization
//! Go passes raw struct bytes, Rust serializes to CDR

use std::ffi::c_char;

use super::{ErrorCode, cstr_to_str};

/// Serialize a message to CDR format
/// Input: type_name (C string), raw message bytes from Go
/// Output: CDR serialized bytes via out_ptr/out_len
///
/// # Safety
/// Every non-null string pointer must address a NUL-terminated string. The input
/// byte pointer must be readable for its supplied length, within one allocation,
/// for this call. Non-null output pointers must be aligned, writable, disjoint
/// storage. Inputs are borrowed only until return. On success, ownership of the
/// output allocation transfers to the caller; free it exactly once with
/// `hiroz_free_bytes` and the returned length. Outputs are unchanged on failure.
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
///
/// # Safety
/// Every non-null string pointer must address a NUL-terminated string. The input
/// byte pointer must be readable for its supplied length, within one allocation,
/// for this call. Non-null output pointers must be aligned, writable, disjoint
/// storage. Inputs are borrowed only until return. On success, ownership of the
/// output allocation transfers to the caller; free it exactly once with
/// `hiroz_free_bytes` and the returned length. Outputs are unchanged on failure.
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

/// Free bytes allocated by serialize/deserialize.
///
/// # Safety
/// A non-null pointer with a nonzero length must be an allocation returned by a
/// Hiroz FFI function that documents `hiroz_free_bytes` as its deallocator. Pass
/// the exact returned length, and call this only once after all readers have
/// finished. No other operation may access the allocation during or after this
/// call. A null pointer or zero length is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len));
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
