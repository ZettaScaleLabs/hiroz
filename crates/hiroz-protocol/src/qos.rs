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
            "" | "0" => default_qos.reliability,
            "1" => QosReliability::Reliable,
            "2" => QosReliability::BestEffort,
            _ => return Err(QosDecodeError::InvalidReliability),
        };

        // Parse durability (RMW values: 1=TransientLocal, 2=Volatile)
        let durability = match fields[1] {
            "" | "0" => default_qos.durability,
            "1" => QosDurability::TransientLocal,
            "2" => QosDurability::Volatile,
            _ => return Err(QosDecodeError::InvalidDurability),
        };

        // Parse history: <kind>,<depth>. rmw_zenoh_cpp omits QoS sub-fields
        // whose value is SYSTEM_DEFAULT, so the history field can be just `,`.
        let history = match fields[2] {
            "," => default_qos.history,
            // An omitted history field is only meaningful in the complete
            // six-field wire representation. Keep rejecting truncated `::`.
            "" if fields.len() >= 6 => default_qos.history,
            encoded => {
                let (kind, encoded_depth) = encoded
                    .split_once(',')
                    .ok_or(QosDecodeError::InvalidHistory)?;

                match kind {
                    "" | "0" | "1" => {
                        let depth = if encoded_depth.is_empty() {
                            default_qos.history.depth()
                        } else {
                            encoded_depth
                                .parse::<usize>()
                                .map_err(|_| QosDecodeError::InvalidHistory)?
                        };
                        // A zero depth represents an unspecified/default depth
                        // at the ROS boundary; KeepLast(0) is not useful.
                        QosHistory::KeepLast(if depth == 0 {
                            default_qos.history.depth()
                        } else {
                            depth
                        })
                    }
                    "2" => QosHistory::KeepAll,
                    _ => return Err(QosDecodeError::InvalidHistory),
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rmw_compact_qos_corpus() {
        let cases = [
            ("::,:,:,:,,", QosProfile::default()),
            (":::,:,:,,", QosProfile::default()),
            ("::1,:,:,:,,", QosProfile::default()),
            ("::,10:,:,:,,", QosProfile::default()),
            (
                "::2,:,:,:,,",
                QosProfile {
                    history: QosHistory::KeepAll,
                    ..QosProfile::default()
                },
            ),
            (
                "1:1:,5:,:,:,,",
                QosProfile {
                    durability: QosDurability::TransientLocal,
                    history: QosHistory::KeepLast(5),
                    ..QosProfile::default()
                },
            ),
            (
                "2::,1:,:,:,,",
                QosProfile {
                    reliability: QosReliability::BestEffort,
                    history: QosHistory::KeepLast(1),
                    ..QosProfile::default()
                },
            ),
            ("0:0:0,0:,:,:,,", QosProfile::default()),
        ];

        for (encoded, expected) in cases {
            assert_eq!(QosProfile::decode(encoded), Ok(expected), "{encoded}");
        }
    }

    #[test]
    fn qos_round_trip() {
        let profiles = [
            QosProfile::default(),
            QosProfile {
                reliability: QosReliability::BestEffort,
                durability: QosDurability::TransientLocal,
                history: QosHistory::KeepLast(5),
            },
            QosProfile {
                history: QosHistory::KeepAll,
                ..QosProfile::default()
            },
        ];

        for profile in profiles {
            assert_eq!(QosProfile::decode(&profile.encode()), Ok(profile));
        }
    }

    #[test]
    fn reject_invalid_history() {
        for encoded in ["::3,1:,:,:,,", "::1,-1:,:,:,,", "::"] {
            assert_eq!(
                QosProfile::decode(encoded),
                Err(QosDecodeError::InvalidHistory),
                "{encoded}"
            );
        }
    }
}
