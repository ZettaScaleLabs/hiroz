//! CDR serialization for dynamic messages.
//!
//! This module uses the low-level primitives from `hiroz-cdr` for CDR
//! serialization and deserialization of dynamic messages.

use std::sync::Arc;

use byteorder::ByteOrder;
use hiroz_cdr::{BigEndian, CdrReader, CdrWriter, LittleEndian};
use zenoh_buffers::ZBuf;

use crate::dynamic::error::DynamicError;
use crate::dynamic::message::{
    DynamicMessage, MAX_DYNAMIC_DEPTH, MAX_DYNAMIC_VALUES, validate_message,
};
use crate::dynamic::schema::{FieldType, MessageSchema};
use crate::dynamic::value::DynamicValue;

use super::CDR_HEADER_LE;

/// Serialize a dynamic message to CDR bytes.
pub fn serialize_cdr(msg: &DynamicMessage) -> Result<Vec<u8>, DynamicError> {
    validate_message(msg)?;
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
    let mut budget = DecodeBudget {
        remaining_values: MAX_DYNAMIC_VALUES,
        remaining_schema_nodes: MAX_DYNAMIC_VALUES,
    };
    deserialize_message(schema, &mut reader, "$", 0, &mut budget)
}

fn serialize_message(
    msg: &DynamicMessage,
    writer: &mut CdrWriter<LittleEndian>,
) -> Result<(), DynamicError> {
    if msg.schema().fields.is_empty() {
        writer.write_u8(0);
        return Ok(());
    }
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
        (DynamicValue::String(v), FieldType::WString | FieldType::BoundedWString(_)) => {
            write_wstring(v, writer)?;
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
        (DynamicValue::Bytes(bytes), FieldType::BoundedSequence(inner, _))
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
            return Err(DynamicError::SerializationError(
                "validated dynamic value did not match its schema".into(),
            ));
        }
    }
    Ok(())
}

fn deserialize_message<BO: ByteOrder>(
    schema: &Arc<MessageSchema>,
    reader: &mut CdrReader<BO>,
    path: &str,
    depth: usize,
    budget: &mut DecodeBudget,
) -> Result<DynamicMessage, DynamicError> {
    check_decode_depth(depth)?;
    if schema.fields.is_empty() {
        reader.read_u8().map_err(map_cdr_err)?;
        return Ok(DynamicMessage::from_values(schema, Vec::new()));
    }
    if schema.fields.len() > budget.remaining_values {
        return Err(value_budget_error());
    }
    let mut values = Vec::with_capacity(schema.fields.len());

    for field in &schema.fields {
        let field_path = join_path(path, &field.name);
        let value = deserialize_value(&field.field_type, reader, &field_path, depth, budget)?;
        values.push(value);
    }

    Ok(DynamicMessage::from_values(schema, values))
}

fn deserialize_value<BO: ByteOrder>(
    field_type: &FieldType,
    reader: &mut CdrReader<BO>,
    path: &str,
    depth: usize,
    budget: &mut DecodeBudget,
) -> Result<DynamicValue, DynamicError> {
    check_decode_depth(depth)?;
    budget.consume()?;
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
        FieldType::String => read_string(reader, None, path).map(DynamicValue::String),
        FieldType::BoundedString(max) => {
            read_string(reader, Some(*max), path).map(DynamicValue::String)
        }
        FieldType::WString => read_wstring(reader, None, path).map(DynamicValue::String),
        FieldType::BoundedWString(max) => {
            read_wstring(reader, Some(*max), path).map(DynamicValue::String)
        }

        // Fixed-size array
        FieldType::Array(inner, len) => {
            ensure_collection_feasible(*len, inner, reader, budget, depth + 1, path)?;
            let mut values = Vec::with_capacity(*len);
            for index in 0..*len {
                values.push(deserialize_value(
                    inner,
                    reader,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    budget,
                )?);
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
                let len = reader.read_sequence_length().map_err(map_cdr_err)?;
                if len > reader.remaining() {
                    return Err(DynamicError::DeserializationError(format!(
                        "byte sequence at '{path}' exceeds remaining payload"
                    )));
                }
                let bytes = reader.read_bytes(len).map_err(map_cdr_err)?.to_vec();
                return Ok(DynamicValue::Bytes(bytes));
            }

            let len = reader.read_sequence_length().map_err(map_cdr_err)?;
            ensure_collection_feasible(len, inner, reader, budget, depth + 1, path)?;
            let mut values = Vec::with_capacity(len);
            for index in 0..len {
                values.push(deserialize_value(
                    inner,
                    reader,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    budget,
                )?);
            }
            Ok(DynamicValue::Array(values))
        }

        // Bounded sequence
        FieldType::BoundedSequence(inner, max) => {
            // Same handling as unbounded sequence for deserialization
            if matches!(
                **inner,
                FieldType::Uint8 | FieldType::Char | FieldType::Byte
            ) {
                let len = reader.read_sequence_length().map_err(map_cdr_err)?;
                check_bound(len, *max, path)?;
                if len > reader.remaining() {
                    return Err(DynamicError::DeserializationError(format!(
                        "byte sequence at '{path}' exceeds remaining payload"
                    )));
                }
                let bytes = reader.read_bytes(len).map_err(map_cdr_err)?.to_vec();
                return Ok(DynamicValue::Bytes(bytes));
            }

            let len = reader.read_sequence_length().map_err(map_cdr_err)?;
            check_bound(len, *max, path)?;
            ensure_collection_feasible(len, inner, reader, budget, depth + 1, path)?;
            let mut values = Vec::with_capacity(len);
            for index in 0..len {
                values.push(deserialize_value(
                    inner,
                    reader,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    budget,
                )?);
            }
            Ok(DynamicValue::Array(values))
        }

        // Nested message
        FieldType::Message(schema) => {
            let msg = deserialize_message(schema, reader, path, depth + 1, budget)?;
            Ok(DynamicValue::Message(Box::new(msg)))
        }
    }
}

struct DecodeBudget {
    remaining_values: usize,
    remaining_schema_nodes: usize,
}

impl DecodeBudget {
    fn consume(&mut self) -> Result<(), DynamicError> {
        self.remaining_values = self
            .remaining_values
            .checked_sub(1)
            .ok_or_else(value_budget_error)?;
        Ok(())
    }

    fn consume_schema_node(&mut self) -> Result<(), DynamicError> {
        self.remaining_schema_nodes =
            self.remaining_schema_nodes.checked_sub(1).ok_or_else(|| {
                DynamicError::ResourceLimitExceeded(format!(
                    "schema traversal contains more than {MAX_DYNAMIC_VALUES} nodes"
                ))
            })?;
        Ok(())
    }
}

fn value_budget_error() -> DynamicError {
    DynamicError::ResourceLimitExceeded(format!(
        "message contains more than {MAX_DYNAMIC_VALUES} values"
    ))
}

fn check_decode_depth(depth: usize) -> Result<(), DynamicError> {
    if depth > MAX_DYNAMIC_DEPTH {
        return Err(DynamicError::ResourceLimitExceeded(format!(
            "nesting exceeds {MAX_DYNAMIC_DEPTH} levels"
        )));
    }
    Ok(())
}

fn ensure_collection_feasible<BO: ByteOrder>(
    len: usize,
    inner: &FieldType,
    reader: &CdrReader<BO>,
    budget: &mut DecodeBudget,
    depth: usize,
    path: &str,
) -> Result<(), DynamicError> {
    check_decode_depth(depth)?;
    if len > budget.remaining_values {
        return Err(value_budget_error());
    }
    let minimum = minimum_wire_size(inner, depth, budget)?
        .checked_mul(len)
        .ok_or_else(|| {
            DynamicError::DeserializationError(format!(
                "collection at '{path}' has a byte length overflow"
            ))
        })?;
    if minimum > reader.remaining() {
        return Err(DynamicError::DeserializationError(format!(
            "collection at '{path}' exceeds remaining payload"
        )));
    }
    Ok(())
}

fn minimum_wire_size(
    field_type: &FieldType,
    depth: usize,
    budget: &mut DecodeBudget,
) -> Result<usize, DynamicError> {
    check_decode_depth(depth)?;
    budget.consume_schema_node()?;
    let size = match field_type {
        FieldType::Bool
        | FieldType::Int8
        | FieldType::Uint8
        | FieldType::Char
        | FieldType::Byte => 1,
        FieldType::Int16 | FieldType::Uint16 | FieldType::WChar => 2,
        FieldType::Int32 | FieldType::Uint32 | FieldType::Float32 => 4,
        FieldType::Int64 | FieldType::Uint64 | FieldType::Float64 => 8,
        FieldType::String | FieldType::BoundedString(_) => 5,
        FieldType::WString | FieldType::BoundedWString(_) => 4,
        FieldType::Array(inner, len) => minimum_wire_size(inner, depth + 1, budget)?
            .checked_mul(*len)
            .ok_or_else(|| {
                DynamicError::DeserializationError("array byte length overflow".into())
            })?,
        FieldType::Sequence(_) | FieldType::BoundedSequence(_, _) => 4,
        FieldType::Message(schema) => {
            if schema.fields.is_empty() {
                return Ok(1);
            }
            let mut total = 0usize;
            for field in &schema.fields {
                total = total
                    .checked_add(minimum_wire_size(&field.field_type, depth + 1, budget)?)
                    .ok_or_else(|| {
                        DynamicError::DeserializationError(
                            "nested message byte length overflow".into(),
                        )
                    })?;
            }
            total
        }
    };
    Ok(size)
}

fn read_string<BO: ByteOrder>(
    reader: &mut CdrReader<BO>,
    max_bytes: Option<usize>,
    path: &str,
) -> Result<String, DynamicError> {
    let wire_len = reader.read_u32().map_err(map_cdr_err)? as usize;
    if wire_len == 0 {
        return Err(DynamicError::InvalidString {
            path: path.to_string(),
            reason: "CDR length must include a NUL terminator".into(),
        });
    }
    if wire_len > reader.remaining() {
        return Err(DynamicError::DeserializationError(format!(
            "string at '{path}' exceeds remaining payload"
        )));
    }
    let bytes = reader.read_bytes(wire_len).map_err(map_cdr_err)?;
    if bytes.last() != Some(&0) {
        return Err(DynamicError::InvalidString {
            path: path.to_string(),
            reason: "missing CDR NUL terminator".into(),
        });
    }
    let text_bytes = &bytes[..bytes.len() - 1];
    if text_bytes.contains(&0) {
        return Err(DynamicError::InvalidString {
            path: path.to_string(),
            reason: "contains an embedded NUL byte".into(),
        });
    }
    if let Some(max) = max_bytes {
        check_bound(text_bytes.len(), max, path)?;
    }
    std::str::from_utf8(text_bytes)
        .map(str::to_owned)
        .map_err(|error| DynamicError::InvalidString {
            path: path.to_string(),
            reason: format!("invalid UTF-8: {error}"),
        })
}

fn check_bound(actual: usize, max: usize, path: &str) -> Result<(), DynamicError> {
    if actual > max {
        return Err(DynamicError::FieldBoundExceeded {
            path: path.to_string(),
            max,
            actual,
        });
    }
    Ok(())
}

fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

fn write_wstring(value: &str, writer: &mut CdrWriter<LittleEndian>) -> Result<(), DynamicError> {
    let unit_count = value.encode_utf16().count();
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
    path: &str,
) -> Result<String, DynamicError> {
    let unit_count = reader.read_sequence_length().map_err(map_cdr_err)?;
    if let Some(max) = max_units
        && unit_count > max
    {
        return Err(DynamicError::FieldBoundExceeded {
            path: path.to_string(),
            max,
            actual: unit_count,
        });
    }
    let byte_count = unit_count.checked_mul(4).ok_or_else(|| {
        DynamicError::DeserializationError(format!(
            "wide string at '{path}' has a byte length overflow"
        ))
    })?;
    if byte_count > reader.remaining() {
        return Err(DynamicError::DeserializationError(format!(
            "wide string at '{path}' exceeds remaining payload"
        )));
    }

    let mut units = Vec::with_capacity(unit_count);
    for _ in 0..unit_count {
        let value = reader.read_u32().map_err(map_cdr_err)?;
        let unit = u16::try_from(value).map_err(|_| DynamicError::InvalidString {
            path: path.to_string(),
            reason: format!("code unit exceeds uint16: {value:#x}"),
        })?;
        if unit == 0 {
            return Err(DynamicError::InvalidString {
                path: path.to_string(),
                reason: "contains an embedded NUL word".into(),
            });
        }
        units.push(unit);
    }
    String::from_utf16(&units).map_err(|error| DynamicError::InvalidString {
        path: path.to_string(),
        reason: format!("invalid UTF-16: {error}"),
    })
}

/// Map hiroz-cdr errors to DynamicError.
fn map_cdr_err(e: hiroz_cdr::Error) -> DynamicError {
    DynamicError::DeserializationError(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_message_matches_ros_cdr_and_roundtrips() {
        let schema = MessageSchema::builder("std_msgs/msg/Empty")
            .build()
            .unwrap();
        let message = DynamicMessage::new(&schema);

        let bytes = serialize_cdr(&message).unwrap();
        assert_eq!(bytes, [0, 1, 0, 0, 0]);
        assert!(
            deserialize_cdr(&bytes, &schema)
                .unwrap()
                .values()
                .is_empty()
        );
        assert!(deserialize_cdr(&CDR_HEADER_LE, &schema).is_err());
    }

    #[test]
    fn nested_empty_consumes_its_synthetic_byte() {
        let empty = MessageSchema::builder("std_msgs/msg/Empty")
            .build()
            .unwrap();
        let schema = MessageSchema::builder("test_msgs/msg/EmptyThenByte")
            .field("empty", FieldType::Message(empty.clone()))
            .field("tail", FieldType::Uint8)
            .build()
            .unwrap();
        let mut message = DynamicMessage::new(&schema);
        message.set("tail", 0x7fu8).unwrap();

        let bytes = serialize_cdr(&message).unwrap();
        assert_eq!(bytes, [0, 1, 0, 0, 0, 0x7f]);
        let decoded = deserialize_cdr(&bytes, &schema).unwrap();
        assert_eq!(decoded.get::<u8>("tail").unwrap(), 0x7f);
        assert_eq!(empty.fixed_cdr_size(), Some(1));
    }

    #[test]
    fn empty_collections_keep_one_byte_per_element() {
        let empty = MessageSchema::builder("std_msgs/msg/Empty")
            .build()
            .unwrap();
        let schema = MessageSchema::builder("test_msgs/msg/EmptyCollections")
            .field(
                "fixed",
                FieldType::Array(Box::new(FieldType::Message(empty.clone())), 2),
            )
            .field(
                "sequence",
                FieldType::Sequence(Box::new(FieldType::Message(empty.clone()))),
            )
            .build()
            .unwrap();
        let mut message = DynamicMessage::new(&schema);
        message
            .set_dynamic(
                "sequence",
                DynamicValue::Array(vec![
                    DynamicValue::Message(Box::new(DynamicMessage::new(&empty))),
                    DynamicValue::Message(Box::new(DynamicMessage::new(&empty))),
                ]),
            )
            .unwrap();

        assert_eq!(
            serialize_cdr(&message).unwrap(),
            [0, 1, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0]
        );
    }

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
            Err(DynamicError::FieldBoundExceeded {
                max: 2,
                actual: 3,
                ..
            })
        ));

        let over_bound = cdr_words(false, &[3, 0xd83d, 0xde00, 0x61]);
        assert!(matches!(
            deserialize_cdr(&over_bound, &schema),
            Err(DynamicError::FieldBoundExceeded {
                max: 2,
                actual: 3,
                ..
            })
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
            vec![1, 0],
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
        let zero = message(FieldType::WChar, DynamicValue::Uint16(0));
        assert_eq!(serialize_cdr(&zero).unwrap(), [0, 1, 0, 0, 0, 0]);

        let value = message(FieldType::WChar, DynamicValue::Uint16(0x6c34));
        assert_eq!(serialize_cdr(&value).unwrap(), [0, 1, 0, 0, 0x34, 0x6c]);

        let schema = schema(FieldType::WChar);
        let decoded = deserialize_cdr(&[0, 0, 0, 0, 0x6c, 0x34], &schema).unwrap();
        assert_eq!(decoded.get::<u16>("data").unwrap(), 0x6c34);
    }

    #[test]
    fn malformed_narrow_strings_are_rejected() {
        fn payload(declared: u32, bytes: &[u8]) -> Vec<u8> {
            let mut payload = CDR_HEADER_LE.to_vec();
            payload.extend_from_slice(&declared.to_le_bytes());
            payload.extend_from_slice(bytes);
            payload
        }

        let string_schema = schema(FieldType::String);
        for bytes in [
            payload(0, &[]),
            payload(2, b"a"),
            payload(2, b"ab"),
            payload(4, b"a\0b\0"),
            payload(2, &[0xff, 0]),
        ] {
            assert!(deserialize_cdr(&bytes, &string_schema).is_err());
        }
        let decoded = deserialize_cdr(&payload(2, b"a\0"), &string_schema).unwrap();
        assert_eq!(decoded.get::<String>("data").unwrap(), "a");

        let bounded = schema(FieldType::BoundedString(1));
        assert!(matches!(
            deserialize_cdr(&payload(3, b"ab\0"), &bounded),
            Err(DynamicError::FieldBoundExceeded {
                path,
                max: 1,
                actual: 2
            }) if path == "$.data"
        ));
    }

    #[test]
    fn exact_bounds_keep_canonical_cdr_bytes() {
        let string = message(
            FieldType::BoundedString(4),
            DynamicValue::String("éé".into()),
        );
        assert_eq!(
            serialize_cdr(&string).unwrap(),
            [0, 1, 0, 0, 5, 0, 0, 0, 0xc3, 0xa9, 0xc3, 0xa9, 0]
        );

        let sequence = message(
            FieldType::BoundedSequence(Box::new(FieldType::Uint32), 2),
            DynamicValue::Array(vec![DynamicValue::Uint32(1), DynamicValue::Uint32(2)]),
        );
        assert_eq!(
            serialize_cdr(&sequence).unwrap(),
            [0, 1, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]
        );
    }

    #[test]
    fn bounded_sequence_decode_checks_bound_and_payload_before_allocation() {
        let bounded = schema(FieldType::BoundedSequence(Box::new(FieldType::Uint64), 2));
        assert!(matches!(
            deserialize_cdr(&cdr_words(false, &[3]), &bounded),
            Err(DynamicError::FieldBoundExceeded {
                path,
                max: 2,
                actual: 3
            }) if path == "$.data"
        ));

        let sequence = schema(FieldType::Sequence(Box::new(FieldType::Uint64)));
        assert!(deserialize_cdr(&cdr_words(false, &[1]), &sequence).is_err());

        let empty = MessageSchema::builder("test_msgs/msg/Empty")
            .build()
            .unwrap();
        let zero_width = schema(FieldType::Sequence(Box::new(FieldType::Message(empty))));
        assert!(matches!(
            deserialize_cdr(
                &cdr_words(false, &[(MAX_DYNAMIC_VALUES as u32) + 1]),
                &zero_width
            ),
            Err(DynamicError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn byte_buffers_are_bounded_by_payload_not_dynamic_value_count() {
        let schema = schema(FieldType::Sequence(Box::new(FieldType::Uint8)));
        let mut message = DynamicMessage::new(&schema);
        let bytes = vec![0x5a; MAX_DYNAMIC_VALUES + 1];
        message.values_mut()[0] = DynamicValue::Bytes(bytes.clone());
        let encoded = serialize_cdr(&message).unwrap();
        let decoded = deserialize_cdr(&encoded, &schema).unwrap();
        assert_eq!(
            decoded.get_dynamic("data").unwrap(),
            DynamicValue::Bytes(bytes)
        );
    }

    #[test]
    fn excessive_nested_decode_fails_closed() {
        let mut nested = MessageSchema::builder("test_msgs/msg/Leaf")
            .build()
            .unwrap();
        for index in 0..=MAX_DYNAMIC_DEPTH {
            nested = MessageSchema::builder(&format!("test_msgs/msg/Level{index}"))
                .field("child", FieldType::Message(nested))
                .build()
                .unwrap();
        }
        assert!(matches!(
            deserialize_cdr(&CDR_HEADER_LE, &nested),
            Err(DynamicError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn serialization_revalidates_mutably_accessible_values() {
        let schema = MessageSchema::builder("test_msgs/msg/Shapes")
            .field("fixed", FieldType::Array(Box::new(FieldType::Int32), 2))
            .field(
                "bounded_bytes",
                FieldType::BoundedSequence(Box::new(FieldType::Byte), 2),
            )
            .build()
            .unwrap();
        let mut message = DynamicMessage::new(&schema);
        message.values_mut()[0] = DynamicValue::Array(vec![DynamicValue::Int32(1)]);
        assert!(matches!(
            serialize_cdr(&message),
            Err(DynamicError::WrongArrayLength { path, .. }) if path == "$.fixed"
        ));

        message.values_mut()[0] =
            DynamicValue::Array(vec![DynamicValue::Int32(1), DynamicValue::Int32(2)]);
        message.values_mut()[1] = DynamicValue::Bytes(vec![1, 2]);
        assert!(serialize_cdr(&message).is_ok());
        message.values_mut()[1] = DynamicValue::Bytes(vec![1, 2, 3]);
        assert!(matches!(
            serialize_cdr(&message),
            Err(DynamicError::FieldBoundExceeded { path, .. }) if path == "$.bounded_bytes"
        ));
    }
}
