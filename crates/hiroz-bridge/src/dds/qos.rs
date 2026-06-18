use hiroz_protocol::qos::{QosDurability, QosHistory, QosProfile, QosReliability};

use super::participant::{
    BridgeQos, Durability, DurabilityKind, History, HistoryKind, Reliability, ReliabilityKind,
};

pub use super::cyclors::qos::{adapt_reader_qos_for_writer, adapt_writer_qos_for_reader};

/// Convert `ros-z-protocol` `QosProfile` back to `BridgeQos` (for creating DDS entities from
/// parsed liveliness tokens).
pub fn qos_profile_to_bridge_qos(qos: &QosProfile) -> BridgeQos {
    let reliability = Some(Reliability {
        kind: match qos.reliability {
            QosReliability::Reliable => ReliabilityKind::Reliable,
            QosReliability::BestEffort => ReliabilityKind::BestEffort,
        },
        max_blocking_time: None,
    });
    let durability = Some(Durability {
        kind: match qos.durability {
            QosDurability::TransientLocal => DurabilityKind::TransientLocal,
            _ => DurabilityKind::Volatile,
        },
    });
    let history = Some(History {
        kind: match qos.history {
            QosHistory::KeepAll => HistoryKind::KeepAll,
            _ => HistoryKind::KeepLast,
        },
        depth: match qos.history {
            QosHistory::KeepLast(n) => n as i32,
            _ => 1,
        },
    });
    BridgeQos {
        reliability,
        durability,
        history,
        ..Default::default()
    }
}

/// Convert `BridgeQos` (DDS QoS) to `ros-z-protocol` `QosProfile` for key construction.
pub fn bridge_qos_to_qos_profile(qos: &BridgeQos) -> QosProfile {
    let reliability = qos
        .reliability
        .as_ref()
        .map(|r| match r.kind {
            ReliabilityKind::Reliable => QosReliability::Reliable,
            ReliabilityKind::BestEffort => QosReliability::BestEffort,
        })
        .unwrap_or_default();

    let durability = qos
        .durability
        .as_ref()
        .map(|d| match d.kind {
            DurabilityKind::TransientLocal => QosDurability::TransientLocal,
            _ => QosDurability::Volatile,
        })
        .unwrap_or_default();

    let history = qos
        .history
        .as_ref()
        .map(|h| match h.kind {
            HistoryKind::KeepAll => QosHistory::KeepAll,
            HistoryKind::KeepLast => QosHistory::KeepLast(h.depth as usize),
        })
        .unwrap_or_default();

    QosProfile {
        reliability,
        durability,
        history,
    }
}
