//! Limits for runtime schemas and dynamically allocated CDR values.

use super::{DynamicMessage, DynamicValue, FieldType, MessageSchema};

pub(crate) const MAX_DYNAMIC_ALLOCATION_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_DYNAMIC_DEPTH: usize = 64;

/// Check the allocation implied by defaults before constructing them. Sequence
/// elements are checked too, even though a default sequence itself is empty.
pub(crate) fn validate_schema(schema: &MessageSchema) -> Result<(), String> {
    schema_size(schema, 0).map(|_| ())
}

fn checked_size(size: Option<usize>) -> Result<usize, String> {
    size.filter(|n| *n <= MAX_DYNAMIC_ALLOCATION_BYTES)
        .ok_or_else(|| "dynamic schema defaults exceed the 64 MiB allocation limit".to_owned())
}

fn schema_size(schema: &MessageSchema, depth: usize) -> Result<usize, String> {
    if depth > MAX_DYNAMIC_DEPTH {
        return Err("dynamic schema nesting exceeds 64 levels".to_owned());
    }
    let mut size = std::mem::size_of::<DynamicMessage>();
    for field in &schema.fields {
        size = checked_size(size.checked_add(field_size(&field.field_type, depth + 1)?))?;
    }
    Ok(size)
}

fn field_size(field: &FieldType, depth: usize) -> Result<usize, String> {
    if depth > MAX_DYNAMIC_DEPTH {
        return Err("dynamic schema nesting exceeds 64 levels".to_owned());
    }
    let storage = std::mem::size_of::<DynamicValue>();
    let extra = match field {
        FieldType::Message(schema) => schema_size(schema, depth + 1)?,
        FieldType::Array(inner, len) => {
            checked_size(field_size(inner, depth + 1)?.checked_mul(*len))?
        }
        FieldType::Sequence(inner) | FieldType::BoundedSequence(inner, _) => {
            field_size(inner, depth + 1)?;
            0
        }
        _ => 0,
    };
    checked_size(storage.checked_add(extra))
}
