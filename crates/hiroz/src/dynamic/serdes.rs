//! Serialization/deserialization implementations for dynamic messages.
//!
//! This module provides `DynamicSerdeCdrSerdes` which implements the `ZSerializer`
//! and `ZDeserializer` traits, allowing `DynamicMessage` to be used with
//! the standard `ZPub`/`ZSub` infrastructure.

use std::sync::Arc;

use zenoh_buffers::ZBuf;

use crate::msg::{ZDeserializer, ZSerializer};

use super::error::DynamicError;
use super::message::DynamicMessage;
use super::schema::MessageSchema;

/// CDR serializer/deserializer for `DynamicMessage`.
///
/// This type implements both `ZSerializer` and `ZDeserializer`, enabling
/// `DynamicMessage` to work with the standard pub/sub infrastructure.
///
/// # Example
///
/// ```ignore
/// use hiroz::dynamic::{DynamicMessage, DynamicSerdeCdrSerdes, MessageSchema};
/// use hiroz::pubsub::{ZPub, ZSub};
///
/// // Publisher - schema is embedded in DynamicMessage
/// let publisher: ZPub<DynamicMessage, DynamicSerdeCdrSerdes> = node
///     .create_pub("/topic")
///     .with_serdes::<DynamicSerdeCdrSerdes>()
///     .build()?;
///
/// // Subscriber - schema must be provided via with_dyn_schema()
/// let subscriber: ZSub<DynamicMessage, _, DynamicSerdeCdrSerdes> = node
///     .create_sub("/topic")
///     .with_serdes::<DynamicSerdeCdrSerdes>()
///     .with_dyn_schema(schema)
///     .build()?;
/// ```
pub struct DynamicSerdeCdrSerdes;

impl ZSerializer for DynamicSerdeCdrSerdes {
    type Input<'a> = &'a DynamicMessage;

    fn serialize_to_zbuf(input: &DynamicMessage) -> ZBuf {
        input
            .to_cdr_zbuf()
            .expect("DynamicMessage CDR serialization failed")
    }

    fn serialize_to_zbuf_with_hint(input: &DynamicMessage, _capacity_hint: usize) -> ZBuf {
        // DynamicMessage doesn't use capacity hints (it has its own serialization path)
        Self::serialize_to_zbuf(input)
    }

    fn serialize_to_shm(
        input: &DynamicMessage,
        _estimated_size: usize,
        provider: &zenoh::shm::ShmProvider<zenoh::shm::PosixShmProviderBackend>,
    ) -> zenoh::Result<(ZBuf, usize)> {
        // DynamicMessage uses primitives-based serialization, not serde
        // So we serialize to Vec first, then copy to SHM
        // This is similar to the protobuf approach
        let data = input.to_cdr().map_err(|e| {
            zenoh::Error::from(format!("DynamicMessage serialization failed: {}", e))
        })?;
        let actual_size = data.len();

        use zenoh::Wait;
        use zenoh::shm::{BlockOn, GarbageCollect};

        let mut shm_buf = provider
            .alloc(actual_size)
            .with_policy::<BlockOn<GarbageCollect>>()
            .wait()
            .map_err(|e| zenoh::Error::from(format!("SHM allocation failed: {}", e)))?;

        shm_buf[0..actual_size].copy_from_slice(&data);

        Ok((ZBuf::from(shm_buf), actual_size))
    }

    fn serialize(input: &DynamicMessage) -> Vec<u8> {
        input
            .to_cdr()
            .expect("DynamicMessage CDR serialization failed")
    }

    fn serialize_to_buf(input: &DynamicMessage, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend(
            input
                .to_cdr()
                .expect("DynamicMessage CDR serialization failed"),
        );
    }
}

impl ZDeserializer for DynamicSerdeCdrSerdes {
    type Input<'a> = (&'a [u8], &'a Arc<MessageSchema>);
    type Output = DynamicMessage;
    type Error = DynamicError;

    fn deserialize(input: Self::Input<'_>) -> Result<DynamicMessage, DynamicError> {
        let (bytes, schema) = input;
        DynamicMessage::from_cdr(bytes, schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::schema::{FieldType, MessageSchema};
    use zenoh_buffers::buffer::Buffer;

    fn create_point_schema() -> Arc<MessageSchema> {
        MessageSchema::builder("geometry_msgs/msg/Point")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap()
    }

    #[test]
    fn test_serialize_to_zbuf() {
        let schema = create_point_schema();
        let mut msg = DynamicMessage::new(&schema);
        msg.set("x", 1.0f64).unwrap();
        msg.set("y", 2.0f64).unwrap();
        msg.set("z", 3.0f64).unwrap();

        let zbuf = DynamicSerdeCdrSerdes::serialize_to_zbuf(&msg);
        assert!(zbuf.len() > 0);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let schema = create_point_schema();
        let mut msg = DynamicMessage::new(&schema);
        msg.set("x", 1.5f64).unwrap();
        msg.set("y", 2.5f64).unwrap();
        msg.set("z", 3.5f64).unwrap();

        // Serialize
        let bytes = DynamicSerdeCdrSerdes::serialize(&msg);

        // Deserialize
        let deserialized = DynamicSerdeCdrSerdes::deserialize((&bytes, &schema)).unwrap();

        assert_eq!(deserialized.get::<f64>("x").unwrap(), 1.5);
        assert_eq!(deserialized.get::<f64>("y").unwrap(), 2.5);
        assert_eq!(deserialized.get::<f64>("z").unwrap(), 3.5);
    }

    #[test]
    fn test_serialize_to_buf() {
        let schema = create_point_schema();
        let mut msg = DynamicMessage::new(&schema);
        msg.set("x", 1.0f64).unwrap();
        msg.set("y", 2.0f64).unwrap();
        msg.set("z", 3.0f64).unwrap();

        let mut buffer = Vec::new();
        DynamicSerdeCdrSerdes::serialize_to_buf(&msg, &mut buffer);

        // Should match serialize() output
        let direct = DynamicSerdeCdrSerdes::serialize(&msg);
        assert_eq!(buffer, direct);
    }
}
