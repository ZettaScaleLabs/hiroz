//! CDR serialization for dynamic messages.
//!
//! This module uses the low-level primitives from `hiroz-cdr` for CDR
//! serialization and deserialization of dynamic messages.

use std::sync::Arc;

use byteorder::ByteOrder;
use hiroz_cdr::{BigEndian, CdrReader, CdrWriter, LittleEndian};
use zenoh_buffers::ZBuf;

use super::CDR_HEADER_LE;
use crate::dynamic::{
    error::DynamicError,
    limits::{MAX_DYNAMIC_ALLOCATION_BYTES, MAX_DYNAMIC_DEPTH, validate_schema},
    message::DynamicMessage,
    schema::{FieldType, MessageSchema},
    value::DynamicValue,
};

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
///
/// Checks collection bounds, nesting (64 levels), and a 64 MiB allocation budget
/// for dynamic value storage before allocating from schema or wire lengths.
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
    validate_schema(schema).map_err(DynamicError::DeserializationError)?;
    let mut budget = MAX_DYNAMIC_ALLOCATION_BYTES;
    deserialize_message(schema, &mut reader, &mut budget, 0)
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
        (DynamicValue::Uint8(v), FieldType::Byte | FieldType::Uint8) => writer.write_u8(*v),
        (DynamicValue::Uint16(v), FieldType::Uint16) => writer.write_u16(*v),
        (DynamicValue::Uint32(v), FieldType::Uint32) => writer.write_u32(*v),
        (DynamicValue::Uint64(v), FieldType::Uint64) => writer.write_u64(*v),
        (DynamicValue::Float32(v), FieldType::Float32) => writer.write_f32(*v),
        (DynamicValue::Float64(v), FieldType::Float64) => writer.write_f64(*v),
        (DynamicValue::String(v), FieldType::String) => writer.write_string(v),
        (DynamicValue::String(v), FieldType::BoundedString(max)) => {
            if v.len() > *max {
                return Err(DynamicError::SerializationError(
                    "bounded string too long".into(),
                ));
            }
            writer.write_string(v);
        }

        // Fixed-size array (no length prefix)
        (DynamicValue::Array(values), FieldType::Array(inner, len)) => {
            if values.len() != *len {
                return Err(DynamicError::SerializationError(
                    "incorrect fixed array length".into(),
                ));
            }
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
        (DynamicValue::Array(values), FieldType::BoundedSequence(inner, max)) => {
            if values.len() > *max {
                return Err(DynamicError::SerializationError(
                    "bounded sequence too long".into(),
                ));
            }
            writer.write_sequence_length(values.len());
            for v in values {
                serialize_value(v, inner, writer)?;
            }
        }

        // Optimized byte array
        (DynamicValue::Bytes(bytes), FieldType::Sequence(inner))
            if matches!(**inner, FieldType::Byte | FieldType::Uint8) =>
        {
            writer.write_bytes(bytes);
        }

        (DynamicValue::Bytes(bytes), FieldType::BoundedSequence(inner, max))
            if matches!(**inner, FieldType::Byte | FieldType::Uint8) =>
        {
            if bytes.len() > *max {
                return Err(DynamicError::SerializationError(
                    "bounded byte sequence too long".into(),
                ));
            }
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
    budget: &mut usize,
    depth: usize,
) -> Result<DynamicMessage, DynamicError> {
    check_depth(depth)?;
    let mut values = allocate_values(schema.fields.len(), budget)?;

    for field in &schema.fields {
        let value = deserialize_value(&field.field_type, reader, budget, depth + 1)?;
        values.push(value);
    }

    Ok(DynamicMessage::from_values(schema, values))
}

fn deserialize_value<BO: ByteOrder>(
    field_type: &FieldType,
    reader: &mut CdrReader<BO>,
    budget: &mut usize,
    depth: usize,
) -> Result<DynamicValue, DynamicError> {
    check_depth(depth)?;
    match field_type {
        FieldType::Bool => Ok(DynamicValue::Bool(reader.read_bool().map_err(map_cdr_err)?)),
        FieldType::Int8 => Ok(DynamicValue::Int8(reader.read_i8().map_err(map_cdr_err)?)),
        FieldType::Int16 => Ok(DynamicValue::Int16(reader.read_i16().map_err(map_cdr_err)?)),
        FieldType::Int32 => Ok(DynamicValue::Int32(reader.read_i32().map_err(map_cdr_err)?)),
        FieldType::Int64 => Ok(DynamicValue::Int64(reader.read_i64().map_err(map_cdr_err)?)),
        FieldType::Byte | FieldType::Uint8 => {
            Ok(DynamicValue::Uint8(reader.read_u8().map_err(map_cdr_err)?))
        }
        FieldType::Uint16 => Ok(DynamicValue::Uint16(
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
        FieldType::String | FieldType::BoundedString(_) => {
            // Borrow the payload first: even an invalid length prefix cannot
            // cause an allocation before the payload and schema bounds checks.
            let text = reader.read_str().map_err(map_cdr_err)?;
            if let FieldType::BoundedString(max) = field_type
                && text.len() > *max
            {
                return Err(decode_error("bounded string too long"));
            }
            charge(budget, text.len())?;
            let mut value = String::new();
            value
                .try_reserve_exact(text.len())
                .map_err(|e| decode_error(&e.to_string()))?;
            value.push_str(text);
            Ok(DynamicValue::String(value))
        }

        FieldType::Array(inner, len) => deserialize_array(inner, *len, reader, budget, depth),
        FieldType::Sequence(inner) | FieldType::BoundedSequence(inner, _) => {
            let len = reader.read_sequence_length().map_err(map_cdr_err)?;
            if let FieldType::BoundedSequence(_, max) = field_type
                && len > *max
            {
                return Err(decode_error("bounded sequence too long"));
            }
            if matches!(**inner, FieldType::Byte | FieldType::Uint8) {
                let bytes = reader.read_bytes(len).map_err(map_cdr_err)?;
                charge(budget, len)?;
                let mut value = Vec::new();
                value
                    .try_reserve_exact(len)
                    .map_err(|e| decode_error(&e.to_string()))?;
                value.extend_from_slice(bytes);
                return Ok(DynamicValue::Bytes(value));
            }
            deserialize_array(inner, len, reader, budget, depth)
        }

        // Nested message
        FieldType::Message(schema) => {
            charge(budget, std::mem::size_of::<DynamicMessage>())?;
            let msg = deserialize_message(schema, reader, budget, depth + 1)?;
            Ok(DynamicValue::Message(Box::new(msg)))
        }
    }
}

fn decode_error(message: &str) -> DynamicError {
    DynamicError::DeserializationError(message.to_owned())
}

fn check_depth(depth: usize) -> Result<(), DynamicError> {
    if depth > MAX_DYNAMIC_DEPTH {
        return Err(decode_error("dynamic CDR nesting exceeds 64 levels"));
    }
    Ok(())
}

fn charge(budget: &mut usize, bytes: usize) -> Result<(), DynamicError> {
    *budget = budget
        .checked_sub(bytes)
        .ok_or_else(|| decode_error("dynamic CDR exceeds the 64 MiB allocation limit"))?;
    Ok(())
}

fn allocate_values(len: usize, budget: &mut usize) -> Result<Vec<DynamicValue>, DynamicError> {
    let bytes = len
        .checked_mul(std::mem::size_of::<DynamicValue>())
        .ok_or_else(|| decode_error("dynamic collection allocation overflow"))?;
    charge(budget, bytes)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|e| decode_error(&e.to_string()))?;
    Ok(values)
}

fn deserialize_array<BO: ByteOrder>(
    inner: &FieldType,
    len: usize,
    reader: &mut CdrReader<BO>,
    budget: &mut usize,
    depth: usize,
) -> Result<DynamicValue, DynamicError> {
    let minimum = minimum_wire_size(inner);
    if minimum > 0 && len > reader.remaining() / minimum {
        return Err(decode_error(
            "collection length exceeds remaining CDR payload",
        ));
    }
    // Even zero-sized message sequences have a bounded allocation budget.
    let mut values = allocate_values(len, budget)?;
    for _ in 0..len {
        values.push(deserialize_value(inner, reader, budget, depth + 1)?);
    }
    Ok(DynamicValue::Array(values))
}

/// A lower bound which ignores alignment padding. No length prefix may allocate
/// more non-empty elements than can fit in the remaining wire payload.
fn minimum_wire_size(field: &FieldType) -> usize {
    match field {
        FieldType::Bool | FieldType::Int8 | FieldType::Byte | FieldType::Uint8 => 1,
        FieldType::Int16 | FieldType::Uint16 => 2,
        FieldType::Int32 | FieldType::Uint32 | FieldType::Float32 => 4,
        FieldType::Int64 | FieldType::Uint64 | FieldType::Float64 => 8,
        FieldType::String
        | FieldType::BoundedString(_)
        | FieldType::Sequence(_)
        | FieldType::BoundedSequence(_, _) => 4,
        FieldType::Array(inner, len) => minimum_wire_size(inner).saturating_mul(*len),
        FieldType::Message(schema) => schema.fields.iter().fold(0usize, |size, field| {
            size.saturating_add(minimum_wire_size(&field.field_type))
        }),
    }
}

/// Map hiroz-cdr errors to DynamicError.
fn map_cdr_err(e: hiroz_cdr::Error) -> DynamicError {
    DynamicError::DeserializationError(e.to_string())
}
