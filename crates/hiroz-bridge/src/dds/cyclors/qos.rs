use std::time::Duration;

use cyclors::qos::{
    DDS_INFINITE_TIME, Durability as CDurability, DurabilityKind as CDurabilityKind,
    DurabilityService as CDurabilityService, History as CHistory, HistoryKind as CHistoryKind,
    IgnoreLocal, IgnoreLocalKind, Qos, Reliability as CReliability,
    ReliabilityKind as CReliabilityKind,
};

use crate::dds::participant::{
    BridgeQos, Durability, DurabilityKind, DurabilityService, History, HistoryKind, Reliability,
    ReliabilityKind,
};

pub const DDS_LENGTH_UNLIMITED: i32 = -1;

// ─── Duration helpers ─────────────────────────────────────────────────────────

fn duration_to_nanos(d: Duration) -> i64 {
    d.as_nanos().min(i64::MAX as u128) as i64
}

fn nanos_to_duration(ns: i64) -> Option<Duration> {
    if ns == DDS_INFINITE_TIME || ns < 0 {
        None
    } else {
        Some(Duration::from_nanos(ns as u64))
    }
}

fn duration_option_to_nanos(d: Option<Duration>) -> i64 {
    d.map(duration_to_nanos).unwrap_or(DDS_INFINITE_TIME)
}

fn max_to_option(v: i32) -> Option<usize> {
    if v == DDS_LENGTH_UNLIMITED {
        None
    } else {
        Some(v.max(0) as usize)
    }
}

fn option_to_max(v: Option<usize>) -> i32 {
    v.map(|n| n.min(i32::MAX as usize) as i32)
        .unwrap_or(DDS_LENGTH_UNLIMITED)
}

// ─── Kind conversions ─────────────────────────────────────────────────────────

impl From<CReliabilityKind> for ReliabilityKind {
    fn from(k: CReliabilityKind) -> Self {
        match k {
            CReliabilityKind::BEST_EFFORT => Self::BestEffort,
            CReliabilityKind::RELIABLE => Self::Reliable,
        }
    }
}

impl From<ReliabilityKind> for CReliabilityKind {
    fn from(k: ReliabilityKind) -> Self {
        match k {
            ReliabilityKind::BestEffort => Self::BEST_EFFORT,
            ReliabilityKind::Reliable => Self::RELIABLE,
        }
    }
}

impl From<CDurabilityKind> for DurabilityKind {
    fn from(k: CDurabilityKind) -> Self {
        match k {
            CDurabilityKind::VOLATILE => Self::Volatile,
            CDurabilityKind::TRANSIENT_LOCAL => Self::TransientLocal,
            // TRANSIENT and PERSISTENT are not used by ROS 2 nodes; treat as Volatile.
            _ => Self::Volatile,
        }
    }
}

impl From<DurabilityKind> for CDurabilityKind {
    fn from(k: DurabilityKind) -> Self {
        match k {
            DurabilityKind::Volatile => Self::VOLATILE,
            DurabilityKind::TransientLocal => Self::TRANSIENT_LOCAL,
        }
    }
}

impl From<CHistoryKind> for HistoryKind {
    fn from(k: CHistoryKind) -> Self {
        match k {
            CHistoryKind::KEEP_LAST => Self::KeepLast,
            CHistoryKind::KEEP_ALL => Self::KeepAll,
        }
    }
}

impl From<HistoryKind> for CHistoryKind {
    fn from(k: HistoryKind) -> Self {
        match k {
            HistoryKind::KeepLast => Self::KEEP_LAST,
            HistoryKind::KeepAll => Self::KEEP_ALL,
        }
    }
}

// ─── BridgeQos ↔ cyclors::qos::Qos ──────────────────────────────────────────

impl From<Qos> for BridgeQos {
    fn from(q: Qos) -> Self {
        Self {
            reliability: q.reliability.map(|r| Reliability {
                kind: r.kind.into(),
                max_blocking_time: nanos_to_duration(r.max_blocking_time),
            }),
            durability: q.durability.map(|d| Durability {
                kind: d.kind.into(),
            }),
            history: q.history.map(|h| History {
                kind: h.kind.into(),
                depth: h.depth,
            }),
            durability_service: q.durability_service.map(|ds| DurabilityService {
                service_cleanup_delay: nanos_to_duration(ds.service_cleanup_delay),
                history_kind: ds.history_kind.into(),
                history_depth: ds.history_depth,
                max_samples: max_to_option(ds.max_samples),
                max_instances: max_to_option(ds.max_instances),
                max_samples_per_instance: max_to_option(ds.max_samples_per_instance),
            }),
            deadline: q.deadline.and_then(|d| nanos_to_duration(d.period)),
            latency_budget: q
                .latency_budget
                .and_then(|lb| nanos_to_duration(lb.duration)),
            lifespan: q.lifespan.and_then(|ls| nanos_to_duration(ls.duration)),
            user_data: q.user_data,
            ignore_local: q
                .ignore_local
                .is_some_and(|il| il.kind == IgnoreLocalKind::PARTICIPANT),
        }
    }
}

impl From<BridgeQos> for Qos {
    fn from(bq: BridgeQos) -> Self {
        Qos {
            reliability: bq.reliability.map(|r| CReliability {
                kind: r.kind.into(),
                max_blocking_time: duration_option_to_nanos(r.max_blocking_time),
            }),
            durability: bq.durability.map(|d| CDurability {
                kind: d.kind.into(),
            }),
            history: bq.history.map(|h| CHistory {
                kind: h.kind.into(),
                depth: h.depth,
            }),
            durability_service: bq.durability_service.map(|ds| CDurabilityService {
                service_cleanup_delay: duration_option_to_nanos(ds.service_cleanup_delay),
                history_kind: ds.history_kind.into(),
                history_depth: ds.history_depth,
                max_samples: option_to_max(ds.max_samples),
                max_instances: option_to_max(ds.max_instances),
                max_samples_per_instance: option_to_max(ds.max_samples_per_instance),
            }),
            deadline: bq.deadline.map(|d| cyclors::qos::Deadline {
                period: duration_to_nanos(d),
            }),
            latency_budget: bq.latency_budget.map(|d| cyclors::qos::LatencyBudget {
                duration: duration_to_nanos(d),
            }),
            lifespan: bq.lifespan.map(|d| cyclors::qos::Lifespan {
                duration: duration_to_nanos(d),
            }),
            user_data: bq.user_data,
            ignore_local: if bq.ignore_local {
                Some(IgnoreLocal {
                    kind: IgnoreLocalKind::PARTICIPANT,
                })
            } else {
                None
            },
            ..Default::default()
        }
    }
}

/// Adapt a discovered DDS writer's QoS to create a compatible reader.
pub fn adapt_writer_qos_for_reader(qos: &BridgeQos) -> BridgeQos {
    BridgeQos {
        reliability: Some(qos.reliability.clone().unwrap_or(Reliability {
            kind: ReliabilityKind::BestEffort,
            max_blocking_time: Some(DDS_100MS_DURATION_STD),
        })),
        durability: qos.durability.clone(),
        history: qos.history.clone(),
        user_data: qos.user_data.clone(),
        ignore_local: true,
        ..Default::default()
    }
}

/// Adapt a discovered DDS reader's QoS to create a compatible writer.
pub fn adapt_reader_qos_for_writer(qos: &BridgeQos) -> BridgeQos {
    let is_transient = qos
        .durability
        .as_ref()
        .is_some_and(|d| d.kind == DurabilityKind::TransientLocal);

    let history_kind = qos
        .history
        .as_ref()
        .map_or(HistoryKind::KeepLast, |h| h.kind);
    let history_depth = qos.history.as_ref().map_or(1, |h| h.depth);

    let durability_service = if is_transient {
        Some(DurabilityService {
            service_cleanup_delay: Some(Duration::from_secs(60)),
            history_kind,
            history_depth,
            max_samples: None,
            max_instances: None,
            max_samples_per_instance: None,
        })
    } else {
        None
    };

    // Bump max_blocking_time by 1 ns for FastRTPS interop.
    let reliability = Some(match &qos.reliability {
        Some(r) => Reliability {
            kind: r.kind,
            max_blocking_time: Some(
                r.max_blocking_time
                    .unwrap_or(Duration::from_millis(100))
                    .saturating_add(Duration::from_nanos(1)),
            ),
        },
        None => Reliability {
            kind: ReliabilityKind::Reliable,
            max_blocking_time: Some(DDS_100MS_DURATION_STD + Duration::from_nanos(1)),
        },
    });

    BridgeQos {
        reliability,
        durability: qos.durability.clone(),
        history: qos.history.clone(),
        durability_service,
        user_data: qos.user_data.clone(),
        ignore_local: true,
        ..Default::default()
    }
}

const DDS_100MS_DURATION_STD: Duration = Duration::from_millis(100);

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reliable_bridge_qos() -> BridgeQos {
        BridgeQos {
            reliability: Some(Reliability {
                kind: ReliabilityKind::Reliable,
                max_blocking_time: Some(Duration::from_millis(100)),
            }),
            ..Default::default()
        }
    }

    fn transient_bridge_qos(depth: i32) -> BridgeQos {
        BridgeQos {
            durability: Some(Durability {
                kind: DurabilityKind::TransientLocal,
            }),
            history: Some(History {
                kind: HistoryKind::KeepLast,
                depth,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_round_trip_reliable_qos() {
        let bq = reliable_bridge_qos();
        let cq: Qos = bq.clone().into();
        let back: BridgeQos = cq.into();
        assert_eq!(back.reliability, bq.reliability);
    }

    #[test]
    fn test_round_trip_transient_local() {
        let bq = transient_bridge_qos(5);
        let cq: Qos = bq.clone().into();
        let back: BridgeQos = cq.into();
        assert_eq!(back.durability, bq.durability);
        assert_eq!(back.history, bq.history);
    }

    #[test]
    fn test_none_duration_becomes_infinite() {
        let bq = BridgeQos {
            reliability: Some(Reliability {
                kind: ReliabilityKind::Reliable,
                max_blocking_time: None,
            }),
            ..Default::default()
        };
        let cq: Qos = bq.into();
        assert_eq!(cq.reliability.unwrap().max_blocking_time, DDS_INFINITE_TIME);
    }

    #[test]
    fn test_unlimited_instances_roundtrip() {
        let bq = BridgeQos {
            durability_service: Some(DurabilityService {
                service_cleanup_delay: Some(Duration::from_secs(60)),
                history_kind: HistoryKind::KeepLast,
                history_depth: 1,
                max_samples: None,
                max_instances: None,
                max_samples_per_instance: None,
            }),
            ..Default::default()
        };
        let cq: Qos = bq.into();
        let ds = cq.durability_service.unwrap();
        assert_eq!(ds.max_instances, DDS_LENGTH_UNLIMITED);
    }

    #[test]
    fn test_adapt_writer_sets_ignore_local() {
        let adapted = adapt_writer_qos_for_reader(&reliable_bridge_qos());
        assert!(adapted.ignore_local);
    }

    #[test]
    fn test_adapt_reader_transient_adds_durability_service() {
        let adapted = adapt_reader_qos_for_writer(&transient_bridge_qos(5));
        let ds = adapted.durability_service.expect("durability_service set");
        assert_eq!(ds.history_depth, 5);
        assert_eq!(ds.max_instances, None);
    }
}
