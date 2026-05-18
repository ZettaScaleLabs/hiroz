//! Fast CDR serializer optimized for direct buffer output.

use byteorder::ByteOrder;
use serde::{Serialize, ser};
use zenoh_buffers::ZBuf;

use crate::buffer::CdrBuffer;
use crate::error::{Error, Result};
use crate::primitives::CdrWriter;
use crate::zbuf_writer::ZBufWriter;

/// Fast CDR serializer that writes directly to a buffer.
///
/// This is a serde-based serializer that uses `CdrWriter` internally
/// for the actual byte-level operations.
pub struct CdrSerializer<'a, BO, B: CdrBuffer = Vec<u8>> {
    writer: CdrWriter<'a, BO, B>,
}

impl<'a, BO: ByteOrder, B: CdrBuffer> CdrSerializer<'a, BO, B> {
    /// Create a new serializer writing to the given buffer.
    pub fn new(buffer: &'a mut B) -> Self {
        Self {
            writer: CdrWriter::new(buffer),
        }
    }

    /// Get the current position in the buffer.
    #[inline(always)]
    pub fn position(&self) -> usize {
        self.writer.position()
    }
}

impl<'a, BO: ByteOrder> CdrSerializer<'a, BO, ZBufWriter> {
    /// Serialize a ZBuf field with zero-copy.
    #[inline]
    pub fn serialize_zbuf(&mut self, zbuf: &ZBuf) -> Result<()> {
        let len: usize = zbuf.zslices().map(|s| s.len()).sum();
        self.writer.write_u32(len as u32);
        self.writer.buffer_mut().append_zbuf(zbuf);
        Ok(())
    }
}

/// Serialize to a new Vec<u8>.
pub fn to_vec<T, BO>(value: &T, capacity_hint: usize) -> Result<Vec<u8>>
where
    T: Serialize,
    BO: ByteOrder,
{
    let mut buffer = Vec::with_capacity(capacity_hint);
    let mut serializer = CdrSerializer::<BO>::new(&mut buffer);
    value.serialize(&mut serializer)?;
    Ok(buffer)
}

/// Serialize to an existing Vec<u8> (for buffer reuse).
///
/// Uses 4KB-aligned buffer growth for reduced reallocation frequency.
pub fn to_vec_reuse<T, BO>(value: &T, buffer: &mut Vec<u8>) -> Result<()>
where
    T: Serialize,
    BO: ByteOrder,
{
    buffer.clear();
    let estimated_size = std::mem::size_of_val(value) * 2;
    buffer.reserve_4k(estimated_size);

    let mut serializer = CdrSerializer::<BO>::new(buffer);
    value.serialize(&mut serializer)?;
    Ok(())
}

/// Serialize to any buffer type implementing `CdrBuffer`.
pub fn to_buffer<T, BO, B>(value: &T, buffer: &mut B) -> Result<()>
where
    T: Serialize,
    BO: ByteOrder,
    B: CdrBuffer,
{
    buffer.clear();
    let mut serializer = CdrSerializer::<BO, B>::new(buffer);
    value.serialize(&mut serializer)?;
    Ok(())
}

impl<BO, B> ser::Serializer for &mut CdrSerializer<'_, BO, B>
where
    BO: ByteOrder,
    B: CdrBuffer,
{
    type Ok = ();
    type Error = Error;

    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    #[inline]
    fn serialize_bool(self, v: bool) -> Result<()> {
        self.writer.write_bool(v);
        Ok(())
    }

    #[inline]
    fn serialize_u8(self, v: u8) -> Result<()> {
        self.writer.write_u8(v);
        Ok(())
    }

    #[inline]
    fn serialize_u16(self, v: u16) -> Result<()> {
        self.writer.write_u16(v);
        Ok(())
    }

    #[inline]
    fn serialize_u32(self, v: u32) -> Result<()> {
        self.writer.write_u32(v);
        Ok(())
    }

    #[inline]
    fn serialize_u64(self, v: u64) -> Result<()> {
        self.writer.write_u64(v);
        Ok(())
    }

    #[inline]
    fn serialize_u128(self, v: u128) -> Result<()> {
        self.writer.align(16);
        let mut buf = [0u8; 16];
        BO::write_u128(&mut buf, v);
        self.writer.buffer_mut().extend_from_slice(&buf);
        Ok(())
    }

    #[inline]
    fn serialize_i8(self, v: i8) -> Result<()> {
        self.writer.write_i8(v);
        Ok(())
    }

    #[inline]
    fn serialize_i16(self, v: i16) -> Result<()> {
        self.writer.write_i16(v);
        Ok(())
    }

    #[inline]
    fn serialize_i32(self, v: i32) -> Result<()> {
        self.writer.write_i32(v);
        Ok(())
    }

    #[inline]
    fn serialize_i64(self, v: i64) -> Result<()> {
        self.writer.write_i64(v);
        Ok(())
    }

    #[inline]
    fn serialize_f32(self, v: f32) -> Result<()> {
        self.writer.write_f32(v);
        Ok(())
    }

    #[inline]
    fn serialize_f64(self, v: f64) -> Result<()> {
        self.writer.write_f64(v);
        Ok(())
    }

    #[inline]
    fn serialize_char(self, v: char) -> Result<()> {
        self.serialize_u32(v as u32)
    }

    #[inline]
    fn serialize_str(self, v: &str) -> Result<()> {
        self.writer.write_string(v);
        Ok(())
    }

    #[inline]
    fn serialize_bytes(self, v: &[u8]) -> Result<()> {
        self.writer.write_bytes(v);
        Ok(())
    }

    #[inline]
    fn serialize_none(self) -> Result<()> {
        self.serialize_u32(0)
    }

    #[inline]
    fn serialize_some<T>(self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.serialize_u32(1)?;
        value.serialize(self)
    }

    #[inline]
    fn serialize_unit(self) -> Result<()> {
        Ok(())
    }

    #[inline]
    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.serialize_unit()
    }

    #[inline]
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> Result<()> {
        self.serialize_u32(variant_index)
    }

    #[inline]
    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }

    #[inline]
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        self.serialize_u32(variant_index)?;
        value.serialize(self)
    }

    #[inline]
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        match len {
            None => Err(Error::UnknownLength),
            Some(elem_count) => {
                self.writer.write_sequence_length(elem_count);
                Ok(self)
            }
        }
    }

    #[inline]
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        Ok(self)
    }

    #[inline]
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Ok(self)
    }

    #[inline]
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.serialize_u32(variant_index)?;
        Ok(self)
    }

    #[inline]
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        match len {
            None => Err(Error::UnknownLength),
            Some(elem_count) => {
                self.writer.write_sequence_length(elem_count);
                Ok(self)
            }
        }
    }

    #[inline]
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Ok(self)
    }

    #[inline]
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.serialize_u32(variant_index)?;
        Ok(self)
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeSeq for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeTuple for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_element<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeTupleStruct for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeTupleVariant for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeMap for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_key<T>(&mut self, key: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        key.serialize(&mut **self)
    }

    #[inline]
    fn serialize_value<T>(&mut self, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeStruct for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T>(&mut self, _key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl<BO: ByteOrder, B: CdrBuffer> ser::SerializeStructVariant for &mut CdrSerializer<'_, BO, B> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T>(&mut self, _key: &'static str, value: &T) -> Result<()>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(&mut **self)
    }

    #[inline]
    fn end(self) -> Result<()> {
        Ok(())
    }
}
