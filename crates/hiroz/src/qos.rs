use std::fmt;
use std::num::NonZeroUsize;

#[non_exhaustive]
#[derive(Debug, Default, Hash, PartialEq, Eq, Clone, Copy)]
pub enum QosReliability {
    #[default]
    Reliable,
    BestEffort,
}

impl fmt::Display for QosReliability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reliable => write!(f, "Reliable"),
            Self::BestEffort => write!(f, "Best Effort"),
        }
    }
}

/// Default depth for `KeepLast` when SYSTEM_DEFAULT (depth=0) is used.
/// Matches rclcpp's default of 10. Note this is distinct from
/// [`KEEP_ALL_CACHE_DEPTH`] below, which is the rmw_zenoh-aligned cap
/// applied to `KeepAll` when mapped to zenoh-ext's `cache.max_samples`.
pub const DEFAULT_HISTORY_DEPTH: usize = 10;

/// Cache/history depth used when a TransientLocal endpoint carries
/// `QosHistory::KeepAll` (no inherent depth) or `KeepLast(0)`.
///
/// Matches rmw_zenoh_cpp's `RMW_ZENOH_DEFAULT_HISTORY_DEPTH = 42`
/// (`rmw_zenoh_cpp/src/detail/qos.cpp:27`), which is the value
/// rmw_zenoh's `best_available_qos` substitutes for a zero-valued
/// `qos.depth` before passing it to
/// `AdvancedPublisherOptions::CacheOptions::max_samples`.
///
/// This is intentionally a *finite* cap that mirrors rmw_zenoh's
/// pragmatic behaviour rather than the DDS KEEP_ALL spec's "keep
/// everything" semantics.
pub const KEEP_ALL_CACHE_DEPTH: usize = 42;

#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum QosHistory {
    KeepLast(NonZeroUsize),
    KeepAll,
}

impl Default for QosHistory {
    fn default() -> Self {
        Self::KeepLast(NonZeroUsize::new(DEFAULT_HISTORY_DEPTH).unwrap())
    }
}

impl QosHistory {
    /// Normalize depth by replacing 0 with the default depth
    /// Used when converting from RMW QoS (which allows depth=0 for SYSTEM_DEFAULT)
    pub fn from_depth(depth: usize) -> Self {
        let normalized_depth = if depth == 0 {
            DEFAULT_HISTORY_DEPTH
        } else {
            depth
        };
        Self::KeepLast(
            NonZeroUsize::new(normalized_depth)
                .unwrap_or_else(|| NonZeroUsize::new(DEFAULT_HISTORY_DEPTH).unwrap()),
        )
    }
}

impl fmt::Display for QosHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeepLast(depth) => write!(f, "Keep Last ({})", depth),
            Self::KeepAll => write!(f, "Keep All"),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Default, Hash, PartialEq, Eq, Clone, Copy)]
pub enum QosDurability {
    TransientLocal,
    #[default]
    Volatile,
}

impl fmt::Display for QosDurability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransientLocal => write!(f, "Transient Local"),
            Self::Volatile => write!(f, "Volatile"),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Default, Hash, PartialEq, Eq, Clone, Copy)]
pub enum QosLiveliness {
    #[default]
    Automatic,
    ManualByNode,
    ManualByTopic,
}

impl fmt::Display for QosLiveliness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Automatic => write!(f, "Automatic"),
            Self::ManualByNode => write!(f, "Manual by Node"),
            Self::ManualByTopic => write!(f, "Manual by Topic"),
        }
    }
}

/// Represents a QoS duration in seconds and nanoseconds.
///
/// This is distinct from [`std::time::Duration`] and is used exclusively for
/// configuring QoS deadline, lifespan, and liveliness lease duration.
/// Use [`QosDuration::INFINITE`] (the default) to disable a QoS time constraint.
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct QosDuration {
    pub sec: u64,
    pub nsec: u64,
}

impl QosDuration {
    pub const INFINITE: QosDuration = QosDuration {
        sec: 9223372036,
        nsec: 854775807,
    };
}

impl Default for QosDuration {
    fn default() -> Self {
        Self::INFINITE
    }
}

impl From<std::time::Duration> for QosDuration {
    fn from(d: std::time::Duration) -> Self {
        Self {
            sec: d.as_secs(),
            nsec: d.subsec_nanos() as u64,
        }
    }
}

impl fmt::Display for QosDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::INFINITE {
            write!(f, "Infinite")
        } else if self.nsec == 0 {
            write!(f, "{}s", self.sec)
        } else {
            write!(f, "{}s {}ns", self.sec, self.nsec)
        }
    }
}

#[derive(Debug, Default, Hash, PartialEq, Eq, Clone, Copy)]
pub struct QosProfile {
    pub reliability: QosReliability,
    pub durability: QosDurability,
    pub history: QosHistory,
    pub deadline: QosDuration,
    pub lifespan: QosDuration,
    pub liveliness: QosLiveliness,
    pub liveliness_lease_duration: QosDuration,
}

impl QosProfile {
    /// Convert to hiroz-protocol's QosProfile for key expression generation
    pub fn to_protocol_qos(&self) -> hiroz_protocol::qos::QosProfile {
        hiroz_protocol::qos::QosProfile {
            reliability: match self.reliability {
                QosReliability::Reliable => hiroz_protocol::qos::QosReliability::Reliable,
                QosReliability::BestEffort => hiroz_protocol::qos::QosReliability::BestEffort,
            },
            durability: match self.durability {
                QosDurability::TransientLocal => hiroz_protocol::qos::QosDurability::TransientLocal,
                QosDurability::Volatile => hiroz_protocol::qos::QosDurability::Volatile,
            },
            history: match self.history {
                QosHistory::KeepLast(depth) => {
                    hiroz_protocol::qos::QosHistory::KeepLast(depth.get())
                }
                QosHistory::KeepAll => hiroz_protocol::qos::QosHistory::KeepAll,
            },
        }
    }
}

impl fmt::Display for QosProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "QoS({}, {}, {}",
            self.reliability, self.durability, self.history
        )?;
        if self.deadline != QosDuration::INFINITE {
            write!(f, ", deadline={}", self.deadline)?;
        }
        if self.lifespan != QosDuration::INFINITE {
            write!(f, ", lifespan={}", self.lifespan)?;
        }
        if self.liveliness != QosLiveliness::Automatic {
            write!(f, ", liveliness={}", self.liveliness)?;
        }
        if self.liveliness_lease_duration != QosDuration::INFINITE {
            write!(f, ", lease={}", self.liveliness_lease_duration)?;
        }
        write!(f, ")")
    }
}

const QOS_DELIMITER: &str = ":";

#[derive(Debug)]
pub enum QosDecodeError {
    IncompleteQos,
    InvalidReliability,
    InvalidDurability,
    InvalidHistory,
    InvalidHistoryDepth,
}

impl std::fmt::Display for QosDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompleteQos => write!(f, "Incomplete QoS string"),
            Self::InvalidReliability => write!(f, "Invalid reliability value in QoS"),
            Self::InvalidDurability => write!(f, "Invalid durability value in QoS"),
            Self::InvalidHistory => write!(f, "Invalid history value in QoS"),
            Self::InvalidHistoryDepth => write!(f, "Invalid history depth value in QoS"),
        }
    }
}

impl std::error::Error for QosDecodeError {}

impl QosProfile {
    // This format comes from rmw_zenoh
    // <ReliabilityKind>:<DurabilityKind>:<HistoryKind>,<HistoryDepth>:<DeadlineSec, DeadlineNSec>:<LifespanSec, LifespanNSec>:<Liveliness, LivelinessSec, LivelinessNSec>"
    pub fn encode(&self) -> String {
        let default_qos = Self::default();

        // Reliability - empty if default
        let reliability = if self.reliability != default_qos.reliability {
            match self.reliability {
                QosReliability::Reliable => "1",
                QosReliability::BestEffort => "2",
            }
        } else {
            ""
        };

        // Durability - empty if default
        let durability = if self.durability != default_qos.durability {
            match self.durability {
                QosDurability::TransientLocal => "1",
                QosDurability::Volatile => "2",
            }
        } else {
            ""
        };

        // History format: <history_kind>,<depth>
        // Only include kind if it's non-default
        // Always include depth (even if default)
        let history = match self.history {
            QosHistory::KeepLast(depth) => {
                if self.history != default_qos.history {
                    // Non-default history kind - include both kind and depth
                    format!("1,{}", depth.get())
                } else {
                    // Default history kind - only include depth
                    format!(",{}", depth.get())
                }
            }
            QosHistory::KeepAll => "2,".to_string(),
        };

        // Deadline - empty if default (infinite)
        let deadline = if self.deadline != default_qos.deadline {
            format!("{},{}", self.deadline.sec, self.deadline.nsec)
        } else {
            ",".to_string()
        };

        // Lifespan - empty if default (infinite)
        let lifespan = if self.lifespan != default_qos.lifespan {
            format!("{},{}", self.lifespan.sec, self.lifespan.nsec)
        } else {
            ",".to_string()
        };

        // Liveliness - format: <liveliness_kind>,<lease_sec>,<lease_nsec>
        let liveliness = if self.liveliness != default_qos.liveliness
            || self.liveliness_lease_duration != default_qos.liveliness_lease_duration
        {
            let kind = match self.liveliness {
                QosLiveliness::Automatic => "1",
                QosLiveliness::ManualByNode => "2",
                QosLiveliness::ManualByTopic => "3",
            };
            format!(
                "{},{},{}",
                kind, self.liveliness_lease_duration.sec, self.liveliness_lease_duration.nsec
            )
        } else {
            ",,".to_string()
        };

        format!(
            "{}:{}:{}:{}:{}:{}",
            reliability, durability, history, deadline, lifespan, liveliness
        )
    }

    pub fn decode(encoded: impl AsRef<str>) -> Result<Self, QosDecodeError> {
        let mut fields = encoded.as_ref().split(QOS_DELIMITER);
        let reliability = match fields.next() {
            Some(x) => match x {
                "0" | "" => QosReliability::default(),
                "1" => QosReliability::Reliable,
                "2" => QosReliability::BestEffort,
                _ => return Err(QosDecodeError::InvalidReliability),
            },
            None => return Err(QosDecodeError::IncompleteQos),
        };
        let durability = match fields.next() {
            Some(x) => match x {
                "0" | "" => QosDurability::default(),
                "1" => QosDurability::TransientLocal,
                "2" => QosDurability::Volatile,
                _ => return Err(QosDecodeError::InvalidDurability),
            },
            None => return Err(QosDecodeError::IncompleteQos),
        };
        let history = match fields.next() {
            Some(x) => match x {
                "," | "" => QosHistory::default(),
                x => {
                    let mut iter = x.split(",");
                    let Some(kind) = iter.next() else {
                        return Err(QosDecodeError::IncompleteQos);
                    };
                    let Some(depth) = iter.next() else {
                        return Err(QosDecodeError::IncompleteQos);
                    };
                    match (kind, depth) {
                        ("", d) | ("0", d) | ("1", d) => {
                            let depth_usize: usize =
                                d.parse().map_err(|_| QosDecodeError::InvalidHistory)?;
                            let non_zero_depth = NonZeroUsize::new(depth_usize)
                                .ok_or(QosDecodeError::InvalidHistoryDepth)?;
                            QosHistory::KeepLast(non_zero_depth)
                        }
                        ("2", _) => QosHistory::KeepAll,
                        _ => return Err(QosDecodeError::InvalidHistory),
                    }
                }
            },
            None => return Err(QosDecodeError::IncompleteQos),
        };

        // Deadline - format: <sec>,<nsec>
        let deadline = match fields.next() {
            Some(x) if x.is_empty() || x == "," => QosDuration::default(),
            Some(x) => {
                let mut iter = x.split(",");
                let sec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.sec);
                let nsec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.nsec);
                QosDuration { sec, nsec }
            }
            None => QosDuration::default(),
        };

        // Lifespan - format: <sec>,<nsec>
        let lifespan = match fields.next() {
            Some(x) if x.is_empty() || x == "," => QosDuration::default(),
            Some(x) => {
                let mut iter = x.split(",");
                let sec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.sec);
                let nsec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.nsec);
                QosDuration { sec, nsec }
            }
            None => QosDuration::default(),
        };

        // Liveliness - format: <kind>,<lease_sec>,<lease_nsec>
        let (liveliness, liveliness_lease_duration) = match fields.next() {
            Some(x) if x.is_empty() || x == ",," => {
                (QosLiveliness::default(), QosDuration::default())
            }
            Some(x) => {
                let mut iter = x.split(",");
                let kind = match iter.next().unwrap_or("") {
                    "" | "0" | "1" => QosLiveliness::Automatic,
                    "2" => QosLiveliness::ManualByNode,
                    "3" => QosLiveliness::ManualByTopic,
                    _ => QosLiveliness::default(),
                };
                let sec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.sec);
                let nsec = iter
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(QosDuration::INFINITE.nsec);
                (kind, QosDuration { sec, nsec })
            }
            None => (QosLiveliness::default(), QosDuration::default()),
        };

        Ok(Self {
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

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;

    // -----------------------------------------------------------------------
    // Reliability mapping: QosReliability → protocol::QosReliability
    // -----------------------------------------------------------------------

    #[test]
    fn test_reliable_maps_to_protocol_reliable() {
        let qos = QosProfile {
            reliability: QosReliability::Reliable,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::Reliable
        );
    }

    #[test]
    fn test_best_effort_maps_to_protocol_best_effort() {
        let qos = QosProfile {
            reliability: QosReliability::BestEffort,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.reliability,
            hiroz_protocol::qos::QosReliability::BestEffort
        );
    }

    // -----------------------------------------------------------------------
    // Durability mapping
    // -----------------------------------------------------------------------

    #[test]
    fn test_volatile_maps_to_protocol_volatile() {
        let qos = QosProfile {
            durability: QosDurability::Volatile,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.durability,
            hiroz_protocol::qos::QosDurability::Volatile
        );
    }

    #[test]
    fn test_transient_local_maps_to_protocol_transient_local() {
        let qos = QosProfile {
            durability: QosDurability::TransientLocal,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(
            proto.durability,
            hiroz_protocol::qos::QosDurability::TransientLocal
        );
    }

    // -----------------------------------------------------------------------
    // History mapping: depth is preserved
    // -----------------------------------------------------------------------

    #[test]
    fn test_keep_last_depth_is_preserved() {
        let depth = NonZeroUsize::new(7).unwrap();
        let qos = QosProfile {
            history: QosHistory::KeepLast(depth),
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(proto.history, hiroz_protocol::qos::QosHistory::KeepLast(7));
    }

    #[test]
    fn test_keep_all_maps_to_protocol_keep_all() {
        let qos = QosProfile {
            history: QosHistory::KeepAll,
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(proto.history, hiroz_protocol::qos::QosHistory::KeepAll);
    }

    #[test]
    fn test_keep_last_depth_1_is_preserved() {
        let depth = NonZeroUsize::new(1).unwrap();
        let qos = QosProfile {
            history: QosHistory::KeepLast(depth),
            ..Default::default()
        };
        let proto = qos.to_protocol_qos();
        assert_eq!(proto.history, hiroz_protocol::qos::QosHistory::KeepLast(1));
    }

    // -----------------------------------------------------------------------
    // QoS encode/decode roundtrip (pure string logic, no Zenoh session)
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_decode_reliable_volatile_keep_last() {
        let qos = QosProfile {
            reliability: QosReliability::Reliable,
            durability: QosDurability::Volatile,
            history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
            ..Default::default()
        };
        let encoded = qos.encode();
        let decoded = QosProfile::decode(&encoded).expect("decode");
        assert_eq!(decoded.reliability, qos.reliability);
        assert_eq!(decoded.durability, qos.durability);
        assert_eq!(decoded.history, qos.history);
    }

    #[test]
    fn test_encode_decode_best_effort_transient_keep_last() {
        let qos = QosProfile {
            reliability: QosReliability::BestEffort,
            durability: QosDurability::TransientLocal,
            history: QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
            ..Default::default()
        };
        let encoded = qos.encode();
        let decoded = QosProfile::decode(&encoded).expect("decode");
        assert_eq!(decoded.reliability, qos.reliability);
        assert_eq!(decoded.durability, qos.durability);
        assert_eq!(decoded.history, qos.history);
    }

    // -----------------------------------------------------------------------
    // QosHistory::from_depth normalizes depth=0 to DEFAULT_HISTORY_DEPTH
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_depth_zero_uses_default() {
        let h = QosHistory::from_depth(0);
        assert_eq!(
            h,
            QosHistory::KeepLast(NonZeroUsize::new(DEFAULT_HISTORY_DEPTH).unwrap())
        );
    }

    #[test]
    fn test_from_depth_nonzero_preserved() {
        let h = QosHistory::from_depth(3);
        assert_eq!(h, QosHistory::KeepLast(NonZeroUsize::new(3).unwrap()));
    }

    #[test]
    fn test_qos_duration_from_std_duration() {
        let d = std::time::Duration::new(3, 500_000_000);
        let qd = QosDuration::from(d);
        assert_eq!(qd.sec, 3);
        assert_eq!(qd.nsec, 500_000_000);
    }

    #[test]
    fn test_qos_duration_from_std_duration_zero() {
        let d = std::time::Duration::ZERO;
        let qd = QosDuration::from(d);
        assert_eq!(qd.sec, 0);
        assert_eq!(qd.nsec, 0);
    }
}
