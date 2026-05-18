//! Raw bytes message type for generic Python ↔ Rust bridging.
//!
//! This module provides `RawBytesMessage` — a message type that passes CDR bytes
//! through without transformation. This eliminates the need for per-type factory
//! match statements, allowing any registered message type to work with pub/sub
//! and services.

use hiroz::msg::{SerdeCdrSerdes, ZDeserializer, ZMessage, ZSerializer, ZService};
use zenoh_buffers::ZBuf;

/// A message type that wraps raw CDR bytes and passes them through unchanged.
///
/// This is the key to the generic Python bridge: Python handles CDR
/// serialization/deserialization via msgspec, and Rust just forwards bytes.
#[derive(Clone)]
pub struct RawBytesMessage(pub Vec<u8>);

/// Identity CDR serializer/deserializer — passes bytes through unchanged.
pub struct RawBytesCdrSerdes;

// Manual ZMessage impl (no Serialize/Deserialize, so blanket impl doesn't apply)
impl ZMessage for RawBytesMessage {
    type Serdes = RawBytesCdrSerdes;

    fn estimated_serialized_size(&self) -> usize {
        self.0.len()
    }
}

impl ZSerializer for RawBytesCdrSerdes {
    type Input<'a> = &'a RawBytesMessage;

    fn serialize_to_zbuf(input: &RawBytesMessage) -> ZBuf {
        ZBuf::from(input.0.clone())
    }

    fn serialize_to_zbuf_with_hint(input: &RawBytesMessage, _capacity_hint: usize) -> ZBuf {
        ZBuf::from(input.0.clone())
    }

    fn serialize_to_shm(
        input: &RawBytesMessage,
        _estimated_size: usize,
        provider: &zenoh::shm::ShmProvider<zenoh::shm::PosixShmProviderBackend>,
    ) -> zenoh::Result<(ZBuf, usize)> {
        use zenoh::Wait;
        use zenoh::shm::{BlockOn, GarbageCollect};

        let data = &input.0;
        let actual_size = data.len();

        let mut shm_buf = provider
            .alloc(actual_size)
            .with_policy::<BlockOn<GarbageCollect>>()
            .wait()
            .map_err(|e| zenoh::Error::from(format!("SHM allocation failed: {}", e)))?;

        shm_buf[0..actual_size].copy_from_slice(data);

        Ok((ZBuf::from(shm_buf), actual_size))
    }

    fn serialize_to_buf(input: &RawBytesMessage, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.extend_from_slice(&input.0);
    }
}

impl ZDeserializer for RawBytesCdrSerdes {
    type Input<'a> = &'a [u8];
    type Output = RawBytesMessage;
    type Error = std::convert::Infallible;

    fn deserialize(input: &[u8]) -> Result<RawBytesMessage, Self::Error> {
        Ok(RawBytesMessage(input.to_vec()))
    }
}

/// A service type that passes raw CDR bytes for both request and response.
pub struct RawBytesService;

impl ZService for RawBytesService {
    type Request = RawBytesMessage;
    type Response = RawBytesMessage;
}

// ---- Action support ----

/// A message type for actions that participates in serde CDR serialization.
///
/// Unlike `RawBytesMessage` (which uses a custom identity serdes), this type
/// implements serde so it can be embedded in action protocol messages like
/// `SendGoalRequest<A>` which are serialized via `CdrSerdes`.
///
/// Wire format: [4-byte CDR length][bytes] — the length prefix is transparent
/// to Python users who always work with standard CDR bytes (with header).
#[derive(Clone)]
pub struct DynActionMessage(pub Vec<u8>);

impl serde::Serialize for DynActionMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DynActionMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;
        impl<'de> serde::de::Visitor<'de> for BytesVisitor {
            type Value = DynActionMessage;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "byte array")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<DynActionMessage, E> {
                Ok(DynActionMessage(v.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(
                self,
                v: Vec<u8>,
            ) -> Result<DynActionMessage, E> {
                Ok(DynActionMessage(v))
            }
        }
        deserializer.deserialize_bytes(BytesVisitor)
    }
}

impl ZMessage for DynActionMessage {
    type Serdes = SerdeCdrSerdes<DynActionMessage>;

    fn estimated_serialized_size(&self) -> usize {
        4 + 4 + self.0.len() // CDR header + length prefix + payload
    }
}

/// A dynamic action type that uses raw CDR bytes for Goal, Result, and Feedback.
///
/// This enables Python-to-Python (and Python-to-Rust hiroz) action communication
/// without per-type Rust code. ROS 2 interop requires proper type hashes which
/// are only available with typed Rust action implementations.
pub struct RawBytesAction;

impl hiroz::action::ZAction for RawBytesAction {
    type Goal = DynActionMessage;
    type Result = DynActionMessage;
    type Feedback = DynActionMessage;

    fn name() -> &'static str {
        // The builder uses the passed action_name, not this value
        "hiroz_py/DynamicAction"
    }
    // Default type_info methods return zero-hash TypeInfo (fine for hiroz↔hiroz)
}
