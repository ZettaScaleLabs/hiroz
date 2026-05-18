//! Error types for dynamic message handling.

use std::fmt;

/// Errors that can occur during dynamic message operations.
#[derive(Debug)]
pub enum DynamicError {
    /// Invalid type name format (should be "package/msg/Name")
    InvalidTypeName(String),

    /// Field not found in message schema
    FieldNotFound(String),

    /// Empty path provided for field access
    EmptyPath,

    /// Attempted to access a nested field on a non-message type
    NotAMessage(String),

    /// Type mismatch during field access or conversion
    TypeMismatch { path: String, expected: String },

    /// Index out of bounds for field access
    IndexOutOfBounds(usize),

    /// CDR serialization error
    SerializationError(String),

    /// CDR deserialization error
    DeserializationError(String),

    /// Schema loading error
    SchemaLoadError { package: String, source: String },

    /// Registry lock was poisoned
    RegistryLockPoisoned,

    /// Schema not found in registry
    SchemaNotFound(String),

    /// Type description service call timed out — no response from the remote node.
    ServiceTimeout { node: String, service: String },

    /// Automatic topic-based schema discovery requires publisher node identity.
    MissingNodeIdentity { topic: String },

    /// Invalid default value for field type
    InvalidDefaultValue { field: String, reason: String },

    /// Bounded string/sequence exceeded maximum size
    BoundExceeded { max: usize, actual: usize },
}

impl fmt::Display for DynamicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DynamicError::InvalidTypeName(name) => {
                write!(
                    f,
                    "Invalid type name '{}': expected format 'package/msg/Name'",
                    name
                )
            }
            DynamicError::FieldNotFound(name) => {
                write!(f, "Field '{}' not found in message schema", name)
            }
            DynamicError::EmptyPath => {
                write!(f, "Empty path provided for field access")
            }
            DynamicError::NotAMessage(name) => {
                write!(
                    f,
                    "Field '{}' is not a message type, cannot access nested fields",
                    name
                )
            }
            DynamicError::TypeMismatch { path, expected } => {
                write!(
                    f,
                    "Type mismatch at '{}': expected type '{}'",
                    path, expected
                )
            }
            DynamicError::IndexOutOfBounds(idx) => {
                write!(f, "Field index {} is out of bounds", idx)
            }
            DynamicError::SerializationError(msg) => {
                write!(f, "CDR serialization error: {}", msg)
            }
            DynamicError::DeserializationError(msg) => {
                write!(f, "CDR deserialization error: {}", msg)
            }
            DynamicError::SchemaLoadError { package, source } => {
                write!(
                    f,
                    "Failed to load schema for package '{}': {}",
                    package, source
                )
            }
            DynamicError::RegistryLockPoisoned => {
                write!(f, "Schema registry lock was poisoned")
            }
            DynamicError::SchemaNotFound(name) => {
                write!(f, "Schema '{}' not found in registry", name)
            }
            DynamicError::ServiceTimeout { node, service } => {
                write!(
                    f,
                    "type description service timed out: no response from node '{}' on service '{}'",
                    node, service
                )
            }
            DynamicError::MissingNodeIdentity { topic } => {
                write!(
                    f,
                    "automatic schema discovery for topic '{}' requires publisher node identity, which is unavailable from this backend/discovery format",
                    topic
                )
            }
            DynamicError::InvalidDefaultValue { field, reason } => {
                write!(f, "Invalid default value for field '{}': {}", field, reason)
            }
            DynamicError::BoundExceeded { max, actual } => {
                write!(
                    f,
                    "Bounded type exceeded maximum size: max={}, actual={}",
                    max, actual
                )
            }
        }
    }
}

impl std::error::Error for DynamicError {}

impl From<hiroz_cdr::Error> for DynamicError {
    fn from(e: hiroz_cdr::Error) -> Self {
        DynamicError::DeserializationError(e.to_string())
    }
}
