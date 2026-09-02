//! hiroz wrapper around Zenoh's ZBuf with optimized serde implementations.
//!
//! This module provides a [`ZBuf`] type that wraps `zenoh_buffers::ZBuf` and implements
//! efficient serialization/deserialization for ROS message byte arrays (`uint8[]`, `byte[]`).
//!
//! The wrapper uses `serialize_bytes()` instead of `serialize_seq()` for better performance
//! with large byte arrays, which is critical for messages like sensor images.

use hiroz_cdr::{CdrBuffer, CdrDeserialize, CdrReader, CdrSerialize, CdrSerializedSize, CdrWriter};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use zenoh_buffers::ZBuf as ZenohZBuf;
use zenoh_buffers::buffer::{Buffer, SplitBuffer};

/// hiroz wrapper around Zenoh's ZBuf with optimized serde.
///
/// This type wraps [`zenoh_buffers::ZBuf`] and provides efficient serialization
/// for ROS message byte array fields. It's used in generated code for `uint8[]`
/// and `byte[]` fields.
///
/// # Examples
///
/// ```
/// use hiroz::ZBuf;
///
/// let data = vec![1u8, 2, 3, 4, 5];
/// let zbuf = ZBuf::from(data);
///
/// // Serialization uses optimized serialize_bytes()
/// let serialized = serde_json::to_string(&zbuf).unwrap();
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZBuf(pub ZenohZBuf);

impl ZBuf {
    /// Creates a new empty ZBuf.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the underlying Zenoh ZBuf.
    #[inline]
    pub fn into_inner(self) -> ZenohZBuf {
        self.0
    }

    /// Creates a ZBuf from a Zenoh ZBuf.
    #[inline]
    pub fn from_zenoh(zbuf: ZenohZBuf) -> Self {
        Self(zbuf)
    }
}

// Conversions
impl From<ZenohZBuf> for ZBuf {
    #[inline]
    fn from(zbuf: ZenohZBuf) -> Self {
        Self(zbuf)
    }
}

impl From<Vec<u8>> for ZBuf {
    #[inline]
    fn from(vec: Vec<u8>) -> Self {
        Self(ZenohZBuf::from(vec))
    }
}

impl From<ZBuf> for ZenohZBuf {
    #[inline]
    fn from(zbuf: ZBuf) -> Self {
        zbuf.0
    }
}

impl From<&[u8]> for ZBuf {
    #[inline]
    fn from(slice: &[u8]) -> Self {
        Self(ZenohZBuf::from(slice.to_vec()))
    }
}

impl From<zenoh::shm::ZShmMut> for ZBuf {
    #[inline]
    fn from(shm: zenoh::shm::ZShmMut) -> Self {
        Self(ZenohZBuf::from(shm))
    }
}

// Deref to make it transparent
impl std::ops::Deref for ZBuf {
    type Target = ZenohZBuf;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ZBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// ── CdrSerialize / CdrDeserialize / CdrSerializedSize ────────────────────────
//
// CDR encoding for ZBuf: u32 sequence length prefix + raw bytes (like Vec<u8>).
// This matches the ROS CDR wire format for `byte[]` / `uint8[]` fields.

impl CdrSerialize for ZBuf {
    #[inline]
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(&self, w: &mut CdrWriter<'_, BO, B>) {
        let bytes = self.0.contiguous();
        // write_bytes writes the u32 length prefix followed by the raw bytes
        w.write_bytes(bytes.as_ref());
    }
}

impl CdrDeserialize for ZBuf {
    #[inline]
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        r: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        let count = r.read_sequence_length()?;
        let bytes = r.read_bytes(count)?;
        // Try zero-copy: check if the bytes sit inside a source ZBuf.
        let zbuf = hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
            let borrow = cell.borrow();
            let source = borrow.as_ref()?;
            let v_start = bytes.as_ptr() as usize;
            let v_end = v_start + bytes.len();
            for zslice in source.zslices() {
                let s_start = zslice.as_slice().as_ptr() as usize;
                let s_end = s_start + zslice.len();
                if v_start >= s_start && v_end <= s_end {
                    let offset = v_start - s_start;
                    let sub = zslice.subslice(offset..offset + bytes.len())?;
                    let mut z = ZenohZBuf::default();
                    z.push_zslice(sub);
                    return Some(z);
                }
            }
            None
        });
        let inner = zbuf.unwrap_or_else(|| ZenohZBuf::from(bytes.to_vec()));
        Ok(ZBuf(inner))
    }
}

impl CdrSerializedSize for ZBuf {
    #[inline]
    fn cdr_serialized_size(&self, pos: usize) -> usize {
        // u32 length prefix (4-byte aligned) + byte contents
        let after_len = pos + (4 - pos % 4) % 4 + 4;
        after_len + self.0.len()
    }
}

// ── Optimized serde implementation ──────────────────────────────────────────
impl Serialize for ZBuf {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Use contiguous() for zero-copy access and serialize as bytes.
        let bytes = self.0.contiguous();
        serializer.serialize_bytes(bytes.as_ref())
    }
}

impl<'de> Deserialize<'de> for ZBuf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;

        impl<'de> serde::de::Visitor<'de> for BytesVisitor {
            type Value = ZenohZBuf;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("byte array")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(ZenohZBuf::from(v.to_vec()))
            }

            fn visit_borrowed_bytes<E: serde::de::Error>(
                self,
                v: &'de [u8],
            ) -> Result<Self::Value, E> {
                // Try zero-copy: check if v is within a source ZBuf's ZSlice
                let zbuf = hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
                    let borrow = cell.borrow();
                    let source = borrow.as_ref()?;
                    let v_start = v.as_ptr() as usize;
                    let v_end = v_start + v.len();
                    for zslice in source.zslices() {
                        let s_start = zslice.as_slice().as_ptr() as usize;
                        let s_end = s_start + zslice.len();
                        if v_start >= s_start && v_end <= s_end {
                            let offset = v_start - s_start;
                            let sub = zslice.subslice(offset..offset + v.len())?;
                            let mut zbuf = ZenohZBuf::default();
                            zbuf.push_zslice(sub);
                            return Some(zbuf);
                        }
                    }
                    None
                });
                Ok(zbuf.unwrap_or_else(|| ZenohZBuf::from(v.to_vec())))
            }

            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                Ok(ZenohZBuf::from(v))
            }

            // Fallback for formats that don't support bytes natively
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let len = seq.size_hint().unwrap_or(0);
                let mut bytes = Vec::with_capacity(len);
                while let Some(b) = seq.next_element()? {
                    bytes.push(b);
                }
                Ok(ZenohZBuf::from(bytes))
            }
        }

        deserializer
            .deserialize_bytes(BytesVisitor)
            .map(ZBuf::from_zenoh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenoh_buffers::buffer::Buffer;

    #[test]
    fn test_zbuf_creation() {
        let zbuf = ZBuf::new();
        assert_eq!(zbuf.len(), 0);

        let zbuf = ZBuf::from(vec![1u8, 2, 3]);
        assert_eq!(zbuf.len(), 3);
    }

    #[test]
    fn test_zbuf_from_slice() {
        let data: &[u8] = &[1u8, 2, 3, 4, 5];
        let zbuf = ZBuf::from(data);
        assert_eq!(zbuf.len(), 5);
    }

    #[test]
    fn test_zbuf_deref() {
        let zbuf = ZBuf::from(vec![1u8, 2, 3]);
        // Test that we can call ZenohZBuf methods via Deref
        assert_eq!(zbuf.len(), 3);
        let bytes = zbuf.contiguous();
        assert_eq!(bytes.as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn test_zbuf_serialize_json() {
        let zbuf = ZBuf::from(vec![1u8, 2, 3, 4, 5]);
        let serialized = serde_json::to_string(&zbuf).unwrap();
        // JSON represents bytes as an array, but serde_json uses serialize_bytes
        // which gets encoded as a base64 string or array depending on the serializer
        assert!(!serialized.is_empty());
    }

    #[test]
    fn test_zbuf_deserialize_json() {
        // JSON array format
        let json = "[1,2,3,4,5]";
        let zbuf: ZBuf = serde_json::from_str(json).unwrap();
        let bytes = zbuf.contiguous();
        assert_eq!(bytes.as_ref(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_zbuf_roundtrip() {
        let original = ZBuf::from(vec![1u8, 2, 3, 4, 5, 6, 7, 8]);
        let serialized = serde_json::to_vec(&original).unwrap();
        let deserialized: ZBuf = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(original, deserialized);
    }
}
