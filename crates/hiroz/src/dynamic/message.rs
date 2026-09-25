//! Dynamic message container for ROS 2 messages.
//!
//! This module provides `DynamicMessage`, a runtime container for ROS 2
//! messages where the type is determined at runtime rather than compile time.

use std::sync::Arc;

use super::error::DynamicError;
use super::schema::{FieldType, MessageSchema};
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
        let parts: Vec<&str> = path.split('.').take(MAX_DYNAMIC_DEPTH + 2).collect();
        check_path_depth(parts.len())?;
        self.get_nested(&parts)
    }

    fn get_nested(&self, path: &[&str]) -> Result<DynamicValue, DynamicError> {
        if path.is_empty() {
            return Err(DynamicError::EmptyPath);
        }
        self.ensure_cardinality("$")?;

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
        self.ensure_cardinality("$")?;
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
        self.ensure_cardinality("$")?;
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
        let parts: Vec<&str> = path.split('.').take(MAX_DYNAMIC_DEPTH + 2).collect();
        check_path_depth(parts.len())?;
        let mut budget = ValidationBudget {
            remaining: MAX_DYNAMIC_VALUES,
        };
        self.set_nested(&parts, value, "", 0, &mut budget)
    }

    fn set_nested(
        &mut self,
        path: &[&str],
        value: DynamicValue,
        prefix: &str,
        depth: usize,
        budget: &mut ValidationBudget,
    ) -> Result<(), DynamicError> {
        if path.is_empty() {
            return Err(DynamicError::EmptyPath);
        }
        check_depth(depth)?;
        self.ensure_cardinality(if prefix.is_empty() { "$" } else { prefix })?;

        let field_name = path[0];
        let field_idx = self
            .schema
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .ok_or_else(|| DynamicError::FieldNotFound(field_name.to_string()))?;
        let full_path = join_path(prefix, field_name);

        if path.len() == 1 {
            validate_value_inner(
                &self.schema.fields[field_idx].field_type,
                &value,
                &full_path,
                depth,
                budget,
            )?;
            self.values[field_idx] = value;
            Ok(())
        } else {
            let expected_type = &self.schema.fields[field_idx].field_type;
            match (expected_type, &self.values[field_idx]) {
                (FieldType::Message(expected_schema), DynamicValue::Message(actual)) => {
                    if !schemas_match(expected_schema, actual.schema(), depth + 1, budget)? {
                        return type_mismatch(&full_path, expected_type, &self.values[field_idx]);
                    }
                }
                (FieldType::Message(_), _) => {
                    return type_mismatch(&full_path, expected_type, &self.values[field_idx]);
                }
                _ => return Err(DynamicError::NotAMessage(field_name.to_string())),
            }
            match &mut self.values[field_idx] {
                DynamicValue::Message(msg) => {
                    msg.set_nested(&path[1..], value, &full_path, depth + 1, budget)
                }
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
        self.ensure_cardinality("$")?;
        if index >= self.values.len() {
            return Err(DynamicError::IndexOutOfBounds(index));
        }
        let value = value.into_dynamic();
        validate_value(
            &self.schema.fields[index].field_type,
            &value,
            &self.schema.fields[index].name,
        )?;
        self.values[index] = value;
        Ok(())
    }

    fn ensure_cardinality(&self, path: &str) -> Result<(), DynamicError> {
        if self.schema.fields.len() != self.values.len() {
            return Err(DynamicError::WrongArrayLength {
                path: path.to_string(),
                expected: self.schema.fields.len(),
                actual: self.values.len(),
            });
        }
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
        let value = value.into_dynamic();
        validate_value(&self.schema.fields[idx].field_type, &value, name)?;
        self.values[idx] = Some(value);
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
        let value = value.into_dynamic();
        validate_value(
            &self.schema.fields[index].field_type,
            &value,
            &self.schema.fields[index].name,
        )?;
        self.values[index] = Some(value);
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

pub(crate) const MAX_DYNAMIC_DEPTH: usize = 128;
pub(crate) const MAX_DYNAMIC_VALUES: usize = 1_000_000;

pub(crate) fn validate_message(message: &DynamicMessage) -> Result<(), DynamicError> {
    let mut budget = ValidationBudget {
        remaining: MAX_DYNAMIC_VALUES,
    };
    validate_message_inner(message.schema(), message, "$", 0, &mut budget)
}

fn validate_value(
    field_type: &FieldType,
    value: &DynamicValue,
    path: &str,
) -> Result<(), DynamicError> {
    let mut budget = ValidationBudget {
        remaining: MAX_DYNAMIC_VALUES,
    };
    validate_value_inner(field_type, value, path, 0, &mut budget)
}

struct ValidationBudget {
    remaining: usize,
}

impl ValidationBudget {
    fn consume(&mut self) -> Result<(), DynamicError> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            DynamicError::ResourceLimitExceeded(format!(
                "message contains more than {MAX_DYNAMIC_VALUES} values"
            ))
        })?;
        Ok(())
    }
}

fn validate_message_inner(
    expected: &MessageSchema,
    message: &DynamicMessage,
    path: &str,
    depth: usize,
    budget: &mut ValidationBudget,
) -> Result<(), DynamicError> {
    check_depth(depth)?;
    if expected.fields.len() != message.values().len() {
        return Err(DynamicError::WrongArrayLength {
            path: path.to_string(),
            expected: expected.fields.len(),
            actual: message.values().len(),
        });
    }
    for (field, value) in expected.fields.iter().zip(message.values()) {
        let field_path = join_path(path, &field.name);
        validate_value_inner(&field.field_type, value, &field_path, depth, budget)?;
    }
    Ok(())
}

fn validate_value_inner(
    field_type: &FieldType,
    value: &DynamicValue,
    path: &str,
    depth: usize,
    budget: &mut ValidationBudget,
) -> Result<(), DynamicError> {
    check_depth(depth)?;
    budget.consume()?;
    match (field_type, value) {
        (FieldType::Bool, DynamicValue::Bool(_))
        | (FieldType::Int8, DynamicValue::Int8(_))
        | (FieldType::Int16, DynamicValue::Int16(_))
        | (FieldType::Int32, DynamicValue::Int32(_))
        | (FieldType::Int64, DynamicValue::Int64(_))
        | (FieldType::Uint8 | FieldType::Char | FieldType::Byte, DynamicValue::Uint8(_))
        | (FieldType::Uint16 | FieldType::WChar, DynamicValue::Uint16(_))
        | (FieldType::Uint32, DynamicValue::Uint32(_))
        | (FieldType::Uint64, DynamicValue::Uint64(_))
        | (FieldType::Float32, DynamicValue::Float32(_))
        | (FieldType::Float64, DynamicValue::Float64(_)) => {}
        (FieldType::String | FieldType::BoundedString(_), DynamicValue::String(text)) => {
            validate_narrow_string(text, path)?;
            if let FieldType::BoundedString(max) = field_type
                && text.len() > *max
            {
                return Err(DynamicError::FieldBoundExceeded {
                    path: path.to_string(),
                    max: *max,
                    actual: text.len(),
                });
            }
        }
        (FieldType::WString, DynamicValue::String(text)) => validate_wide_string(text, path)?,
        (FieldType::BoundedWString(max), DynamicValue::String(text)) => {
            validate_wide_string(text, path)?;
            let actual = text.encode_utf16().count();
            if actual > *max {
                return Err(DynamicError::FieldBoundExceeded {
                    path: path.to_string(),
                    max: *max,
                    actual,
                });
            }
        }
        (FieldType::Array(inner, expected), DynamicValue::Array(values)) => {
            if values.len() != *expected {
                return Err(DynamicError::WrongArrayLength {
                    path: path.to_string(),
                    expected: *expected,
                    actual: values.len(),
                });
            }
            validate_elements(inner, values, path, depth, budget)?;
        }
        (FieldType::Sequence(inner), DynamicValue::Array(values)) => {
            validate_sequence_length(values.len(), path)?;
            validate_elements(inner, values, path, depth, budget)?;
        }
        (FieldType::BoundedSequence(inner, max), DynamicValue::Array(values)) => {
            validate_sequence_length(values.len(), path)?;
            if values.len() > *max {
                return Err(DynamicError::FieldBoundExceeded {
                    path: path.to_string(),
                    max: *max,
                    actual: values.len(),
                });
            }
            validate_elements(inner, values, path, depth, budget)?;
        }
        (FieldType::Sequence(inner), DynamicValue::Bytes(bytes)) if is_byte_type(inner) => {
            validate_sequence_length(bytes.len(), path)?;
        }
        (FieldType::BoundedSequence(inner, max), DynamicValue::Bytes(bytes))
            if is_byte_type(inner) =>
        {
            validate_sequence_length(bytes.len(), path)?;
            if bytes.len() > *max {
                return Err(DynamicError::FieldBoundExceeded {
                    path: path.to_string(),
                    max: *max,
                    actual: bytes.len(),
                });
            }
        }
        (FieldType::Message(expected), DynamicValue::Message(message)) => {
            if !schemas_match(expected, message.schema(), depth + 1, budget)? {
                return type_mismatch(path, field_type, value);
            }
            validate_message_inner(expected, message, path, depth + 1, budget)?;
        }
        _ => return type_mismatch(path, field_type, value),
    }
    Ok(())
}

fn validate_elements(
    inner: &FieldType,
    values: &[DynamicValue],
    path: &str,
    depth: usize,
    budget: &mut ValidationBudget,
) -> Result<(), DynamicError> {
    for (index, value) in values.iter().enumerate() {
        validate_value_inner(inner, value, &format!("{path}[{index}]"), depth + 1, budget)?;
    }
    Ok(())
}

fn validate_narrow_string(value: &str, path: &str) -> Result<(), DynamicError> {
    if value.as_bytes().contains(&0) {
        return Err(DynamicError::InvalidString {
            path: path.to_string(),
            reason: "contains an embedded NUL byte".into(),
        });
    }
    let wire_len = value
        .len()
        .checked_add(1)
        .ok_or_else(|| DynamicError::InvalidString {
            path: path.to_string(),
            reason: "length overflows CDR string prefix".into(),
        })?;
    u32::try_from(wire_len).map_err(|_| DynamicError::InvalidString {
        path: path.to_string(),
        reason: "length exceeds CDR uint32 string prefix".into(),
    })?;
    Ok(())
}

fn validate_wide_string(value: &str, path: &str) -> Result<(), DynamicError> {
    let mut units = 0usize;
    for unit in value.encode_utf16() {
        if unit == 0 {
            return Err(DynamicError::InvalidString {
                path: path.to_string(),
                reason: "contains an embedded NUL word".into(),
            });
        }
        units = units
            .checked_add(1)
            .ok_or_else(|| DynamicError::InvalidString {
                path: path.to_string(),
                reason: "length overflows CDR wide-string prefix".into(),
            })?;
    }
    u32::try_from(units).map_err(|_| DynamicError::InvalidString {
        path: path.to_string(),
        reason: "length exceeds CDR uint32 wide-string prefix".into(),
    })?;
    Ok(())
}

fn validate_sequence_length(len: usize, path: &str) -> Result<(), DynamicError> {
    u32::try_from(len).map(|_| ()).map_err(|_| {
        DynamicError::SerializationError(format!(
            "sequence at '{path}' exceeds CDR uint32 length prefix"
        ))
    })
}

fn is_byte_type(field_type: &FieldType) -> bool {
    matches!(
        field_type,
        FieldType::Uint8 | FieldType::Char | FieldType::Byte
    )
}

fn type_mismatch<T>(
    path: &str,
    expected: &FieldType,
    _actual: &DynamicValue,
) -> Result<T, DynamicError> {
    Err(DynamicError::TypeMismatch {
        path: path.to_string(),
        expected: type_label(expected),
    })
}

fn type_label(field_type: &FieldType) -> String {
    match field_type {
        FieldType::Bool => "bool".into(),
        FieldType::Int8 => "int8".into(),
        FieldType::Int16 => "int16".into(),
        FieldType::Int32 => "int32".into(),
        FieldType::Int64 => "int64".into(),
        FieldType::Uint8 => "uint8".into(),
        FieldType::Char => "char".into(),
        FieldType::WChar => "wchar".into(),
        FieldType::Byte => "byte".into(),
        FieldType::Uint16 => "uint16".into(),
        FieldType::Uint32 => "uint32".into(),
        FieldType::Uint64 => "uint64".into(),
        FieldType::Float32 => "float32".into(),
        FieldType::Float64 => "float64".into(),
        FieldType::String => "string".into(),
        FieldType::BoundedString(_) => "bounded string".into(),
        FieldType::WString => "wstring".into(),
        FieldType::BoundedWString(_) => "bounded wstring".into(),
        FieldType::Message(schema) => format!("message {}", schema.type_name),
        FieldType::Array(_, _) => "fixed array".into(),
        FieldType::Sequence(_) => "sequence".into(),
        FieldType::BoundedSequence(_, _) => "bounded sequence".into(),
    }
}

fn check_depth(depth: usize) -> Result<(), DynamicError> {
    if depth > MAX_DYNAMIC_DEPTH {
        return Err(DynamicError::ResourceLimitExceeded(format!(
            "nesting exceeds {MAX_DYNAMIC_DEPTH} levels"
        )));
    }
    Ok(())
}

fn check_path_depth(components: usize) -> Result<(), DynamicError> {
    if components > MAX_DYNAMIC_DEPTH + 1 {
        return Err(DynamicError::ResourceLimitExceeded(format!(
            "field path exceeds {MAX_DYNAMIC_DEPTH} nested levels"
        )));
    }
    Ok(())
}

fn schemas_match(
    expected: &MessageSchema,
    actual: &MessageSchema,
    depth: usize,
    budget: &mut ValidationBudget,
) -> Result<bool, DynamicError> {
    check_depth(depth)?;
    budget.consume()?;
    if std::ptr::eq(expected, actual) {
        return Ok(true);
    }
    if expected.type_name != actual.type_name || expected.fields.len() != actual.fields.len() {
        return Ok(false);
    }
    for (expected, actual) in expected.fields.iter().zip(&actual.fields) {
        if expected.name != actual.name
            || !field_types_match(&expected.field_type, &actual.field_type, depth + 1, budget)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn field_types_match(
    expected: &FieldType,
    actual: &FieldType,
    depth: usize,
    budget: &mut ValidationBudget,
) -> Result<bool, DynamicError> {
    check_depth(depth)?;
    budget.consume()?;
    let matches = match (expected, actual) {
        (FieldType::Message(expected), FieldType::Message(actual)) => {
            schemas_match(expected, actual, depth + 1, budget)?
        }
        (FieldType::Array(expected, expected_len), FieldType::Array(actual, actual_len)) => {
            expected_len == actual_len && field_types_match(expected, actual, depth + 1, budget)?
        }
        (FieldType::Sequence(expected), FieldType::Sequence(actual)) => {
            field_types_match(expected, actual, depth + 1, budget)?
        }
        (
            FieldType::BoundedSequence(expected, expected_max),
            FieldType::BoundedSequence(actual, actual_max),
        ) => expected_max == actual_max && field_types_match(expected, actual, depth + 1, budget)?,
        _ => {
            std::mem::discriminant(expected) == std::mem::discriminant(actual)
                && match (expected, actual) {
                    (FieldType::BoundedString(a), FieldType::BoundedString(b))
                    | (FieldType::BoundedWString(a), FieldType::BoundedWString(b)) => a == b,
                    _ => true,
                }
        }
    };
    Ok(matches)
}

fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn shared_schema(depth: usize) -> Arc<MessageSchema> {
        let mut schema = MessageSchema::builder("test_msgs/msg/Shared0")
            .build()
            .unwrap();
        for level in 1..=depth {
            schema = MessageSchema::builder(&format!("test_msgs/msg/Shared{level}"))
                .field("left", FieldType::Message(schema.clone()))
                .field("right", FieldType::Message(schema))
                .build()
                .unwrap();
        }
        schema
    }

    #[test]
    fn structural_schema_comparison_has_a_work_budget() {
        let expected = shared_schema(24);
        let actual = shared_schema(24);
        let message = DynamicMessage::from_values(&actual, Vec::new());
        let mut budget = ValidationBudget { remaining: 64 };
        assert!(matches!(
            schemas_match(&expected, message.schema(), 0, &mut budget),
            Err(DynamicError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn mismatch_diagnostic_does_not_format_nested_schema() {
        let expected = shared_schema(MAX_DYNAMIC_DEPTH + 1);
        let error = validate_value(
            &FieldType::Message(expected),
            &DynamicValue::String("wrong".into()),
            "child",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DynamicError::TypeMismatch { path, expected }
                if path == "child" && expected.starts_with("message test_msgs/msg/Shared")
        ));
    }
}
