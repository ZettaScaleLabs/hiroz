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
    pub deadline: QosDuration,
    pub lifespan: QosDuration,
    pub liveliness: QosLiveliness,
    pub liveliness_lease_duration: QosDuration,
}

/// Encode one duration component (seconds or nanoseconds), omitted if it
/// matches the corresponding default component -- rmw_zenoh_cpp compares and
/// omits sec/nsec independently, not the duration as a whole, so a duration
/// can have an empty sec alongside a present nsec on the wire.
fn encode_duration_component(value: u64, default_value: u64) -> String {
    use alloc::format;
    if value != default_value {
        format!("{value}")
    } else {
        String::new()
    }
}

fn parse_duration_component(s: &str, default_value: u64) -> Result<u64, QosDecodeError> {
    if s.is_empty() {
        Ok(default_value)
    } else {
        s.parse::<u64>().map_err(|_| QosDecodeError::InvalidDuration)
    }
}

/// Parse a `<sec>,<nsec>` duration sub-field, empty either side falling back
/// to the matching component of `default`.
fn parse_duration(s: &str, default: &QosDuration) -> Result<QosDuration, QosDecodeError> {
    let (sec, nsec) = s.split_once(',').ok_or(QosDecodeError::InvalidDuration)?;
    Ok(QosDuration {
        sec: parse_duration_component(sec, default.sec)?,
        nsec: parse_duration_component(nsec, default.nsec)?,
    })
}

impl QosProfile {
    /// Encode QoS to string for liveliness token.
    /// Format matches rmw_zenoh_cpp: [reliability]:[durability]:[history],[depth]:[deadline_sec],[deadline_nsec]:[lifespan_sec],[lifespan_nsec]:[liveliness_kind],[lease_sec],[lease_nsec]
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

        let deadline = format!(
            "{},{}",
            encode_duration_component(self.deadline.sec, default_qos.deadline.sec),
            encode_duration_component(self.deadline.nsec, default_qos.deadline.nsec),
        );
        let lifespan = format!(
            "{},{}",
            encode_duration_component(self.lifespan.sec, default_qos.lifespan.sec),
            encode_duration_component(self.lifespan.nsec, default_qos.lifespan.nsec),
        );

        // Liveliness kind - empty if default (RMW values: 1=Automatic,
        // 2=ManualByNode, 3=ManualByTopic). rmw_zenoh_cpp itself never
        // encodes or decodes 2 (MANUAL_BY_NODE was deprecated and removed
        // from RMW); hiroz still carries the variant and encodes it as 2 for
        // completeness, but a peer running rmw_zenoh_cpp will not decode it
        // distinctly -- a pre-existing upstream limitation, not one this
        // encoder introduces.
        let liveliness_kind = if self.liveliness != default_qos.liveliness {
            match self.liveliness {
                QosLiveliness::Automatic => "1",
                QosLiveliness::ManualByNode => "2",
                QosLiveliness::ManualByTopic => "3",
            }
        } else {
            ""
        };
        let liveliness = format!(
            "{},{},{}",
            liveliness_kind,
            encode_duration_component(
                self.liveliness_lease_duration.sec,
                default_qos.liveliness_lease_duration.sec
            ),
            encode_duration_component(
                self.liveliness_lease_duration.nsec,
                default_qos.liveliness_lease_duration.nsec
            ),
        );

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
        // whose value is SYSTEM_DEFAULT, so the history field can be just
        // `,`. An omitted depth means the *peer* used rmw_zenoh_cpp's own
        // wire default (42), not hiroz's unrelated built-in default (10) --
        // substituting the latter here would misreport every such peer's
        // depth.
        let wire_default_history = QosHistory::KeepLast(RMW_ZENOH_DEFAULT_HISTORY_DEPTH);
        let history = match fields[2] {
            "," => wire_default_history,
            // An omitted history field is only meaningful in the complete
            // six-field wire representation. Keep rejecting truncated `::`.
            "" if fields.len() >= 6 => wire_default_history,
            encoded => {
                let (kind, encoded_depth) = encoded
                    .split_once(',')
                    .ok_or(QosDecodeError::InvalidHistory)?;

                match kind {
                    "" | "0" | "1" => {
                        let depth = if encoded_depth.is_empty() {
                            RMW_ZENOH_DEFAULT_HISTORY_DEPTH
                        } else {
                            encoded_depth
                                .parse::<usize>()
                                .map_err(|_| QosDecodeError::InvalidHistory)?
                        };
                        // A zero depth represents an unspecified/default depth
                        // at the ROS boundary; KeepLast(0) is not useful.
                        QosHistory::KeepLast(if depth == 0 {
                            RMW_ZENOH_DEFAULT_HISTORY_DEPTH
                        } else {
                            depth
                        })
                    }
                    "2" => QosHistory::KeepAll,
                    _ => return Err(QosDecodeError::InvalidHistory),
                }
            }
        };

        // Deadline/lifespan/liveliness are only present in the full
        // six-field wire representation; a shorter (e.g. three-field, pre-D5)
        // string decodes them as unset/default, same as an empty sub-field
        // within a present field does.
        let deadline = match fields.get(3) {
            Some(s) if !s.is_empty() => parse_duration(s, &default_qos.deadline)?,
            _ => default_qos.deadline,
        };
        let lifespan = match fields.get(4) {
            Some(s) if !s.is_empty() => parse_duration(s, &default_qos.lifespan)?,
            _ => default_qos.lifespan,
        };
        let (liveliness, liveliness_lease_duration) = match fields.get(5) {
            Some(s) if !s.is_empty() => {
                let mut parts = s.splitn(3, ',');
                let kind_s = parts.next().unwrap_or("");
                let lease_sec_s = parts.next().ok_or(QosDecodeError::InvalidLiveliness)?;
                let lease_nsec_s = parts.next().ok_or(QosDecodeError::InvalidLiveliness)?;
                let kind = match kind_s {
                    "" | "0" => default_qos.liveliness,
                    "1" => QosLiveliness::Automatic,
                    "2" => QosLiveliness::ManualByNode,
                    "3" => QosLiveliness::ManualByTopic,
                    _ => return Err(QosDecodeError::InvalidLiveliness),
                };
                let lease = QosDuration {
                    sec: parse_duration_component(
                        lease_sec_s,
                        default_qos.liveliness_lease_duration.sec,
                    )?,
                    nsec: parse_duration_component(
                        lease_nsec_s,
                        default_qos.liveliness_lease_duration.nsec,
                    )?,
                };
                (kind, lease)
            }
            _ => (default_qos.liveliness, default_qos.liveliness_lease_duration),
        };

        Ok(QosProfile {
            reliability,
            durability,
            history,
            deadline,
            lifespan,
            liveliness,
            liveliness_lease_duration,
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

/// The history depth `rmw_zenoh_cpp` substitutes on the wire when a QoS
/// profile's depth is SYSTEM_DEFAULT (0), and the value it omits from a
/// compact liveliness token's history field for the same reason.
/// `rmw_zenoh_cpp/src/detail/qos.cpp`: `RMW_ZENOH_DEFAULT_HISTORY_DEPTH`.
///
/// Distinct from [`QosHistory::default`]'s depth, which is hiroz's own
/// unrelated fallback (matching `rclcpp`'s default of 10) for a `QosProfile`
/// built without a history depth in code -- conflating the two silently
/// misreports the depth of any peer that relied on `rmw_zenoh_cpp`'s
/// SYSTEM_DEFAULT omission.
pub const RMW_ZENOH_DEFAULT_HISTORY_DEPTH: usize = 42;

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

/// A QoS duration in seconds + nanoseconds, matching ROS 2's
/// `RMW_DURATION_INFINITE` sentinel used for deadline, lifespan, and
/// liveliness lease duration when unset. Distinct from any host `Duration`
/// type so this crate stays `no_std`-clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QosDuration {
    pub sec: u64,
    pub nsec: u64,
}

impl QosDuration {
    /// ROS 2's `RMW_DURATION_INFINITE` sentinel: sec=9223372036, nsec=854775807.
    pub const INFINITE: QosDuration = QosDuration {
        sec: 9_223_372_036,
        nsec: 854_775_807,
    };
}

impl Default for QosDuration {
    fn default() -> Self {
        Self::INFINITE
    }
}

/// QoS liveliness policy.
///
/// `ManualByNode` was deprecated and removed from RMW; rmw_zenoh_cpp itself
/// never encodes or decodes wire value 2. It is kept here only because
/// `hiroz::qos::QosLiveliness` still carries the variant -- see `encode`'s
/// note on this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum QosLiveliness {
    #[default]
    Automatic = 1,
    ManualByNode = 2,
    ManualByTopic = 3,
}

/// QoS decode errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosDecodeError {
    InvalidFormat,
    InvalidReliability,
    InvalidDurability,
    InvalidHistory,
    InvalidDuration,
    InvalidLiveliness,
}

impl Display for QosDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            QosDecodeError::InvalidFormat => write!(f, "Invalid QoS format"),
            QosDecodeError::InvalidReliability => write!(f, "Invalid reliability value"),
            QosDecodeError::InvalidDurability => write!(f, "Invalid durability value"),
            QosDecodeError::InvalidHistory => write!(f, "Invalid history value"),
            QosDecodeError::InvalidDuration => write!(f, "Invalid duration value"),
            QosDecodeError::InvalidLiveliness => write!(f, "Invalid liveliness value"),
        }
    }
}
