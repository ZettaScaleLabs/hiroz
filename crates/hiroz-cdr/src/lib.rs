//! CDR (Common Data Representation) serialization for hiroz.
//!
//! This crate provides CDR serialization and deserialization compatible with
//! RMW Zenoh for ROS 2 communication.
//!
//! # Architecture
//!
//! The crate provides two levels of API:
//!
//! 1. **Low-level primitives** (`CdrWriter`, `CdrReader`): Direct byte-level
//!    operations with CDR alignment handling. Used for schema-driven (dynamic)
//!    message serialization.
//!
//! 2. **Serde integration** (`CdrSerializer`, `CdrDeserializer`): Type-driven
//!    serialization using Rust's serde framework. Used for static message types.

pub mod buffer;
pub mod deserializer;
pub mod error;
pub mod plain;
pub mod primitives;
pub mod serializer;
pub mod traits;
pub mod zbuf_writer;

use std::cell::RefCell;

// Thread-local for zero-copy ZBuf deserialization bypass.
// When set with the source payload ZBuf, ZBuf::Deserialize creates sub-ZSlices
// instead of copying bytes, enabling zero-copy deserialization.
thread_local! {
    pub static ZBUF_DESER_SOURCE: RefCell<Option<zenoh_buffers::ZBuf>> = const { RefCell::new(None) };
}

// Re-export main types for convenience
pub use buffer::CdrBuffer;
// Re-export byteorder types for convenience
pub use byteorder::{BigEndian, LittleEndian};
pub use deserializer::{CdrDeserializer, from_bytes, from_bytes_with};
pub use error::{Error, Result};
#[cfg(target_endian = "little")]
pub use plain::CdrPlain;
pub use primitives::{CdrReader, CdrWriter};
pub use serializer::{CdrSerializer, to_buffer, to_vec, to_vec_reuse};
pub use traits::{CdrDeserialize, CdrSerialize, CdrSerializedSize, cdr_to_vec};
pub use zbuf_writer::ZBufWriter;

/// Native endian type alias for the current platform.
///
/// On little-endian platforms (x86_64, ARM), this is `LittleEndian`.
/// On big-endian platforms, this is `BigEndian`.
///
/// Using `NativeEndian` allows the compiler to optimize away byte-swapping
/// operations when serializing for the native platform.
#[cfg(target_endian = "little")]
pub type NativeEndian = LittleEndian;

#[cfg(target_endian = "big")]
pub type NativeEndian = BigEndian;
