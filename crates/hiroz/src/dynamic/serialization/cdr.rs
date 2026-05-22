//! CDR serialization for dynamic messages.
//!
//! This module uses the low-level primitives from `hiroz-cdr` for CDR
//! serialization and deserialization of dynamic messages.

use std::sync::Arc;

use hiroz_cdr::{CdrReader, CdrWriter, LittleEndian};
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
    let representation_identifier = &header[0..2];
    if representation_identifier != [0x00, 0x01] {
        return Err(DynamicError::DeserializationError(format!(
            "Expected CDR_LE encapsulation ({:?}), found {:?}",
            [0x00, 0x01],
            representation_identifier
        )));
    }

    let payload = &data[4..];
    let mut reader = CdrReader::<LittleEndian>::new(payload);
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
        (DynamicValue::Uint16(v), FieldType::Uint16) => writer.write_u16(*v),
        (DynamicValue::Uint32(v), FieldType::Uint32) => writer.write_u32(*v),
        (DynamicValue::Uint64(v), FieldType::Uint64) => writer.write_u64(*v),
        (DynamicValue::Float32(v), FieldType::Float32) => writer.write_f32(*v),
        (DynamicValue::Float64(v), FieldType::Float64) => writer.write_f64(*v),
        (DynamicValue::String(v), FieldType::String) => writer.write_string(v),
        (DynamicValue::String(v), FieldType::BoundedString(_)) => writer.write_string(v),

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
            if matches!(**inner, FieldType::Uint8) =>
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

fn deserialize_message(
    schema: &Arc<MessageSchema>,
    reader: &mut CdrReader<LittleEndian>,
) -> Result<DynamicMessage, DynamicError> {
    let mut values = Vec::with_capacity(schema.fields.len());

    for field in &schema.fields {
        let value = deserialize_value(&field.field_type, reader)?;
        values.push(value);
    }

    Ok(DynamicMessage::from_values(schema, values))
}

fn deserialize_value(
    field_type: &FieldType,
    reader: &mut CdrReader<LittleEndian>,
) -> Result<DynamicValue, DynamicError> {
    match field_type {
        FieldType::Bool => Ok(DynamicValue::Bool(reader.read_bool().map_err(map_cdr_err)?)),
        FieldType::Int8 => Ok(DynamicValue::Int8(reader.read_i8().map_err(map_cdr_err)?)),
        FieldType::Int16 => Ok(DynamicValue::Int16(reader.read_i16().map_err(map_cdr_err)?)),
        FieldType::Int32 => Ok(DynamicValue::Int32(reader.read_i32().map_err(map_cdr_err)?)),
        FieldType::Int64 => Ok(DynamicValue::Int64(reader.read_i64().map_err(map_cdr_err)?)),
        FieldType::Uint8 => Ok(DynamicValue::Uint8(reader.read_u8().map_err(map_cdr_err)?)),
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
        FieldType::String | FieldType::BoundedString(_) => Ok(DynamicValue::String(
            reader.read_string().map_err(map_cdr_err)?,
        )),

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
            if matches!(**inner, FieldType::Uint8) {
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
            if matches!(**inner, FieldType::Uint8) {
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

/// Map hiroz-cdr errors to DynamicError.
fn map_cdr_err(e: hiroz_cdr::Error) -> DynamicError {
    DynamicError::DeserializationError(e.to_string())
}
