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

/// Deserialize a bounded sequence: a `Vec` with at most `MAX` elements. A longer sequence is an
/// error, reported as soon as the declared count is known. Generated message types name it in
/// `#[serde(deserialize_with = ...)]`.
pub fn deserialize_bounded<'de, D, T, const MAX: usize>(
    deserializer: D,
) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct Visitor<T, const MAX: usize>(std::marker::PhantomData<T>);

    impl<'de, T: serde::Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de>
        for Visitor<T, MAX>
    {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a sequence of at most {MAX} elements")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<T>, A::Error> {
            use serde::de::Error;
            let too_long = |len: usize| {
                A::Error::custom(format!(
                    "Sequence length {len} exceeds the declared maximum ({MAX})"
                ))
            };
            let hint = seq.size_hint();
            if let Some(len) = hint.filter(|len| *len > MAX) {
                return Err(too_long(len));
            }
            // A format without a count hint is checked one element past the maximum.
            let mut values = Vec::with_capacity(hint.unwrap_or(0).min(4096));
            while let Some(value) = seq.next_element()? {
                if values.len() == MAX {
                    return Err(too_long(MAX + 1));
                }
                values.push(value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(Visitor::<T, MAX>(std::marker::PhantomData))
}

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
