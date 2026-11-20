#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::explicit_auto_deref)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::collapsible_if)]

// Macro to get file name as C string pointer for error reporting
#[macro_export]
macro_rules! cfile {
    () => {
        concat!(file!(), "\0").as_ptr() as *const std::os::raw::c_char
    };
}

pub mod common;
pub mod context;
pub mod exec_callback;
pub mod guard_condition;
pub mod msg;
pub mod node;
pub mod pubsub;
pub mod qos;
pub mod rmw;
pub mod ros;
pub mod service;
#[macro_use]
pub mod traits;
pub mod type_support;
pub mod utils;
pub mod wait_set;

/// Newtype wrapper for a C void. Only useful as a `*c_void`
#[allow(non_camel_case_types)]
#[repr(transparent)]
pub struct c_void(pub ::std::os::raw::c_void);

/// # Safety
///
/// We assert that the namespace and type ID refer to a C++
/// type which is equivalent to this Rust type.
unsafe impl cxx::ExternType for c_void {
    type Id = cxx::type_id!(c_void);
    type Kind = cxx::kind::Trivial;
}

// RMW implementation identifier (null-terminated for C compatibility)
pub const RMW_ZENOH_IDENTIFIER: &[u8] = b"rmw_zenoh_rs\0";

// Serialization format (null-terminated for C compatibility)
pub const RMW_ZENOH_SERIALIZATION_FORMAT: &[u8] = b"cdr\0";

// Note: rmw-z implements the RMW layer directly in Rust using Zenoh
// No C++ bridge needed since we're not interfacing with existing RMW implementations

// Remove the cxx extern block since we're implementing RMW directly

pub use rmw::*;
