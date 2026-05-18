//! Schema types for dynamic ROS 2 messages.
//!
//! This module provides runtime representations of ROS 2 message types,
//! including field types, field schemas, and complete message schemas.

use std::sync::Arc;

use super::error::DynamicError;
use super::value::DynamicValue;

/// ROS 2 field types for dynamic messages.
///
/// Maps to all primitive and compound types supported by ROS 2 IDL.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldType {
    // Primitives (matching ROS 2 IDL)
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float32,
    Float64,
    String,
    /// Bounded string: string<=N
    BoundedString(usize),

    // Compound
    /// Nested message type
    Message(Arc<MessageSchema>),

    // Collections
    /// Fixed-size array: `T[N]`
    Array(Box<FieldType>, usize),
    /// Unbounded sequence: sequence<T>
    Sequence(Box<FieldType>),
    /// Bounded sequence: sequence<T, N>
    BoundedSequence(Box<FieldType>, usize),
}

impl FieldType {
    /// CDR size in bytes (None for variable-size types).
    pub fn fixed_size(&self) -> Option<usize> {
        match self {
            FieldType::Bool | FieldType::Int8 | FieldType::Uint8 => Some(1),
            FieldType::Int16 | FieldType::Uint16 => Some(2),
            FieldType::Int32 | FieldType::Uint32 | FieldType::Float32 => Some(4),
            FieldType::Int64 | FieldType::Uint64 | FieldType::Float64 => Some(8),
            FieldType::Array(inner, len) => inner.fixed_size().map(|s| s * len),
            FieldType::Message(schema) => schema.fixed_cdr_size(),
            // String, Sequence types are variable
            _ => None,
        }
    }

    /// CDR alignment requirement in bytes.
    pub fn alignment(&self) -> usize {
        match self {
            FieldType::Bool | FieldType::Int8 | FieldType::Uint8 => 1,
            FieldType::Int16 | FieldType::Uint16 => 2,
            FieldType::Int32 | FieldType::Uint32 | FieldType::Float32 => 4,
            FieldType::Int64 | FieldType::Uint64 | FieldType::Float64 => 8,
            FieldType::String | FieldType::BoundedString(_) => 4, // length prefix
            FieldType::Array(inner, _) => inner.alignment(),
            FieldType::Sequence(_) | FieldType::BoundedSequence(_, _) => 4, // length prefix
            FieldType::Message(schema) => schema.alignment(),
        }
    }

    /// Check if this is a primitive type (not a message or collection).
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            FieldType::Bool
                | FieldType::Int8
                | FieldType::Int16
                | FieldType::Int32
                | FieldType::Int64
                | FieldType::Uint8
                | FieldType::Uint16
                | FieldType::Uint32
                | FieldType::Uint64
                | FieldType::Float32
                | FieldType::Float64
                | FieldType::String
                | FieldType::BoundedString(_)
        )
    }

    /// Check if this is a numeric type.
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            FieldType::Int8
                | FieldType::Int16
                | FieldType::Int32
                | FieldType::Int64
                | FieldType::Uint8
                | FieldType::Uint16
                | FieldType::Uint32
                | FieldType::Uint64
                | FieldType::Float32
                | FieldType::Float64
        )
    }

    /// Get the inner element type for arrays and sequences.
    pub fn element_type(&self) -> Option<&FieldType> {
        match self {
            FieldType::Array(inner, _)
            | FieldType::Sequence(inner)
            | FieldType::BoundedSequence(inner, _) => Some(inner),
            _ => None,
        }
    }
}

/// Schema for a single message field.
#[derive(Clone, Debug)]
pub struct FieldSchema {
    /// Field name
    pub name: String,
    /// Field type
    pub field_type: FieldType,
    /// Optional default value
    pub default_value: Option<DynamicValue>,
}

impl FieldSchema {
    /// Create a new field schema.
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
            default_value: None,
        }
    }

    /// Set the default value for this field.
    pub fn with_default(mut self, value: DynamicValue) -> Self {
        self.default_value = Some(value);
        self
    }
}

/// Complete schema for a ROS 2 message type.
#[derive(Clone, Debug)]
pub struct MessageSchema {
    /// Full type name: "geometry_msgs/msg/Twist"
    pub type_name: String,
    /// Package name: "geometry_msgs"
    pub package: String,
    /// Message name: "Twist"
    pub name: String,
    /// Ordered list of fields
    pub fields: Vec<FieldSchema>,
    /// RIHS01 type hash for compatibility checking
    pub type_hash: Option<String>,
}

impl MessageSchema {
    /// Get field by name.
    pub fn field(&self, name: &str) -> Option<&FieldSchema> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Get field index by name.
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    /// Get field path indices for dot notation (e.g., "linear.x").
    ///
    /// Returns a vector of field indices for navigating nested messages.
    pub fn field_path_indices(&self, path: &str) -> Result<Vec<usize>, DynamicError> {
        let mut indices = Vec::new();
        let mut current_schema = self;

        for part in path.split('.') {
            let idx = current_schema
                .field_index(part)
                .ok_or_else(|| DynamicError::FieldNotFound(part.to_string()))?;
            indices.push(idx);

            // Navigate to nested schema if needed
            if let FieldType::Message(nested) = &current_schema.fields[idx].field_type {
                current_schema = nested;
            }
        }
        Ok(indices)
    }

    /// Fixed CDR size if all fields are fixed-size.
    pub fn fixed_cdr_size(&self) -> Option<usize> {
        let mut size = 0usize;
        for field in &self.fields {
            let field_size = field.field_type.fixed_size()?;
            // Add padding for alignment
            let align = field.field_type.alignment();
            size = (size + align - 1) & !(align - 1);
            size += field_size;
        }
        Some(size)
    }

    /// Maximum alignment of any field.
    pub fn alignment(&self) -> usize {
        self.fields
            .iter()
            .map(|f| f.field_type.alignment())
            .max()
            .unwrap_or(1)
    }

    /// Create a builder for programmatic schema construction.
    pub fn builder(type_name: &str) -> MessageSchemaBuilder {
        MessageSchemaBuilder::new(type_name)
    }

    /// Number of fields in this message.
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Iterate over field names.
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|f| f.name.as_str())
    }
}

impl PartialEq for MessageSchema {
    fn eq(&self, other: &Self) -> bool {
        // Schemas are equal if they have the same type name
        self.type_name == other.type_name
    }
}

/// Builder for creating schemas programmatically.
pub struct MessageSchemaBuilder {
    type_name: String,
    fields: Vec<FieldSchema>,
    type_hash: Option<String>,
}

impl MessageSchemaBuilder {
    /// Create a new builder for the given type name.
    pub fn new(type_name: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
            fields: Vec::new(),
            type_hash: None,
        }
    }

    /// Add a field to the schema.
    pub fn field(mut self, name: &str, field_type: FieldType) -> Self {
        self.fields.push(FieldSchema::new(name, field_type));
        self
    }

    /// Add a field with a default value.
    pub fn field_with_default(
        mut self,
        name: &str,
        field_type: FieldType,
        default: DynamicValue,
    ) -> Self {
        self.fields
            .push(FieldSchema::new(name, field_type).with_default(default));
        self
    }

    /// Set the type hash.
    pub fn type_hash(mut self, hash: impl Into<String>) -> Self {
        self.type_hash = Some(hash.into());
        self
    }

    /// Build the message schema.
    pub fn build(self) -> Result<Arc<MessageSchema>, DynamicError> {
        let parts: Vec<&str> = self.type_name.split('/').collect();
        if parts.len() != 3 || parts[1] != "msg" {
            return Err(DynamicError::InvalidTypeName(self.type_name));
        }

        Ok(Arc::new(MessageSchema {
            type_name: self.type_name.clone(),
            package: parts[0].to_string(),
            name: parts[2].to_string(),
            fields: self.fields,
            type_hash: self.type_hash,
        }))
    }
}
