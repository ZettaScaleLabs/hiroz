//! ROS 2 Type Description Schema
//!
//! This crate provides the canonical schema types for ROS 2 type descriptions,
//! used by hiroz for:
//! - Type hash (RIHS01) computation
//! - Type description service wire format
//! - Dynamic message schema conversion
//! - Python binding type ID mapping
//!
//! The `TypeDescription` type matches the ROS 2 `type_description_interfaces` exactly,
//! making it the single source of truth for type information after parsing.

mod hash;
mod type_description;
mod type_id;

pub use hash::{calculate_hash, to_ros2_json};
pub use type_description::{
    FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg, to_hash_version,
};
pub use type_id::TypeId;

/// RIHS01 type hash (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeHash(pub [u8; 32]);

impl TypeHash {
    /// Convert type hash to RIHS01 string format
    pub fn to_rihs_string(&self) -> String {
        format!("RIHS01_{}", hex::encode(self.0))
    }

    /// Parse RIHS01 string format to type hash
    pub fn from_rihs_string(s: &str) -> Result<Self, String> {
        if !s.starts_with("RIHS01_") {
            return Err("Invalid RIHS01 format: must start with 'RIHS01_'".to_string());
        }

        let hex_part = &s[7..];
        let bytes = hex::decode(hex_part).map_err(|e| format!("Invalid hex encoding: {}", e))?;

        if bytes.len() != 32 {
            return Err(format!("Hash must be 32 bytes, got {}", bytes.len()));
        }

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(TypeHash(hash))
    }

    /// Create a zero (placeholder) type hash for Humble compatibility
    pub fn zero() -> Self {
        TypeHash([0u8; 32])
    }
}

impl std::fmt::Display for TypeHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_rihs_string())
    }
}

impl Default for TypeHash {
    fn default() -> Self {
        Self::zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_hash_roundtrip() {
        let hash = TypeHash([
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0x77, 0x88, 0x99,
        ]);

        let s = hash.to_rihs_string();
        assert!(s.starts_with("RIHS01_"));
        assert_eq!(s.len(), 7 + 64); // "RIHS01_" + 64 hex chars

        let decoded = TypeHash::from_rihs_string(&s).unwrap();
        assert_eq!(hash, decoded);
    }

    #[test]
    fn test_type_hash_invalid_prefix() {
        let result = TypeHash::from_rihs_string("INVALID_1234");
        assert!(result.is_err());
    }

    #[test]
    fn test_type_hash_invalid_length() {
        let result = TypeHash::from_rihs_string("RIHS01_1234");
        assert!(result.is_err());
    }
}
