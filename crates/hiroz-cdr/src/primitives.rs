//! Low-level CDR primitive read/write operations.
//!
//! This module provides `CdrWriter` and `CdrReader` for direct CDR byte
//! manipulation with proper alignment handling. These are used internally
//! by the serde-based serializer/deserializer and can also be used directly
//! for schema-driven (dynamic) message handling.

#[cfg(target_endian = "little")]
use bytemuck;
use byteorder::{ByteOrder, ReadBytesExt};
use std::marker::PhantomData;

use crate::buffer::CdrBuffer;
use crate::error::{Error, Result};

/// Low-level CDR writer with alignment handling.
///
/// Provides primitive write operations that handle CDR alignment requirements.
/// Used internally by `CdrSerializer` and available for direct use with
/// dynamic/schema-driven message serialization.
pub struct CdrWriter<'a, BO, B: CdrBuffer = Vec<u8>> {
    buffer: &'a mut B,
    start_offset: usize,
    _phantom: PhantomData<BO>,
}

impl<'a, BO: ByteOrder, B: CdrBuffer> CdrWriter<'a, BO, B> {
    /// Create a new writer for the given buffer.
    #[inline]
    pub fn new(buffer: &'a mut B) -> Self {
        let start_offset = buffer.len();
        Self {
            buffer,
            start_offset,
            _phantom: PhantomData,
        }
    }

    /// Current position relative to start.
    #[inline(always)]
    pub fn position(&self) -> usize {
        self.buffer.len() - self.start_offset
    }

    /// Add padding bytes for alignment.
    #[inline(always)]
    pub fn align(&mut self, alignment: usize) {
        let modulo = self.position() % alignment;
        if modulo != 0 {
            let padding = alignment - modulo;
            const ZEROS: [u8; 8] = [0; 8];
            self.buffer.extend_from_slice(&ZEROS[..padding]);
        }
    }

    /// Get mutable access to the underlying buffer.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut B {
        self.buffer
    }

    // Primitive write operations

    #[inline]
    pub fn write_bool(&mut self, v: bool) {
        self.buffer.push(if v { 1 } else { 0 });
    }

    #[inline]
    pub fn write_i8(&mut self, v: i8) {
        self.buffer.push(v as u8);
    }

    #[inline]
    pub fn write_u8(&mut self, v: u8) {
        self.buffer.push(v);
    }

    #[inline]
    pub fn write_i16(&mut self, v: i16) {
        self.align(2);
        let mut buf = [0u8; 2];
        BO::write_i16(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_u16(&mut self, v: u16) {
        self.align(2);
        let mut buf = [0u8; 2];
        BO::write_u16(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_i32(&mut self, v: i32) {
        self.align(4);
        let mut buf = [0u8; 4];
        BO::write_i32(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        self.align(4);
        let mut buf = [0u8; 4];
        BO::write_u32(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_i64(&mut self, v: i64) {
        self.align(8);
        let mut buf = [0u8; 8];
        BO::write_i64(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        self.align(8);
        let mut buf = [0u8; 8];
        BO::write_u64(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_f32(&mut self, v: f32) {
        self.align(4);
        let mut buf = [0u8; 4];
        BO::write_f32(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    #[inline]
    pub fn write_f64(&mut self, v: f64) {
        self.align(8);
        let mut buf = [0u8; 8];
        BO::write_f64(&mut buf, v);
        self.buffer.extend_from_slice(&buf);
    }

    /// Write a CDR string (length-prefixed with null terminator).
    #[inline]
    pub fn write_string(&mut self, s: &str) {
        let byte_count = s.len() as u32 + 1; // Include null terminator
        self.write_u32(byte_count);
        self.buffer.extend_from_slice(s.as_bytes());
        self.buffer.push(0); // Null terminator
    }

    /// Write raw bytes with length prefix (for sequences of u8).
    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(bytes.len() as u32);
        self.buffer.extend_from_slice(bytes);
    }

    /// Write a sequence length prefix.
    #[inline]
    pub fn write_sequence_length(&mut self, len: usize) {
        self.write_u32(len as u32);
    }

    /// Bulk-write a slice of plain (POD) values as raw bytes.
    ///
    /// The caller must write the sequence length prefix separately before calling this.
    /// Alignment is handled internally based on `T`'s alignment requirement.
    ///
    /// Only available on little-endian hosts where CDR wire layout == memory layout.
    #[cfg(target_endian = "little")]
    #[inline]
    pub fn write_pod_slice<T: crate::plain::CdrPlain + bytemuck::Pod>(&mut self, slice: &[T]) {
        debug_assert!(!slice.is_empty());
        self.align(std::mem::align_of::<T>());
        self.buffer.extend_from_slice(bytemuck::cast_slice(slice));
    }
}

/// Low-level CDR reader with alignment handling.
///
/// Provides primitive read operations that handle CDR alignment requirements.
/// Used internally by `CdrDeserializer` and available for direct use with
/// dynamic/schema-driven message deserialization.
pub struct CdrReader<'a, BO> {
    input: &'a [u8],
    position: usize,
    _phantom: PhantomData<BO>,
}

impl<'a, BO: ByteOrder> CdrReader<'a, BO> {
    /// Create a new reader for the given input bytes.
    #[inline]
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            position: 0,
            _phantom: PhantomData,
        }
    }

    /// Current read position.
    #[inline]
    pub fn position(&self) -> usize {
        self.position
    }

    /// Remaining bytes available.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.input.len() - self.position
    }

    /// Align to the given boundary.
    #[inline]
    pub fn align(&mut self, alignment: usize) -> Result<()> {
        let modulo = self.position % alignment;
        if modulo != 0 {
            let padding = alignment - modulo;
            if self.remaining() < padding {
                return Err(Error::UnexpectedEof);
            }
            self.position += padding;
        }
        Ok(())
    }

    /// Read raw bytes without alignment.
    #[inline]
    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.remaining() < count {
            return Err(Error::UnexpectedEof);
        }
        let bytes = &self.input[self.position..self.position + count];
        self.position += count;
        Ok(bytes)
    }

    // Primitive read operations

    #[inline]
    pub fn read_bool(&mut self) -> Result<bool> {
        let byte = self.read_bytes(1)?[0];
        match byte {
            0 => Ok(false),
            1 => Ok(true),
            x => Err(Error::InvalidBool(x)),
        }
    }

    #[inline]
    pub fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_bytes(1)?[0] as i8)
    }

    #[inline]
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    #[inline]
    pub fn read_i16(&mut self) -> Result<i16> {
        self.align(2)?;
        let bytes = self.read_bytes(2)?;
        Ok((&bytes[..]).read_i16::<BO>().unwrap())
    }

    #[inline]
    pub fn read_u16(&mut self) -> Result<u16> {
        self.align(2)?;
        let bytes = self.read_bytes(2)?;
        Ok((&bytes[..]).read_u16::<BO>().unwrap())
    }

    #[inline]
    pub fn read_i32(&mut self) -> Result<i32> {
        self.align(4)?;
        let bytes = self.read_bytes(4)?;
        Ok((&bytes[..]).read_i32::<BO>().unwrap())
    }

    #[inline]
    pub fn read_u32(&mut self) -> Result<u32> {
        self.align(4)?;
        let bytes = self.read_bytes(4)?;
        Ok((&bytes[..]).read_u32::<BO>().unwrap())
    }

    #[inline]
    pub fn read_i64(&mut self) -> Result<i64> {
        self.align(8)?;
        let bytes = self.read_bytes(8)?;
        Ok((&bytes[..]).read_i64::<BO>().unwrap())
    }

    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        self.align(8)?;
        let bytes = self.read_bytes(8)?;
        Ok((&bytes[..]).read_u64::<BO>().unwrap())
    }

    #[inline]
    pub fn read_f32(&mut self) -> Result<f32> {
        self.align(4)?;
        let bytes = self.read_bytes(4)?;
        Ok((&bytes[..]).read_f32::<BO>().unwrap())
    }

    #[inline]
    pub fn read_f64(&mut self) -> Result<f64> {
        self.align(8)?;
        let bytes = self.read_bytes(8)?;
        Ok((&bytes[..]).read_f64::<BO>().unwrap())
    }

    /// Read a CDR string (length-prefixed with null terminator).
    #[inline]
    pub fn read_string(&mut self) -> Result<String> {
        let len = self.read_u32()? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.read_bytes(len)?;
        // Remove null terminator if present
        let str_bytes = if bytes.last() == Some(&0) {
            &bytes[..bytes.len() - 1]
        } else {
            bytes
        };
        String::from_utf8(str_bytes.to_vec()).map_err(|e| Error::Utf8(e.utf8_error()))
    }

    /// Read a CDR string as a borrowed slice (zero-copy when possible).
    #[inline]
    pub fn read_str(&mut self) -> Result<&'a str> {
        let len = self.read_u32()? as usize;
        if len == 0 {
            return Ok("");
        }
        let bytes = self.read_bytes(len)?;
        // Remove null terminator if present
        let str_bytes = if bytes.last() == Some(&0) {
            &bytes[..bytes.len() - 1]
        } else {
            bytes
        };
        std::str::from_utf8(str_bytes).map_err(Error::Utf8)
    }

    /// Read a sequence length prefix.
    ///
    /// # Safety
    /// Includes a sanity check to prevent absurd allocations from malformed data.
    /// Maximum sequence length is 100 million elements (~400MB for float32 arrays).
    #[inline]
    pub fn read_sequence_length(&mut self) -> Result<usize> {
        const MAX_SEQUENCE_LENGTH: u32 = 100_000_000;
        let len = self.read_u32()?;
        if len > MAX_SEQUENCE_LENGTH {
            return Err(Error::Custom(format!(
                "Sequence length {} exceeds maximum allowed ({}). Possible schema mismatch or corrupted data.",
                len, MAX_SEQUENCE_LENGTH
            )));
        }
        Ok(len as usize)
    }

    /// Read raw bytes with length prefix.
    #[inline]
    pub fn read_byte_sequence(&mut self) -> Result<&'a [u8]> {
        let len = self.read_u32()? as usize;
        self.read_bytes(len)
    }

    /// Bulk-read `count` plain (POD) values as a zero-copy borrowed slice.
    ///
    /// The caller must have already read the sequence length prefix.
    /// Alignment is handled internally based on `T`'s alignment requirement.
    ///
    /// Only available on little-endian hosts where CDR wire layout == memory layout.
    #[cfg(target_endian = "little")]
    #[inline]
    pub fn read_pod_slice<T: crate::plain::CdrPlain + bytemuck::Pod>(
        &mut self,
        count: usize,
    ) -> Result<Vec<T>> {
        if count == 0 {
            return Ok(vec![]);
        }
        self.align(std::mem::align_of::<T>())?;
        let byte_count = count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(Error::UnexpectedEof)?;
        let bytes = self.read_bytes(byte_count)?;
        // `pod_collect_to_vec` handles misaligned input buffers safely (copies into
        // a freshly aligned allocation). `cast_slice` would panic on misaligned network data.
        Ok(bytemuck::pod_collect_to_vec(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::LittleEndian;

    #[test]
    fn test_writer_primitives() {
        let mut buffer = Vec::new();
        let mut writer = CdrWriter::<LittleEndian>::new(&mut buffer);

        writer.write_bool(true);
        writer.write_u8(42);
        writer.write_u32(12345);
        writer.write_f64(1.23456);
        writer.write_string("hello");

        assert!(!buffer.is_empty());
    }

    #[test]
    fn test_reader_primitives() {
        let mut buffer = Vec::new();
        {
            let mut writer = CdrWriter::<LittleEndian>::new(&mut buffer);
            writer.write_bool(true);
            writer.write_u8(42);
            writer.write_u32(12345);
            writer.write_f64(1.23456);
            writer.write_string("hello");
        }

        let mut reader = CdrReader::<LittleEndian>::new(&buffer);
        assert!(reader.read_bool().unwrap());
        assert_eq!(reader.read_u8().unwrap(), 42);
        assert_eq!(reader.read_u32().unwrap(), 12345);
        assert!((reader.read_f64().unwrap() - 1.23456).abs() < 0.00001);
        assert_eq!(reader.read_string().unwrap(), "hello");
    }

    #[test]
    fn test_alignment() {
        let mut buffer = Vec::new();
        let mut writer = CdrWriter::<LittleEndian>::new(&mut buffer);

        writer.write_u8(1); // position 1
        writer.write_u32(100); // should align to 4, so padding at 1,2,3

        // Buffer should be: [1, 0, 0, 0, 100, 0, 0, 0]
        assert_eq!(buffer.len(), 8);
        assert_eq!(buffer[0], 1);
        assert_eq!(buffer[1], 0); // padding
        assert_eq!(buffer[2], 0); // padding
        assert_eq!(buffer[3], 0); // padding
    }

    #[test]
    fn test_roundtrip_all_types() {
        let mut buffer = Vec::new();
        {
            let mut writer = CdrWriter::<LittleEndian>::new(&mut buffer);
            writer.write_bool(false);
            writer.write_i8(-42);
            writer.write_i16(-1000);
            writer.write_i32(-100000);
            writer.write_i64(-10000000000);
            writer.write_u8(200);
            writer.write_u16(50000);
            writer.write_u32(3000000000);
            writer.write_u64(10000000000);
            writer.write_f32(1.5);
            writer.write_f64(9.87654321);
            writer.write_string("test string");
        }

        let mut reader = CdrReader::<LittleEndian>::new(&buffer);
        assert!(!reader.read_bool().unwrap());
        assert_eq!(reader.read_i8().unwrap(), -42);
        assert_eq!(reader.read_i16().unwrap(), -1000);
        assert_eq!(reader.read_i32().unwrap(), -100000);
        assert_eq!(reader.read_i64().unwrap(), -10000000000);
        assert_eq!(reader.read_u8().unwrap(), 200);
        assert_eq!(reader.read_u16().unwrap(), 50000);
        assert_eq!(reader.read_u32().unwrap(), 3000000000);
        assert_eq!(reader.read_u64().unwrap(), 10000000000);
        assert!((reader.read_f32().unwrap() - 1.5).abs() < 0.001);
        assert!((reader.read_f64().unwrap() - 9.87654321).abs() < 0.0000001);
        assert_eq!(reader.read_string().unwrap(), "test string");
    }
}

// ---------------------------------------------------------------------------
// Kani formal proof harnesses
// ---------------------------------------------------------------------------

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use byteorder::LittleEndian;

    /// For any u32 value, writing then reading it back via CdrWriter/CdrReader
    /// recovers the original value exactly.
    #[kani::proof]
    fn u32_serialize_deserialize_identity() {
        let value: u32 = kani::any();

        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut writer = CdrWriter::<LittleEndian, _>::new(&mut buffer);
            writer.write_u32(value);
        }

        let mut reader = CdrReader::<LittleEndian>::new(&buffer);
        let recovered = reader.read_u32().expect("read_u32");

        kani::assert(recovered == value, "u32 roundtrip identity");
    }

    /// For any i32 value, writing then reading back recovers the original.
    #[kani::proof]
    fn i32_serialize_deserialize_identity() {
        let value: i32 = kani::any();

        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut writer = CdrWriter::<LittleEndian, _>::new(&mut buffer);
            writer.write_i32(value);
        }

        let mut reader = CdrReader::<LittleEndian>::new(&buffer);
        let recovered = reader.read_i32().expect("read_i32");

        kani::assert(recovered == value, "i32 roundtrip identity");
    }
}
