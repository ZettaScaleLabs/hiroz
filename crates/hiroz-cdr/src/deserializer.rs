//! CDR Deserializer for hiroz messages.

use std::marker::PhantomData;

use byteorder::ByteOrder;
use serde::de::{
    self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};

use crate::error::{Error, Result};
use crate::primitives::CdrReader;

/// Deserializer type for converting CDR data stream to Rust objects.
///
/// This is a serde-based deserializer that uses `CdrReader` internally
/// for the actual byte-level operations.
pub struct CdrDeserializer<'i, BO> {
    reader: CdrReader<'i, BO>,
}

impl<'de, BO> CdrDeserializer<'de, BO>
where
    BO: ByteOrder,
{
    /// Create a new deserializer from input bytes.
    #[inline]
    pub fn new(input: &'de [u8]) -> CdrDeserializer<'de, BO> {
        CdrDeserializer {
            reader: CdrReader::new(input),
        }
    }

    /// How many bytes of input stream have been consumed.
    #[inline]
    pub fn bytes_consumed(&self) -> usize {
        self.reader.position()
    }
}

/// Deserialize an object from `&[u8]` based on a [`serde::Deserialize`] implementation.
///
/// Returns deserialized object + count of bytes consumed.
///
/// For zero-copy deserialization of borrowed types (like `&str`), the input
/// bytes must outlive the deserialized value.
#[inline]
pub fn from_bytes<'de, T, BO>(input_bytes: &'de [u8]) -> Result<(T, usize)>
where
    T: serde::Deserialize<'de>,
    BO: ByteOrder,
{
    from_bytes_with::<PhantomData<T>, BO>(input_bytes, PhantomData)
}

/// Deserialize type based on a [`serde::Deserialize`] implementation.
///
/// Returns deserialized object + count of bytes consumed.
#[inline]
pub fn from_bytes_with<'de, S, BO>(input_bytes: &'de [u8], decoder: S) -> Result<(S::Value, usize)>
where
    S: DeserializeSeed<'de>,
    BO: ByteOrder,
{
    let mut deserializer = CdrDeserializer::<BO>::new(input_bytes);
    let t = decoder.deserialize(&mut deserializer)?;
    Ok((t, deserializer.bytes_consumed()))
}

impl<'de, BO> de::Deserializer<'de> for &mut CdrDeserializer<'de, BO>
where
    BO: ByteOrder,
{
    type Error = Error;

    /// CDR serialization is not a self-describing data format.
    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(Error::UnsupportedAny)
    }

    /// Boolean values are encoded as single octets (0 or 1).
    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_bool(self.reader.read_bool()?)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_i8(self.reader.read_i8()?)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_u8(self.reader.read_u8()?)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_i16(self.reader.read_i16()?)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_u16(self.reader.read_u16()?)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_i32(self.reader.read_i32()?)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_u32(self.reader.read_u32()?)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_i64(self.reader.read_i64()?)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_u64(self.reader.read_u64()?)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_f32(self.reader.read_f32()?)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_f64(self.reader.read_f64()?)
    }

    /// Since this is Rust, a char is 32-bit Unicode codepoint.
    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let codepoint = self.reader.read_u32()?;
        match char::from_u32(codepoint) {
            Some(c) => visitor.visit_char(c),
            None => Err(Error::InvalidChar(codepoint)),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let s = self.reader.read_str()?;
        visitor.visit_borrowed_str(s)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        // For owned strings, still use borrowed path - serde will copy if needed
        self.deserialize_str(visitor)
    }

    /// OPTIMIZED: Read bytes efficiently in bulk instead of element-by-element.
    /// This is critical for large byte arrays (images, point clouds, etc.).
    /// Uses zero-copy borrowed bytes when possible.
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let bytes = self.reader.read_byte_sequence()?;
        visitor.visit_borrowed_bytes(bytes)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let bytes = self.reader.read_byte_sequence()?;
        visitor.visit_byte_buf(bytes.to_vec())
    }

    #[inline]
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let enum_tag = self.reader.read_u32()?;
        match enum_tag {
            0 => visitor.visit_none(),
            1 => visitor.visit_some(self),
            wtf => Err(Error::InvalidOptionTag(wtf)),
        }
    }

    #[inline]
    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        // Unit data is not put on wire
        visitor.visit_unit()
    }

    #[inline]
    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    #[inline]
    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    /// Sequences are encoded as an unsigned long value, followed by the elements.
    #[inline]
    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let element_count = self.reader.read_sequence_length()?;
        visitor.visit_seq(SequenceHelper::new(self, element_count))
    }

    /// Fixed length array - number of elements is not included.
    #[inline]
    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(SequenceHelper::new(self, len))
    }

    #[inline]
    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(SequenceHelper::new(self, len))
    }

    #[inline]
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let element_count = self.reader.read_sequence_length()?;
        visitor.visit_map(SequenceHelper::new(self, element_count))
    }

    #[inline]
    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(SequenceHelper::new(self, fields.len()))
    }

    /// Enum values are encoded as unsigned longs (u32).
    #[inline]
    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.reader.align(4)?;
        visitor.visit_enum(EnumerationHelper::<BO>::new(self))
    }

    #[inline]
    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u32(visitor)
    }

    #[inline]
    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        false
    }
}

// ----------------------------------------------------------

struct EnumerationHelper<'a, 'de, BO> {
    de: &'a mut CdrDeserializer<'de, BO>,
}

impl<'a, 'de, BO> EnumerationHelper<'a, 'de, BO>
where
    BO: ByteOrder,
{
    #[inline]
    fn new(de: &'a mut CdrDeserializer<'de, BO>) -> Self {
        EnumerationHelper { de }
    }
}

impl<'de, 'a, BO> EnumAccess<'de> for EnumerationHelper<'a, 'de, BO>
where
    BO: ByteOrder,
{
    type Error = Error;
    type Variant = Self;

    #[inline]
    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
    where
        V: DeserializeSeed<'de>,
    {
        // preceding deserialize_enum aligned to 4
        let enum_tag = self.de.reader.read_u32()?;
        let val: Result<_> = seed.deserialize(enum_tag.into_deserializer());
        Ok((val?, self))
    }
}

impl<'de, 'a, BO> VariantAccess<'de> for EnumerationHelper<'a, 'de, BO>
where
    BO: ByteOrder,
{
    type Error = Error;

    #[inline]
    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    #[inline]
    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        seed.deserialize(self.de)
    }

    #[inline]
    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_tuple(self.de, len, visitor)
    }

    #[inline]
    fn struct_variant<V>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        de::Deserializer::deserialize_tuple(self.de, fields.len(), visitor)
    }
}

// ----------------------------------------------------------

struct SequenceHelper<'a, 'de, BO> {
    de: &'a mut CdrDeserializer<'de, BO>,
    element_counter: usize,
    expected_count: usize,
}

impl<'a, 'de, BO> SequenceHelper<'a, 'de, BO> {
    #[inline]
    fn new(de: &'a mut CdrDeserializer<'de, BO>, expected_count: usize) -> Self {
        SequenceHelper {
            de,
            element_counter: 0,
            expected_count,
        }
    }
}

impl<'a, 'de, BO> SeqAccess<'de> for SequenceHelper<'a, 'de, BO>
where
    BO: ByteOrder,
{
    type Error = Error;

    #[inline]
    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: DeserializeSeed<'de>,
    {
        if self.element_counter == self.expected_count {
            Ok(None)
        } else {
            self.element_counter += 1;
            seed.deserialize(&mut *self.de).map(Some)
        }
    }
}

impl<'de, 'a, BO> MapAccess<'de> for SequenceHelper<'a, 'de, BO>
where
    BO: ByteOrder,
{
    type Error = Error;

    #[inline]
    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: DeserializeSeed<'de>,
    {
        if self.element_counter == self.expected_count {
            Ok(None)
        } else {
            self.element_counter += 1;
            seed.deserialize(&mut *self.de).map(Some)
        }
    }

    #[inline]
    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: DeserializeSeed<'de>,
    {
        seed.deserialize(&mut *self.de)
    }
}
