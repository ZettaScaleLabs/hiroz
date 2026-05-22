//! Dynamic message container for ROS 2 messages.
//!
//! This module provides `DynamicMessage`, a runtime container for ROS 2
//! messages where the type is determined at runtime rather than compile time.

use std::sync::Arc;

use super::error::DynamicError;
use super::schema::MessageSchema;
use super::value::{DynamicValue, FromDynamic, IntoDynamic, default_for_type};

/// A ROS 2 message with runtime-determined type.
///
/// `DynamicMessage` stores message data in a structured format (vector of values)
/// along with the message schema. It supports field access by name (including
/// dot notation for nested fields) and CDR serialization.
#[derive(Clone, Debug)]
pub struct DynamicMessage {
    schema: Arc<MessageSchema>,
    values: Vec<DynamicValue>,
}

impl DynamicMessage {
    /// Create a new message with default values.
    pub fn new(schema: &Arc<MessageSchema>) -> Self {
        let values = schema
            .fields
            .iter()
            .map(|f| {
                f.default_value
                    .clone()
                    .unwrap_or_else(|| default_for_type(&f.field_type))
            })
            .collect();

        Self {
            schema: Arc::clone(schema),
            values,
        }
    }

    /// Create a message from pre-computed values (used by deserialization).
    pub(crate) fn from_values(schema: &Arc<MessageSchema>, values: Vec<DynamicValue>) -> Self {
        Self {
            schema: Arc::clone(schema),
            values,
        }
    }

    /// Create a message builder for the given schema.
    pub fn builder(schema: &Arc<MessageSchema>) -> DynamicMessageBuilder {
        DynamicMessageBuilder::new(schema)
    }

    /// Get the message schema.
    pub fn schema(&self) -> &MessageSchema {
        &self.schema
    }

    /// Get the schema as an Arc (for sharing).
    pub fn schema_arc(&self) -> Arc<MessageSchema> {
        Arc::clone(&self.schema)
    }

    /// Get field value by name with type conversion.
    ///
    /// Supports dot notation for nested fields (e.g., "linear.x").
    pub fn get<T: FromDynamic>(&self, path: &str) -> Result<T, DynamicError> {
        let value = self.get_dynamic(path)?;
        T::from_dynamic(&value).ok_or(DynamicError::TypeMismatch {
            path: path.to_string(),
            expected: std::any::type_name::<T>().to_string(),
        })
    }

    /// Get field value as DynamicValue.
    ///
    /// Supports dot notation for nested fields (e.g., "linear.x").
    pub fn get_dynamic(&self, path: &str) -> Result<DynamicValue, DynamicError> {
        let parts: Vec<&str> = path.split('.').collect();
        self.get_nested(&parts)
    }

    fn get_nested(&self, path: &[&str]) -> Result<DynamicValue, DynamicError> {
        if path.is_empty() {
            return Err(DynamicError::EmptyPath);
        }

        let field_name = path[0];
        let field_idx = self
            .schema
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .ok_or_else(|| DynamicError::FieldNotFound(field_name.to_string()))?;

        let value = self.values[field_idx].clone();

        if path.len() == 1 {
            Ok(value)
        } else {
            // Recurse into nested message
            match value {
                DynamicValue::Message(msg) => msg.get_nested(&path[1..]),
                _ => Err(DynamicError::NotAMessage(field_name.to_string())),
            }
        }
    }

    /// Get field value by pre-computed index (faster than by-name access).
    pub fn get_by_index<T: FromDynamic>(&self, index: usize) -> Result<T, DynamicError> {
        let value = self
            .values
            .get(index)
            .ok_or(DynamicError::IndexOutOfBounds(index))?;
        T::from_dynamic(value).ok_or(DynamicError::TypeMismatch {
            path: format!("[{}]", index),
            expected: std::any::type_name::<T>().to_string(),
        })
    }

    /// Get field value as DynamicValue by pre-computed index.
    pub fn get_dynamic_by_index(&self, index: usize) -> Result<&DynamicValue, DynamicError> {
        self.values
            .get(index)
            .ok_or(DynamicError::IndexOutOfBounds(index))
    }

    /// Set field value by name.
    ///
    /// Supports dot notation for nested fields (e.g., "linear.x").
    pub fn set<T: IntoDynamic>(&mut self, path: &str, value: T) -> Result<(), DynamicError> {
        self.set_dynamic(path, value.into_dynamic())
    }

    /// Set field value as DynamicValue.
    ///
    /// Supports dot notation for nested fields (e.g., "linear.x").
    pub fn set_dynamic(&mut self, path: &str, value: DynamicValue) -> Result<(), DynamicError> {
        let parts: Vec<&str> = path.split('.').collect();
        self.set_nested(&parts, value)
    }

    fn set_nested(&mut self, path: &[&str], value: DynamicValue) -> Result<(), DynamicError> {
        if path.is_empty() {
            return Err(DynamicError::EmptyPath);
        }

        let field_name = path[0];
        let field_idx = self
            .schema
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .ok_or_else(|| DynamicError::FieldNotFound(field_name.to_string()))?;

        if path.len() == 1 {
            self.values[field_idx] = value;
            Ok(())
        } else {
            // Recurse into nested message
            match &mut self.values[field_idx] {
                DynamicValue::Message(msg) => msg.set_nested(&path[1..], value),
                _ => Err(DynamicError::NotAMessage(field_name.to_string())),
            }
        }
    }

    /// Set field value by pre-computed index (faster than by-name access).
    pub fn set_by_index<T: IntoDynamic>(
        &mut self,
        index: usize,
        value: T,
    ) -> Result<(), DynamicError> {
        if index >= self.values.len() {
            return Err(DynamicError::IndexOutOfBounds(index));
        }
        self.values[index] = value.into_dynamic();
        Ok(())
    }

    /// Get the internal values vector (for serialization).
    pub fn values(&self) -> &[DynamicValue] {
        &self.values
    }

    /// Get the internal values vector mutably.
    pub fn values_mut(&mut self) -> &mut Vec<DynamicValue> {
        &mut self.values
    }

    /// Iterate over all fields with their names and values.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &DynamicValue)> {
        self.schema
            .fields
            .iter()
            .zip(self.values.iter())
            .map(|(field, value)| (field.name.as_str(), value))
    }

    /// Get the number of fields.
    pub fn field_count(&self) -> usize {
        self.values.len()
    }
}

impl PartialEq for DynamicMessage {
    fn eq(&self, other: &Self) -> bool {
        // Messages are equal if schemas match and all values are equal
        Arc::ptr_eq(&self.schema, &other.schema) && self.values == other.values
    }
}

/// Builder for creating DynamicMessage with initial values.
pub struct DynamicMessageBuilder {
    schema: Arc<MessageSchema>,
    values: Vec<Option<DynamicValue>>,
}

impl DynamicMessageBuilder {
    /// Create a new builder for the given schema.
    pub fn new(schema: &Arc<MessageSchema>) -> Self {
        Self {
            schema: Arc::clone(schema),
            values: vec![None; schema.fields.len()],
        }
    }

    /// Set a field value by name.
    pub fn set<T: IntoDynamic>(mut self, name: &str, value: T) -> Result<Self, DynamicError> {
        let idx = self
            .schema
            .fields
            .iter()
            .position(|f| f.name == name)
            .ok_or_else(|| DynamicError::FieldNotFound(name.to_string()))?;
        self.values[idx] = Some(value.into_dynamic());
        Ok(self)
    }

    /// Set a field value by index.
    pub fn set_by_index<T: IntoDynamic>(
        mut self,
        index: usize,
        value: T,
    ) -> Result<Self, DynamicError> {
        if index >= self.values.len() {
            return Err(DynamicError::IndexOutOfBounds(index));
        }
        self.values[index] = Some(value.into_dynamic());
        Ok(self)
    }

    /// Build the message, using defaults for unset fields.
    pub fn build(self) -> DynamicMessage {
        let values = self
            .values
            .into_iter()
            .zip(self.schema.fields.iter())
            .map(|(v, f)| {
                v.unwrap_or_else(|| {
                    f.default_value
                        .clone()
                        .unwrap_or_else(|| default_for_type(&f.field_type))
                })
            })
            .collect();

        DynamicMessage {
            schema: self.schema,
            values,
        }
    }
}
