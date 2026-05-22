//! Serialization support for dynamic messages.
//!
//! This module provides serialization and deserialization for dynamic
//! messages, supporting multiple formats (CDR is the default).

mod cdr;

pub use cdr::{deserialize_cdr, serialize_cdr, serialize_cdr_to_zbuf};

use std::sync::Arc;

use zenoh_buffers::ZBuf;

use super::error::DynamicError;
use super::message::DynamicMessage;
use super::schema::MessageSchema;

/// Supported serialization formats for dynamic messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SerializationFormat {
    #[default]
    Cdr,
    #[cfg(feature = "protobuf")]
    Protobuf,
}

/// CDR encapsulation header for little-endian encoding.
pub const CDR_HEADER_LE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

impl DynamicMessage {
    /// Serialize the message to bytes using the specified format.
    pub fn serialize(&self, format: SerializationFormat) -> Result<Vec<u8>, DynamicError> {
        match format {
            SerializationFormat::Cdr => serialize_cdr(self),
            #[cfg(feature = "protobuf")]
            SerializationFormat::Protobuf => Err(DynamicError::SerializationError(
                "Protobuf serialization is not yet implemented for dynamic messages".to_string(),
            )),
        }
    }

    /// Serialize the message to an existing buffer.
    pub fn serialize_to_buf(
        &self,
        format: SerializationFormat,
        buffer: &mut Vec<u8>,
    ) -> Result<(), DynamicError> {
        buffer.clear();
        let data = self.serialize(format)?;
        buffer.extend_from_slice(&data);
        Ok(())
    }

    /// Serialize the message to a ZBuf (zero-copy where possible).
    pub fn serialize_to_zbuf(&self, format: SerializationFormat) -> Result<ZBuf, DynamicError> {
        match format {
            SerializationFormat::Cdr => serialize_cdr_to_zbuf(self),
            #[cfg(feature = "protobuf")]
            SerializationFormat::Protobuf => Err(DynamicError::SerializationError(
                "Protobuf serialization is not yet implemented for dynamic messages".to_string(),
            )),
        }
    }

    /// Deserialize a message from bytes.
    pub fn deserialize(
        data: &[u8],
        schema: &Arc<MessageSchema>,
        format: SerializationFormat,
    ) -> Result<Self, DynamicError> {
        match format {
            SerializationFormat::Cdr => deserialize_cdr(data, schema),
            #[cfg(feature = "protobuf")]
            SerializationFormat::Protobuf => Err(DynamicError::DeserializationError(
                "Protobuf deserialization is not yet implemented for dynamic messages".to_string(),
            )),
        }
    }

    // Convenience methods with default CDR format

    /// Serialize to CDR format.
    pub fn to_cdr(&self) -> Result<Vec<u8>, DynamicError> {
        serialize_cdr(self)
    }

    /// Serialize to CDR format as ZBuf.
    pub fn to_cdr_zbuf(&self) -> Result<ZBuf, DynamicError> {
        serialize_cdr_to_zbuf(self)
    }

    /// Deserialize from CDR format.
    pub fn from_cdr(data: &[u8], schema: &Arc<MessageSchema>) -> Result<Self, DynamicError> {
        deserialize_cdr(data, schema)
    }
}
