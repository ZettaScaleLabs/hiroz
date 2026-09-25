//! CDR serialization for dynamic messages.
//!
//! This module uses the low-level primitives from `hiroz-cdr` for CDR
//! serialization and deserialization of dynamic messages.

use std::sync::Arc;

use byteorder::ByteOrder;
use hiroz_cdr::{BigEndian, CdrReader, CdrWriter, LittleEndian};
use zenoh_buffers::ZBuf;

use crate::dynamic::error::DynamicError;
use crate::dynamic::message::DynamicMessage;
use crate::dynamic::schema::{FieldType, MessageSchema};
use crate::dynamic::value::DynamicValue;

use super::CDR_HEADER_LE;

/// Serialize a dynamic message to CDR bytes.
pub fn serialize_cdr(msg: &DynamicMessage) -> Result<Vec<u8>, DynamicError> {
    let mut buffer = Vec::with_capacity(256);
    buffer.extend_from_slice(&CDR_HEADER_LE);

    let mut writer = CdrWriter::<LittleEndian>::new(&mut buffer);
    serialize_message(msg, &mut writer)?;

    Ok(buffer)
}

/// Serialize a dynamic message to a ZBuf.
pub fn serialize_cdr_to_zbuf(msg: &DynamicMessage) -> Result<ZBuf, DynamicError> {
    let bytes = serialize_cdr(msg)?;
    Ok(ZBuf::from(bytes))
}

/// Deserialize a dynamic message from CDR bytes.
pub fn deserialize_cdr(
    data: &[u8],
    schema: &Arc<MessageSchema>,
) -> Result<DynamicMessage, DynamicError> {
    if data.len() < 4 {
        return Err(DynamicError::DeserializationError(
            "CDR data too short for header".into(),
        ));
    }
    let header = &data[0..4];
    let representation_identifier = [header[0], header[1]];
    match representation_identifier {
        [0x00, 0x01] => deserialize_payload::<LittleEndian>(&data[4..], schema),
        [0x00, 0x00] => deserialize_payload::<BigEndian>(&data[4..], schema),
        other => Err(DynamicError::DeserializationError(format!(
            "Unsupported CDR encapsulation identifier: {other:?}"
        ))),
    }
}

fn deserialize_payload<BO: ByteOrder>(
    payload: &[u8],
    schema: &Arc<MessageSchema>,
) -> Result<DynamicMessage, DynamicError> {
    let mut reader = CdrReader::<BO>::new(payload);
    deserialize_message(schema, &mut reader)
}

fn serialize_message(
    msg: &DynamicMessage,
    writer: &mut CdrWriter<LittleEndian>,
) -> Result<(), DynamicError> {
    for (field, value) in msg.schema().fields.iter().zip(msg.values().iter()) {
        serialize_value(value, &field.field_type, writer)?;
    }
    Ok(())
}

fn serialize_value(
    value: &DynamicValue,
    field_type: &FieldType,
    writer: &mut CdrWriter<LittleEndian>,
) -> Result<(), DynamicError> {
    match (value, field_type) {
        (DynamicValue::Bool(v), FieldType::Bool) => writer.write_bool(*v),
        (DynamicValue::Int8(v), FieldType::Int8) => writer.write_i8(*v),
        (DynamicValue::Int16(v), FieldType::Int16) => writer.write_i16(*v),
        (DynamicValue::Int32(v), FieldType::Int32) => writer.write_i32(*v),
        (DynamicValue::Int64(v), FieldType::Int64) => writer.write_i64(*v),
        (DynamicValue::Uint8(v), FieldType::Uint8) => writer.write_u8(*v),
        (DynamicValue::Uint8(v), FieldType::Char) => writer.write_u8(*v),
        (DynamicValue::Uint8(v), FieldType::Byte) => writer.write_u8(*v),
        (DynamicValue::Uint16(v), FieldType::Uint16) => writer.write_u16(*v),
        (DynamicValue::Uint16(v), FieldType::WChar) => writer.write_u16(*v),
        (DynamicValue::Uint32(v), FieldType::Uint32) => writer.write_u32(*v),
        (DynamicValue::Uint64(v), FieldType::Uint64) => writer.write_u64(*v),
        (DynamicValue::Float32(v), FieldType::Float32) => writer.write_f32(*v),
        (DynamicValue::Float64(v), FieldType::Float64) => writer.write_f64(*v),
        (DynamicValue::String(v), FieldType::String) => writer.write_string(v),
        (DynamicValue::String(v), FieldType::BoundedString(_)) => writer.write_string(v),
        (DynamicValue::String(v), FieldType::WString) => write_wstring(v, None, writer)?,
        (DynamicValue::String(v), FieldType::BoundedWString(max)) => {
            write_wstring(v, Some(*max), writer)?;
        }

        // Fixed-size array (no length prefix)
        (DynamicValue::Array(values), FieldType::Array(inner, _len)) => {
            for v in values {
                serialize_value(v, inner, writer)?;
            }
        }

        // Sequence (with length prefix)
        (DynamicValue::Array(values), FieldType::Sequence(inner)) => {
            writer.write_sequence_length(values.len());
            for v in values {
                serialize_value(v, inner, writer)?;
            }
        }

        // Bounded sequence (with length prefix)
        (DynamicValue::Array(values), FieldType::BoundedSequence(inner, _max)) => {
            writer.write_sequence_length(values.len());
            for v in values {
                serialize_value(v, inner, writer)?;
            }
        }

        // Optimized byte array
        (DynamicValue::Bytes(bytes), FieldType::Sequence(inner))
            if matches!(
                **inner,
                FieldType::Uint8 | FieldType::Char | FieldType::Byte
            ) =>
        {
            writer.write_bytes(bytes);
        }

        // Nested message
        (DynamicValue::Message(nested), FieldType::Message(_)) => {
            serialize_message(nested, writer)?;
        }

        _ => {
            return Err(DynamicError::SerializationError(format!(
                "Type mismatch: cannot serialize {:?} as {:?}",
                value, field_type
            )));
        }
    }
    Ok(())
}

fn deserialize_message<BO: ByteOrder>(
    schema: &Arc<MessageSchema>,
    reader: &mut CdrReader<BO>,
) -> Result<DynamicMessage, DynamicError> {
    let mut values = Vec::with_capacity(schema.fields.len());

    for field in &schema.fields {
        let value = deserialize_value(&field.field_type, reader)?;
        values.push(value);
    }

    Ok(DynamicMessage::from_values(schema, values))
}

fn deserialize_value<BO: ByteOrder>(
    field_type: &FieldType,
    reader: &mut CdrReader<BO>,
) -> Result<DynamicValue, DynamicError> {
    match field_type {
        FieldType::Bool => Ok(DynamicValue::Bool(reader.read_bool().map_err(map_cdr_err)?)),
        FieldType::Int8 => Ok(DynamicValue::Int8(reader.read_i8().map_err(map_cdr_err)?)),
        FieldType::Int16 => Ok(DynamicValue::Int16(reader.read_i16().map_err(map_cdr_err)?)),
        FieldType::Int32 => Ok(DynamicValue::Int32(reader.read_i32().map_err(map_cdr_err)?)),
        FieldType::Int64 => Ok(DynamicValue::Int64(reader.read_i64().map_err(map_cdr_err)?)),
        FieldType::Uint8 | FieldType::Char | FieldType::Byte => {
            Ok(DynamicValue::Uint8(reader.read_u8().map_err(map_cdr_err)?))
        }
        FieldType::Uint16 | FieldType::WChar => Ok(DynamicValue::Uint16(
            reader.read_u16().map_err(map_cdr_err)?,
        )),
        FieldType::Uint32 => Ok(DynamicValue::Uint32(
            reader.read_u32().map_err(map_cdr_err)?,
        )),
        FieldType::Uint64 => Ok(DynamicValue::Uint64(
            reader.read_u64().map_err(map_cdr_err)?,
        )),
        FieldType::Float32 => Ok(DynamicValue::Float32(
            reader.read_f32().map_err(map_cdr_err)?,
        )),
        FieldType::Float64 => Ok(DynamicValue::Float64(
            reader.read_f64().map_err(map_cdr_err)?,
        )),
        FieldType::String | FieldType::BoundedString(_) => Ok(DynamicValue::String(
            reader.read_string().map_err(map_cdr_err)?,
        )),
        FieldType::WString => read_wstring(reader, None).map(DynamicValue::String),
        FieldType::BoundedWString(max) => {
            read_wstring(reader, Some(*max)).map(DynamicValue::String)
        }

        // Fixed-size array
        FieldType::Array(inner, len) => {
            let mut values = Vec::with_capacity(*len);
            for _ in 0..*len {
                values.push(deserialize_value(inner, reader)?);
            }
            Ok(DynamicValue::Array(values))
        }

        // Sequence
        FieldType::Sequence(inner) => {
            // Optimize for byte arrays
            if matches!(
                **inner,
                FieldType::Uint8 | FieldType::Char | FieldType::Byte
            ) {
                let bytes = reader.read_byte_sequence().map_err(map_cdr_err)?.to_vec();
                return Ok(DynamicValue::Bytes(bytes));
            }

            let len = reader.read_sequence_length().map_err(map_cdr_err)?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(deserialize_value(inner, reader)?);
            }
            Ok(DynamicValue::Array(values))
        }

        // Bounded sequence
        FieldType::BoundedSequence(inner, _max) => {
            // Same handling as unbounded sequence for deserialization
            if matches!(
                **inner,
                FieldType::Uint8 | FieldType::Char | FieldType::Byte
            ) {
                let bytes = reader.read_byte_sequence().map_err(map_cdr_err)?.to_vec();
                return Ok(DynamicValue::Bytes(bytes));
            }

            let len = reader.read_sequence_length().map_err(map_cdr_err)?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(deserialize_value(inner, reader)?);
            }
            Ok(DynamicValue::Array(values))
        }

        // Nested message
        FieldType::Message(schema) => {
            let msg = deserialize_message(schema, reader)?;
            Ok(DynamicValue::Message(Box::new(msg)))
        }
    }
}

fn write_wstring(
    value: &str,
    max_units: Option<usize>,
    writer: &mut CdrWriter<LittleEndian>,
) -> Result<(), DynamicError> {
    let unit_count = value.encode_utf16().count();
    if let Some(max) = max_units
        && unit_count > max
    {
        return Err(DynamicError::BoundExceeded {
            max,
            actual: unit_count,
        });
    }
    let wire_len = u32::try_from(unit_count).map_err(|_| {
        DynamicError::SerializationError("wide string length exceeds uint32".into())
    })?;
    writer.write_u32(wire_len);
    for unit in value.encode_utf16() {
        writer.write_u32(u32::from(unit));
    }
    Ok(())
}

fn read_wstring<BO: ByteOrder>(
    reader: &mut CdrReader<BO>,
    max_units: Option<usize>,
) -> Result<String, DynamicError> {
    let unit_count = reader.read_sequence_length().map_err(map_cdr_err)?;
    if let Some(max) = max_units
        && unit_count > max
    {
        return Err(DynamicError::BoundExceeded {
            max,
            actual: unit_count,
        });
    }
    let byte_count = unit_count.checked_mul(4).ok_or_else(|| {
        DynamicError::DeserializationError("wide string byte length overflow".into())
    })?;
    if byte_count > reader.remaining() {
        return Err(DynamicError::DeserializationError(
            "wide string length exceeds remaining payload".into(),
        ));
    }

    let mut units = Vec::with_capacity(unit_count);
    for _ in 0..unit_count {
        let value = reader.read_u32().map_err(map_cdr_err)?;
        units.push(u16::try_from(value).map_err(|_| {
            DynamicError::DeserializationError(format!(
                "wide string code unit exceeds uint16: {value:#x}"
            ))
        })?);
    }
    String::from_utf16(&units).map_err(|error| {
        DynamicError::DeserializationError(format!("invalid UTF-16 wide string: {error}"))
    })
}

/// Map hiroz-cdr errors to DynamicError.
fn map_cdr_err(e: hiroz_cdr::Error) -> DynamicError {
    DynamicError::DeserializationError(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(field_type: FieldType) -> Arc<MessageSchema> {
        MessageSchema::builder("test_msgs/msg/Wide")
            .field("data", field_type)
            .build()
            .unwrap()
    }

    fn message(field_type: FieldType, value: DynamicValue) -> DynamicMessage {
        let schema = schema(field_type);
        let mut message = DynamicMessage::new(&schema);
        message.set_dynamic("data", value).unwrap();
        message
    }

    fn cdr_words(big_endian: bool, words: &[u32]) -> Vec<u8> {
        let mut bytes = if big_endian {
            vec![0x00, 0x00, 0x00, 0x00]
        } else {
            vec![0x00, 0x01, 0x00, 0x00]
        };
        for word in words {
            bytes.extend(if big_endian {
                word.to_be_bytes()
            } else {
                word.to_le_bytes()
            });
        }
        bytes
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(text, 16).unwrap()
            })
            .collect()
    }

    #[test]
    fn ros_rclpy_wstring_fixtures_decode() {
        // Captured with rclpy.serialization.serialize_message from current Jazzy
        // and Lyrical fixtures. These captures contained an extra four-byte tail
        // beyond the count and code units; the field consumes only its declared units.
        let fixtures = [
            ("", "000100000000000000000000"),
            ("A水", "000100000200000041000000346c000065733a3a"),
            (
                "A😀水",
                "0001000004000000410000003dd8000000de0000346c00003a646473",
            ),
        ];
        let schema = schema(FieldType::WString);
        for (expected, hex) in fixtures {
            let bytes = decode_hex(hex);
            let decoded = deserialize_cdr(&bytes, &schema).unwrap();
            assert_eq!(decoded.get::<String>("data").unwrap(), expected);
        }
    }

    #[test]
    fn wstring_encoding_matches_ros_field_bytes() {
        for (text, units) in [
            ("", vec![]),
            ("A水", vec![0x41, 0x6c34]),
            ("A😀水", vec![0x41, 0xd83d, 0xde00, 0x6c34]),
        ] {
            let value = message(FieldType::WString, DynamicValue::String(text.into()));
            let expected = cdr_words(
                false,
                &std::iter::once(units.len() as u32)
                    .chain(units)
                    .collect::<Vec<_>>(),
            );
            assert_eq!(serialize_cdr(&value).unwrap(), expected);
        }
    }

    #[test]
    fn wstring_decodes_big_endian_fixture_bytes() {
        let bytes = cdr_words(false, &[4, 0x41, 0xd83d, 0xde00, 0x6c34]);
        let big_endian = cdr_words(true, &[4, 0x41, 0xd83d, 0xde00, 0x6c34]);
        let schema = schema(FieldType::WString);
        let little = deserialize_cdr(&bytes, &schema).unwrap();
        let big = deserialize_cdr(&big_endian, &schema).unwrap();
        assert_eq!(little.get::<String>("data").unwrap(), "A😀水");
        assert_eq!(big.get::<String>("data").unwrap(), "A😀水");
    }

    #[test]
    fn bounded_wstring_counts_utf16_units() {
        let accepted = message(
            FieldType::BoundedWString(2),
            DynamicValue::String("😀".into()),
        );
        assert!(serialize_cdr(&accepted).is_ok());

        let schema = schema(FieldType::BoundedWString(2));
        let mut rejected = DynamicMessage::new(&schema);
        assert!(matches!(
            rejected.set("data", "😀a"),
            Err(DynamicError::BoundExceeded { max: 2, actual: 3 })
        ));

        let over_bound = cdr_words(false, &[3, 0xd83d, 0xde00, 0x61]);
        assert!(matches!(
            deserialize_cdr(&over_bound, &schema),
            Err(DynamicError::BoundExceeded { max: 2, actual: 3 })
        ));
    }

    #[test]
    fn wstring_arrays_and_sequences_roundtrip() {
        let schema = MessageSchema::builder("test_msgs/msg/WideCollections")
            .field("fixed", FieldType::Array(Box::new(FieldType::WString), 2))
            .field(
                "bounded",
                FieldType::BoundedSequence(Box::new(FieldType::BoundedWString(2)), 2),
            )
            .field("values", FieldType::Sequence(Box::new(FieldType::WString)))
            .build()
            .unwrap();
        let mut original = DynamicMessage::new(&schema);
        original
            .set_dynamic(
                "fixed",
                DynamicValue::Array(vec![
                    DynamicValue::String("A".into()),
                    DynamicValue::String("水".into()),
                ]),
            )
            .unwrap();
        original
            .set_dynamic(
                "bounded",
                DynamicValue::Array(vec![DynamicValue::String("😀".into())]),
            )
            .unwrap();
        original
            .set_dynamic(
                "values",
                DynamicValue::Array(vec![
                    DynamicValue::String(String::new()),
                    DynamicValue::String("A😀水".into()),
                ]),
            )
            .unwrap();

        let bytes = serialize_cdr(&original).unwrap();
        let decoded = deserialize_cdr(&bytes, &schema).unwrap();
        assert_eq!(decoded.values(), original.values());
    }

    #[test]
    fn malformed_wstrings_are_rejected_before_allocation() {
        let schema = schema(FieldType::WString);
        let valid = cdr_words(false, &[2, 0xd83d, 0xde00]);
        for end in 0..valid.len() {
            assert!(deserialize_cdr(&valid[..end], &schema).is_err());
        }
        for words in [
            vec![1, 0x1_0000],
            vec![1, 0xd800],
            vec![1, 0xdc00],
            vec![2, 0xd800, 0x41],
        ] {
            assert!(deserialize_cdr(&cdr_words(false, &words), &schema).is_err());
            assert!(deserialize_cdr(&cdr_words(true, &words), &schema).is_err());
        }
        assert!(deserialize_cdr(&cdr_words(false, &[u32::MAX]), &schema).is_err());
        assert!(deserialize_cdr(&cdr_words(true, &[u32::MAX]), &schema).is_err());
    }

    #[test]
    fn native_wchar_uses_the_ros_fast_cdr_u16_wire_width() {
        let value = message(FieldType::WChar, DynamicValue::Uint16(0x6c34));
        assert_eq!(serialize_cdr(&value).unwrap(), [0, 1, 0, 0, 0x34, 0x6c]);

        let schema = schema(FieldType::WChar);
        let decoded = deserialize_cdr(&[0, 0, 0, 0, 0x6c, 0x34], &schema).unwrap();
        assert_eq!(decoded.get::<u16>("data").unwrap(), 0x6c34);
    }
}
