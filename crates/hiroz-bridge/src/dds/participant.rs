use std::time::Duration;

use anyhow::Result;

use super::discovery::DiscoveryEvent;

// ─── QoS kinds ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReliabilityKind {
    BestEffort,
    Reliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityKind {
    Volatile,
    TransientLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryKind {
    KeepLast,
    KeepAll,
}

// ─── QoS policy structs ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Reliability {
    pub kind: ReliabilityKind,
    /// `None` = DDS infinite blocking time.
    pub max_blocking_time: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Durability {
    pub kind: DurabilityKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub kind: HistoryKind,
    pub depth: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurabilityService {
    /// `None` = DDS infinite cleanup delay.
    pub service_cleanup_delay: Option<Duration>,
    pub history_kind: HistoryKind,
    pub history_depth: i32,
    /// `None` = DDS_LENGTH_UNLIMITED.
    pub max_samples: Option<usize>,
    /// `None` = DDS_LENGTH_UNLIMITED.
    pub max_instances: Option<usize>,
    /// `None` = DDS_LENGTH_UNLIMITED.
    pub max_samples_per_instance: Option<usize>,
}

// ─── BridgeQos ────────────────────────────────────────────────────────────────

/// Backend-independent QoS representation for the bridge.
///
/// Covers the DDS-standard policies needed for pub/sub and service routing.
/// Backend-specific fields (e.g. CycloneDDS `entity_name`, `properties`,
/// `writer_batching`) are handled inside each backend's conversion layer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BridgeQos {
    pub reliability: Option<Reliability>,
    pub durability: Option<Durability>,
    pub history: Option<History>,
    pub durability_service: Option<DurabilityService>,
    /// `None` = infinite deadline.
    pub deadline: Option<Duration>,
    pub latency_budget: Option<Duration>,
    /// `None` = infinite lifespan.
    pub lifespan: Option<Duration>,
    pub user_data: Option<Vec<u8>>,
    /// When true the reader/writer ignores traffic from other endpoints
    /// in the same DDS participant (IgnoreLocal::PARTICIPANT).
    pub ignore_local: bool,
}

// ─── Backend traits ───────────────────────────────────────────────────────────

/// A live DDS participant on a single domain that can create readers/writers
/// and stream discovery events.
///
/// Implement this trait to plug in a DDS library.  The cyclors (CycloneDDS)
/// implementation lives in `dds::cyclors`.
pub trait DdsParticipant: Send + Sync + 'static {
    /// RAII reader handle — dropped when the reader should be destroyed.
    type Reader: DdsReader;
    /// Writer handle that exposes `write_cdr`, `instance_handle`, and `guid`.
    type Writer: DdsWriter;

    /// Create a participant on `domain_id`.
    fn create(domain_id: u32) -> Result<Self>
    where
        Self: Sized;

    /// Return this participant's 16-byte DDS GUID.
    fn participant_guid(&self) -> Result<[u8; 16]>;

    /// Start the builtin-topic discovery loop.
    ///
    /// Endpoint events are sent on the returned channel until the participant
    /// (and therefore the channel sender) is dropped.
    fn run_discovery(&self) -> flume::Receiver<DiscoveryEvent>;

    /// Create a CDR blob reader; `callback` is called for each valid sample.
    ///
    /// The `Vec<u8>` passed to `callback` includes the 4-byte CDR representation
    /// header followed by the payload, matching the raw DDS wire format.
    fn create_reader(
        &self,
        topic: &str,
        type_name: &str,
        keyless: bool,
        qos: BridgeQos,
        callback: Box<dyn Fn(Vec<u8>) + Send + 'static>,
    ) -> Result<Self::Reader>;

    /// Create a CDR blob writer.
    fn create_writer(
        &self,
        topic: &str,
        type_name: &str,
        keyless: bool,
        qos: BridgeQos,
    ) -> Result<Self::Writer>;
}

/// A live DDS reader; minimal trait so the bridge can obtain its GUID.
pub trait DdsReader: Send + Sync + 'static {
    /// Return this reader's 16-byte DDS GUID.
    fn guid(&self) -> Result<[u8; 16]>;
}

/// A live DDS writer that can send raw CDR blobs.
pub trait DdsWriter: Send + Sync + 'static {
    /// Write raw CDR bytes (4-byte representation header + payload) to DDS.
    fn write_cdr(&self, data: Vec<u8>) -> Result<()>;

    /// Return this writer's stable instance handle (GUID fragment).
    ///
    /// Used as the `client_guid` in ROS 2 service request headers so that
    /// the DDS server routes replies back to this specific writer.
    fn instance_handle(&self) -> Result<u64>;

    /// Return this writer's 16-byte DDS GUID.
    fn guid(&self) -> Result<[u8; 16]>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── wire_discriminant values must match zenoh-plugin-ros2dds ─────────────
    // References: zenoh-plugin-ros2dds liveliness_mgt.rs, which encodes these as
    // integers in the liveliness key expression for cross-bridge entity matching.

    impl ReliabilityKind {
        fn wire_discriminant(self) -> i32 {
            match self {
                Self::BestEffort => 0,
                Self::Reliable => 1,
            }
        }
    }

    impl DurabilityKind {
        fn wire_discriminant(self) -> i32 {
            match self {
                Self::Volatile => 0,
                Self::TransientLocal => 1,
            }
        }
    }

    impl HistoryKind {
        fn wire_discriminant(self) -> i32 {
            match self {
                Self::KeepLast => 0,
                Self::KeepAll => 1,
            }
        }
    }

    #[test]
    fn test_reliability_wire_discriminants() {
        assert_eq!(ReliabilityKind::BestEffort.wire_discriminant(), 0);
        assert_eq!(ReliabilityKind::Reliable.wire_discriminant(), 1);
    }

    #[test]
    fn test_durability_wire_discriminants() {
        assert_eq!(DurabilityKind::Volatile.wire_discriminant(), 0);
        assert_eq!(DurabilityKind::TransientLocal.wire_discriminant(), 1);
    }

    #[test]
    fn test_history_wire_discriminants() {
        assert_eq!(HistoryKind::KeepLast.wire_discriminant(), 0);
        assert_eq!(HistoryKind::KeepAll.wire_discriminant(), 1);
    }

    #[test]
    fn test_bridge_qos_default_has_no_policies() {
        let qos = BridgeQos::default();
        assert!(qos.reliability.is_none());
        assert!(qos.durability.is_none());
        assert!(qos.history.is_none());
        assert!(qos.durability_service.is_none());
        assert!(qos.user_data.is_none());
        assert!(!qos.ignore_local);
    }

    #[test]
    fn test_bridge_qos_equality() {
        use std::time::Duration;
        let a = BridgeQos {
            reliability: Some(Reliability {
                kind: ReliabilityKind::Reliable,
                max_blocking_time: Some(Duration::from_millis(100)),
            }),
            ..Default::default()
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
