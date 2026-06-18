use std::fmt;

use serde::{Deserialize, Serialize};

/// 16-byte DDS Global Identifier (GID).
///
/// Serializes as a raw 16-byte array (CDR `octet[16]`), matching the
/// Iron/Jazzy wire format in `ros_discovery_info`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Gid([u8; 16]);

impl From<[u8; 16]> for Gid {
    fn from(b: [u8; 16]) -> Self {
        Self(b)
    }
}

impl std::ops::Deref for Gid {
    type Target = [u8; 16];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Gid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
