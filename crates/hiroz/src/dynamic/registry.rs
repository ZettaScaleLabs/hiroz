//! Schema registry for dynamic message types.
//!
//! Provides a global cache of message schemas with lazy initialization
//! and pre-registration of bundled schemas.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

#[cfg(feature = "dynamic-schema-loader")]
use super::error::DynamicError;
use super::schema::MessageSchema;
#[cfg(feature = "dynamic-schema-loader")]
use super::schema::{FieldSchema, FieldType};

/// Global registry of message schemas.
///
/// Provides fast O(1) lookup by type name and ensures schema sharing
/// via `Arc<MessageSchema>`. Can be pre-populated with bundled schemas.
pub struct SchemaRegistry {
    schemas: HashMap<String, Arc<MessageSchema>>,
}

impl SchemaRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Get the global registry (lazy initialized).
    pub fn global() -> &'static RwLock<SchemaRegistry> {
        static REGISTRY: OnceLock<RwLock<SchemaRegistry>> = OnceLock::new();
        REGISTRY.get_or_init(|| RwLock::new(SchemaRegistry::new()))
    }

    /// Get schema by full type name (e.g., "geometry_msgs/msg/Twist").
    pub fn get(&self, type_name: &str) -> Option<Arc<MessageSchema>> {
        self.schemas.get(type_name).cloned()
    }

    /// Register a schema and return the Arc for sharing.
    pub fn register(&mut self, schema: Arc<MessageSchema>) -> Arc<MessageSchema> {
        let type_name = schema.type_name.clone();
        self.schemas.insert(type_name, schema.clone());
        schema
    }

    /// Check if a type is registered.
    pub fn contains(&self, type_name: &str) -> bool {
        self.schemas.contains_key(type_name)
    }

    /// List all registered type names.
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(|s| s.as_str())
    }

    /// Number of registered schemas.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    /// Clear all registered schemas.
    pub fn clear(&mut self) {
        self.schemas.clear();
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Convenience functions for working with the global registry

/// Get a schema from the global registry (read-only, fast path).
pub fn get_schema(type_name: &str) -> Option<Arc<MessageSchema>> {
    SchemaRegistry::global().read().ok()?.get(type_name)
}

/// Register a schema in the global registry.
pub fn register_schema(schema: Arc<MessageSchema>) -> Arc<MessageSchema> {
    SchemaRegistry::global()
        .write()
        .expect("Registry lock poisoned")
        .register(schema)
}

/// Check if a schema is registered.
pub fn has_schema(type_name: &str) -> bool {
    SchemaRegistry::global()
        .read()
        .map(|r| r.contains(type_name))
        .unwrap_or(false)
}

/// Convert a hiroz-codegen ParsedMessage to a dynamic MessageSchema.
///
/// This function handles the conversion of field types from the codegen
/// representation to the dynamic schema representation.
#[cfg(feature = "dynamic-schema-loader")]
pub fn parsed_message_to_schema(
    msg: &hiroz_codegen::types::ParsedMessage,
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<Arc<MessageSchema>, DynamicError> {
    let fields: Result<Vec<FieldSchema>, DynamicError> = msg
        .fields
        .iter()
        .map(|f| {
            let field_type = convert_field_type(f, resolver)?;
            Ok(FieldSchema::new(&f.name, field_type))
        })
        .collect();

    Ok(Arc::new(MessageSchema {
        type_name: format!("{}/msg/{}", msg.package, msg.name),
        package: msg.package.clone(),
        name: msg.name.clone(),
        fields: fields?,
        type_hash: None,
    }))
}

#[cfg(feature = "dynamic-schema-loader")]
fn convert_field_type(
    field: &hiroz_codegen::types::Field,
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    use hiroz_codegen::types::ArrayType;

    let base_type = convert_base_type(
        &field.field_type.base_type,
        &field.field_type.package,
        resolver,
    )?;

    match &field.field_type.array {
        ArrayType::Single => Ok(base_type),
        ArrayType::Fixed(n) => Ok(FieldType::Array(Box::new(base_type), *n)),
        ArrayType::Bounded(n) => Ok(FieldType::BoundedSequence(Box::new(base_type), *n)),
        ArrayType::Unbounded => Ok(FieldType::Sequence(Box::new(base_type))),
    }
}

#[cfg(feature = "dynamic-schema-loader")]
fn convert_base_type(
    base_type: &str,
    package: &Option<String>,
    resolver: &impl Fn(&str, &str) -> Option<Arc<MessageSchema>>,
) -> Result<FieldType, DynamicError> {
    // Check if it's a primitive type
    match base_type {
        "bool" => return Ok(FieldType::Bool),
        "int8" | "byte" => return Ok(FieldType::Int8),
        "int16" => return Ok(FieldType::Int16),
        "int32" => return Ok(FieldType::Int32),
        "int64" => return Ok(FieldType::Int64),
        "uint8" | "char" => return Ok(FieldType::Uint8),
        "uint16" => return Ok(FieldType::Uint16),
        "uint32" => return Ok(FieldType::Uint32),
        "uint64" => return Ok(FieldType::Uint64),
        "float32" => return Ok(FieldType::Float32),
        "float64" => return Ok(FieldType::Float64),
        "string" => return Ok(FieldType::String),
        _ => {}
    }

    // Check for bounded string
    if let Some(rest) = base_type.strip_prefix("string<=")
        && let Ok(max_len) = rest.parse::<usize>()
    {
        return Ok(FieldType::BoundedString(max_len));
    }

    // It's a message type - resolve it
    let pkg = package
        .as_ref()
        .ok_or_else(|| DynamicError::InvalidTypeName(base_type.to_string()))?;
    let schema = resolver(pkg, base_type)
        .ok_or_else(|| DynamicError::SchemaNotFound(format!("{}/msg/{}", pkg, base_type)))?;

    Ok(FieldType::Message(schema))
}
