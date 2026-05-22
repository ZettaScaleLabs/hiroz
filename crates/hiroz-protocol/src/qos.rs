//! QoS profile encoding/decoding for liveliness tokens.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use core::fmt::Display;

/// QoS profile for ROS 2 entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct QosProfile {
    pub reliability: QosReliability,
    pub durability: QosDurability,
    pub history: QosHistory,
}

impl QosProfile {
    /// Encode QoS to string for liveliness token.
    /// Format matches rmw_zenoh_cpp: [reliability]:[durability]:[history],[depth]:[deadline]:[lifespan]:[liveliness]
    pub fn encode(&self) -> String {
        use alloc::format;
        let default_qos = Self::default();

        // Reliability - empty if default (RMW values: 1=Reliable, 2=BestEffort)
        let reliability = if self.reliability != default_qos.reliability {
            match self.reliability {
                QosReliability::Reliable => "1",
                QosReliability::BestEffort => "2",
            }
        } else {
            ""
        };

        // Durability - empty if default (RMW values: 1=TransientLocal, 2=Volatile)
        let durability = if self.durability != default_qos.durability {
            match self.durability {
                QosDurability::TransientLocal => "1",
                QosDurability::Volatile => "2",
            }
        } else {
            ""
        };

        // History format: <history_kind>,<depth>
        // Only include kind if non-default, always include depth
        let history = match self.history {
            QosHistory::KeepLast(depth) => {
                if self.history != default_qos.history {
                    format!("1,{}", depth)
                } else {
                    format!(",{}", depth)
                }
            }
            QosHistory::KeepAll => "2,".to_string(),
        };

        // Deadline, lifespan, liveliness - use defaults (empty/infinite)
        let deadline = ",";
        let lifespan = ",";
        let liveliness = ",,";

        format!(
            "{}:{}:{}:{}:{}:{}",
            reliability, durability, history, deadline, lifespan, liveliness
        )
    }

    /// Decode QoS from liveliness token string.
    pub fn decode(s: &str) -> Result<Self, QosDecodeError> {
        let fields: alloc::vec::Vec<&str> = s.split(':').collect();
        if fields.len() < 3 {
            return Err(QosDecodeError::InvalidFormat);
        }

        let default_qos = Self::default();

        // Parse reliability (RMW values: 1=Reliable, 2=BestEffort)
        let reliability = match fields[0] {
            "" => default_qos.reliability,
            "1" => QosReliability::Reliable,
            "2" => QosReliability::BestEffort,
            _ => return Err(QosDecodeError::InvalidReliability),
        };

        // Parse durability (RMW values: 1=TransientLocal, 2=Volatile)
        let durability = match fields[1] {
            "" => default_qos.durability,
            "1" => QosDurability::TransientLocal,
            "2" => QosDurability::Volatile,
            _ => return Err(QosDecodeError::InvalidDurability),
        };

        // Parse history: <kind>,<depth>
        let history_parts: alloc::vec::Vec<&str> = fields[2].split(',').collect();
        if history_parts.len() < 2 {
            return Err(QosDecodeError::InvalidHistory);
        }

        let history = match history_parts[0] {
            "" | "1" => {
                // KeepLast - parse depth
                let depth = history_parts[1]
                    .parse::<usize>()
                    .map_err(|_| QosDecodeError::InvalidHistory)?;
                QosHistory::KeepLast(depth)
            }
            "2" => QosHistory::KeepAll,
            _ => return Err(QosDecodeError::InvalidHistory),
        };

        Ok(QosProfile {
            reliability,
            durability,
            history,
        })
    }
}

/// QoS reliability policy.
///
/// ROS 2 default: Reliable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum QosReliability {
    BestEffort = 0,
    #[default]
    Reliable = 1,
}

/// QoS durability policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum QosDurability {
    #[default]
    Volatile = 0,
    TransientLocal = 1,
}

/// QoS history policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QosHistory {
    KeepLast(usize),
    KeepAll,
}

impl Default for QosHistory {
    fn default() -> Self {
        QosHistory::KeepLast(10)
    }
}

impl QosHistory {
    pub fn from_depth(depth: usize) -> Self {
        QosHistory::KeepLast(depth)
    }

    pub fn depth(&self) -> usize {
        match self {
            QosHistory::KeepLast(d) => *d,
            QosHistory::KeepAll => 0,
        }
    }
}

/// QoS decode errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosDecodeError {
    InvalidFormat,
    InvalidReliability,
    InvalidDurability,
    InvalidHistory,
}

impl Display for QosDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            QosDecodeError::InvalidFormat => write!(f, "Invalid QoS format"),
            QosDecodeError::InvalidReliability => write!(f, "Invalid reliability value"),
            QosDecodeError::InvalidDurability => write!(f, "Invalid durability value"),
            QosDecodeError::InvalidHistory => write!(f, "Invalid history value"),
        }
    }
}
