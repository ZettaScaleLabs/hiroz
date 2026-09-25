//! ROS 2 Type ID Constants
//!
//! Type IDs follow the ROS 2 RIHS01 specification:
//! - 1-48: Single-value type-ID range
//! - 49-96: Fixed arrays (base + 48)
//! - 97-144: Bounded sequences (base + 96)
//! - 145-192: Unbounded sequences (base + 144)
//!
//! Special type IDs:
//! - 1, 49, 97, 145: Nested message types

/// Type ID constants matching ROS 2 RIHS01 specification
pub struct TypeId;

impl TypeId {
    // ===== Single Values (1-48; defined primitives occupy selected IDs) =====

    /// Nested message type (single)
    pub const NESTED_TYPE: u8 = 1;
    /// int8 (single)
    pub const INT8: u8 = 2;
    /// uint8 (single)
    pub const UINT8: u8 = 3;
    /// int16 (single)
    pub const INT16: u8 = 4;
    /// uint16 (single)
    pub const UINT16: u8 = 5;
    /// int32 (single)
    pub const INT32: u8 = 6;
    /// uint32 (single)
    pub const UINT32: u8 = 7;
    /// int64 (single)
    pub const INT64: u8 = 8;
    /// uint64 (single)
    pub const UINT64: u8 = 9;
    /// float32 (single)
    pub const FLOAT32: u8 = 10;
    /// float64 (single)
    pub const FLOAT64: u8 = 11;
    /// native IDL char (single)
    pub const CHAR: u8 = 13;
    /// bool (single)
    pub const BOOL: u8 = 15;
    /// IDL byte (single)
    pub const BYTE: u8 = 16;
    /// string (single)
    pub const STRING: u8 = 17;
    /// wstring (single) - wide string
    pub const WSTRING: u8 = 18;
    /// fixed string (single)
    pub const FIXED_STRING: u8 = 19;
    /// fixed wstring (single)
    pub const FIXED_WSTRING: u8 = 20;
    /// bounded string (single) - string with max length
    pub const BOUNDED_STRING: u8 = 21;
    /// bounded wstring (single)
    pub const BOUNDED_WSTRING: u8 = 22;

    // ===== Fixed Arrays (49-96) = base + 48 =====

    /// Nested message type (fixed array)
    pub const NESTED_TYPE_ARRAY: u8 = 49;
    /// int8 (fixed array)
    pub const INT8_ARRAY: u8 = 50;
    /// uint8 (fixed array)
    pub const UINT8_ARRAY: u8 = 51;
    /// native IDL char (fixed array)
    pub const CHAR_ARRAY: u8 = Self::CHAR + Self::ARRAY_OFFSET;
    /// byte (fixed array)
    pub const BYTE_ARRAY: u8 = Self::BYTE + Self::ARRAY_OFFSET;
    /// int16 (fixed array)
    pub const INT16_ARRAY: u8 = 52;
    /// uint16 (fixed array)
    pub const UINT16_ARRAY: u8 = 53;
    /// int32 (fixed array)
    pub const INT32_ARRAY: u8 = 54;
    /// uint32 (fixed array)
    pub const UINT32_ARRAY: u8 = 55;
    /// int64 (fixed array)
    pub const INT64_ARRAY: u8 = 56;
    /// uint64 (fixed array)
    pub const UINT64_ARRAY: u8 = 57;
    /// float32 (fixed array)
    pub const FLOAT32_ARRAY: u8 = 58;
    /// float64 (fixed array)
    pub const FLOAT64_ARRAY: u8 = 59;
    /// bool (fixed array)
    pub const BOOL_ARRAY: u8 = 63;
    /// string (fixed array)
    pub const STRING_ARRAY: u8 = 65;
    /// bounded string (fixed array)
    pub const BOUNDED_STRING_ARRAY: u8 = Self::BOUNDED_STRING + Self::ARRAY_OFFSET;

    // ===== Bounded Sequences (97-144) = base + 96 =====

    /// Nested message type (bounded sequence)
    pub const NESTED_TYPE_BOUNDED_SEQUENCE: u8 = 97;
    /// int8 (bounded sequence)
    pub const INT8_BOUNDED_SEQUENCE: u8 = 98;
    /// uint8 (bounded sequence)
    pub const UINT8_BOUNDED_SEQUENCE: u8 = 99;
    /// native IDL char (bounded sequence)
    pub const CHAR_BOUNDED_SEQUENCE: u8 = Self::CHAR + Self::BOUNDED_SEQUENCE_OFFSET;
    /// byte (bounded sequence)
    pub const BYTE_BOUNDED_SEQUENCE: u8 = Self::BYTE + Self::BOUNDED_SEQUENCE_OFFSET;
    /// int16 (bounded sequence)
    pub const INT16_BOUNDED_SEQUENCE: u8 = 100;
    /// uint16 (bounded sequence)
    pub const UINT16_BOUNDED_SEQUENCE: u8 = 101;
    /// int32 (bounded sequence)
    pub const INT32_BOUNDED_SEQUENCE: u8 = 102;
    /// uint32 (bounded sequence)
    pub const UINT32_BOUNDED_SEQUENCE: u8 = 103;
    /// int64 (bounded sequence)
    pub const INT64_BOUNDED_SEQUENCE: u8 = 104;
    /// uint64 (bounded sequence)
    pub const UINT64_BOUNDED_SEQUENCE: u8 = 105;
    /// float32 (bounded sequence)
    pub const FLOAT32_BOUNDED_SEQUENCE: u8 = 106;
    /// float64 (bounded sequence)
    pub const FLOAT64_BOUNDED_SEQUENCE: u8 = 107;
    /// bool (bounded sequence)
    pub const BOOL_BOUNDED_SEQUENCE: u8 = 111;
    /// string (bounded sequence)
    pub const STRING_BOUNDED_SEQUENCE: u8 = 113;
    /// bounded string (bounded sequence)
    pub const BOUNDED_STRING_BOUNDED_SEQUENCE: u8 =
        Self::BOUNDED_STRING + Self::BOUNDED_SEQUENCE_OFFSET;

    // ===== Unbounded Sequences (145-192) = base + 144 =====

    /// Nested message type (unbounded sequence)
    pub const NESTED_TYPE_UNBOUNDED_SEQUENCE: u8 = 145;
    /// int8 (unbounded sequence)
    pub const INT8_UNBOUNDED_SEQUENCE: u8 = 146;
    /// uint8 (unbounded sequence)
    pub const UINT8_UNBOUNDED_SEQUENCE: u8 = 147;
    /// native IDL char (unbounded sequence)
    pub const CHAR_UNBOUNDED_SEQUENCE: u8 = Self::CHAR + Self::UNBOUNDED_SEQUENCE_OFFSET;
    /// byte (unbounded sequence)
    pub const BYTE_UNBOUNDED_SEQUENCE: u8 = Self::BYTE + Self::UNBOUNDED_SEQUENCE_OFFSET;
    /// int16 (unbounded sequence)
    pub const INT16_UNBOUNDED_SEQUENCE: u8 = 148;
    /// uint16 (unbounded sequence)
    pub const UINT16_UNBOUNDED_SEQUENCE: u8 = 149;
    /// int32 (unbounded sequence)
    pub const INT32_UNBOUNDED_SEQUENCE: u8 = 150;
    /// uint32 (unbounded sequence)
    pub const UINT32_UNBOUNDED_SEQUENCE: u8 = 151;
    /// int64 (unbounded sequence)
    pub const INT64_UNBOUNDED_SEQUENCE: u8 = 152;
    /// uint64 (unbounded sequence)
    pub const UINT64_UNBOUNDED_SEQUENCE: u8 = 153;
    /// float32 (unbounded sequence)
    pub const FLOAT32_UNBOUNDED_SEQUENCE: u8 = 154;
    /// float64 (unbounded sequence)
    pub const FLOAT64_UNBOUNDED_SEQUENCE: u8 = 155;
    /// bool (unbounded sequence)
    pub const BOOL_UNBOUNDED_SEQUENCE: u8 = 159;
    /// string (unbounded sequence)
    pub const STRING_UNBOUNDED_SEQUENCE: u8 = 161;
    /// bounded string (unbounded sequence)
    pub const BOUNDED_STRING_UNBOUNDED_SEQUENCE: u8 =
        Self::BOUNDED_STRING + Self::UNBOUNDED_SEQUENCE_OFFSET;

    // ===== Offset Constants =====

    /// Offset to convert single type to fixed array
    pub const ARRAY_OFFSET: u8 = 48;
    /// Offset to convert single type to bounded sequence
    pub const BOUNDED_SEQUENCE_OFFSET: u8 = 96;
    /// Offset to convert single type to unbounded sequence
    pub const UNBOUNDED_SEQUENCE_OFFSET: u8 = 144;

    /// Check if a type ID is a nested (message) type
    pub const fn is_nested(type_id: u8) -> bool {
        type_id == Self::NESTED_TYPE
            || type_id == Self::NESTED_TYPE_ARRAY
            || type_id == Self::NESTED_TYPE_BOUNDED_SEQUENCE
            || type_id == Self::NESTED_TYPE_UNBOUNDED_SEQUENCE
    }

    /// Check if a type ID is an array (fixed size)
    pub const fn is_array(type_id: u8) -> bool {
        type_id > Self::ARRAY_OFFSET && type_id <= Self::BOUNDED_SEQUENCE_OFFSET
    }

    /// Check if a type ID is a bounded sequence
    pub const fn is_bounded_sequence(type_id: u8) -> bool {
        type_id > Self::BOUNDED_SEQUENCE_OFFSET && type_id <= Self::UNBOUNDED_SEQUENCE_OFFSET
    }

    /// Check if a type ID is an unbounded sequence
    pub const fn is_unbounded_sequence(type_id: u8) -> bool {
        type_id > Self::UNBOUNDED_SEQUENCE_OFFSET
            && type_id <= Self::UNBOUNDED_SEQUENCE_OFFSET + Self::ARRAY_OFFSET
    }

    /// Check if a type ID is a single (non-array, non-sequence) value
    pub const fn is_single(type_id: u8) -> bool {
        type_id >= 1 && type_id <= Self::ARRAY_OFFSET
    }

    /// Get the base type ID (strip array/sequence modifier)
    pub const fn base_type(type_id: u8) -> u8 {
        if Self::is_unbounded_sequence(type_id) {
            type_id - Self::UNBOUNDED_SEQUENCE_OFFSET
        } else if Self::is_bounded_sequence(type_id) {
            type_id - Self::BOUNDED_SEQUENCE_OFFSET
        } else if Self::is_array(type_id) {
            type_id - Self::ARRAY_OFFSET
        } else {
            type_id
        }
    }

    /// Get the type name for a base type ID
    pub const fn type_name(type_id: u8) -> Option<&'static str> {
        match Self::base_type(type_id) {
            Self::NESTED_TYPE => Some("nested"),
            Self::INT8 => Some("int8"),
            Self::UINT8 => Some("uint8"),
            Self::CHAR => Some("char"),
            Self::INT16 => Some("int16"),
            Self::UINT16 => Some("uint16"),
            Self::INT32 => Some("int32"),
            Self::UINT32 => Some("uint32"),
            Self::INT64 => Some("int64"),
            Self::UINT64 => Some("uint64"),
            Self::FLOAT32 => Some("float32"),
            Self::FLOAT64 => Some("float64"),
            Self::BOOL => Some("bool"),
            Self::STRING => Some("string"),
            Self::BOUNDED_STRING => Some("bounded_string"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_id_offsets() {
        assert_eq!(TypeId::INT32_ARRAY, TypeId::INT32 + TypeId::ARRAY_OFFSET);
        assert_eq!(
            TypeId::INT32_BOUNDED_SEQUENCE,
            TypeId::INT32 + TypeId::BOUNDED_SEQUENCE_OFFSET
        );
        assert_eq!(
            TypeId::INT32_UNBOUNDED_SEQUENCE,
            TypeId::INT32 + TypeId::UNBOUNDED_SEQUENCE_OFFSET
        );
    }

    #[test]
    fn test_is_nested() {
        assert!(TypeId::is_nested(TypeId::NESTED_TYPE));
        assert!(TypeId::is_nested(TypeId::NESTED_TYPE_ARRAY));
        assert!(TypeId::is_nested(TypeId::NESTED_TYPE_BOUNDED_SEQUENCE));
        assert!(TypeId::is_nested(TypeId::NESTED_TYPE_UNBOUNDED_SEQUENCE));
        assert!(!TypeId::is_nested(TypeId::INT32));
    }

    #[test]
    fn test_is_array() {
        assert!(TypeId::is_array(TypeId::INT32_ARRAY));
        assert!(TypeId::is_array(TypeId::STRING_ARRAY));
        assert!(!TypeId::is_array(TypeId::INT32));
        assert!(!TypeId::is_array(TypeId::INT32_UNBOUNDED_SEQUENCE));
    }

    #[test]
    fn test_base_type() {
        assert_eq!(TypeId::base_type(TypeId::INT32), TypeId::INT32);
        assert_eq!(TypeId::base_type(TypeId::INT32_ARRAY), TypeId::INT32);
        assert_eq!(
            TypeId::base_type(TypeId::INT32_BOUNDED_SEQUENCE),
            TypeId::INT32
        );
        assert_eq!(
            TypeId::base_type(TypeId::INT32_UNBOUNDED_SEQUENCE),
            TypeId::INT32
        );
        assert!(TypeId::is_single(TypeId::BOUNDED_STRING));
        assert!(TypeId::is_array(TypeId::BOUNDED_STRING_ARRAY));
        assert!(TypeId::is_bounded_sequence(
            TypeId::BOUNDED_STRING_BOUNDED_SEQUENCE
        ));
        assert!(TypeId::is_unbounded_sequence(
            TypeId::BOUNDED_STRING_UNBOUNDED_SEQUENCE
        ));
        assert_eq!(TypeId::base_type(TypeId::CHAR_ARRAY), TypeId::CHAR);
        assert!(TypeId::is_single(48));
        assert!(TypeId::is_array(49));
        assert!(TypeId::is_array(96));
        assert!(TypeId::is_bounded_sequence(97));
        assert!(TypeId::is_bounded_sequence(144));
        assert!(TypeId::is_unbounded_sequence(145));
        assert!(TypeId::is_unbounded_sequence(192));
        assert!(!TypeId::is_single(0));
        assert!(!TypeId::is_array(48));
        assert!(!TypeId::is_bounded_sequence(96));
        assert!(!TypeId::is_unbounded_sequence(193));
        assert!(!TypeId::is_unbounded_sequence(255));
    }

    #[test]
    fn test_type_name() {
        assert_eq!(TypeId::type_name(TypeId::INT32), Some("int32"));
        assert_eq!(TypeId::type_name(TypeId::INT32_ARRAY), Some("int32"));
        assert_eq!(TypeId::type_name(TypeId::STRING), Some("string"));
        assert_eq!(TypeId::type_name(TypeId::NESTED_TYPE), Some("nested"));
    }
}
