//! C-compatible QoS types for FFI

use crate::qos::{
    QosDurability, QosDuration, QosHistory, QosLiveliness, QosProfile, QosReliability,
};
use std::num::NonZeroUsize;

/// C-compatible QoS profile for FFI
#[repr(C)]
pub struct CQosProfile {
    /// 0 = Reliable (default), 1 = BestEffort
    pub reliability: i32,
    /// 0 = Volatile (default), 1 = TransientLocal
    pub durability: i32,
    /// 0 = KeepLast (default), 1 = KeepAll
    pub history: i32,
    /// Depth for KeepLast (default: 10, ignored for KeepAll)
    pub history_depth: i32,
    pub deadline_sec: u64,
    pub deadline_nsec: u64,
    pub lifespan_sec: u64,
    pub lifespan_nsec: u64,
    /// 0 = Automatic (default), 1 = ManualByNode, 2 = ManualByTopic
    pub liveliness: i32,
    pub liveliness_lease_sec: u64,
    pub liveliness_lease_nsec: u64,
}

impl CQosProfile {
    /// Convert a nullable pointer to an optional QosProfile
    ///
    /// # Safety
    /// Pointer must be valid or null.
    pub unsafe fn to_qos_profile(ptr: *const CQosProfile) -> Option<QosProfile> {
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { (*ptr).as_qos_profile() })
    }

    fn as_qos_profile(&self) -> QosProfile {
        let reliability = match self.reliability {
            1 => QosReliability::BestEffort,
            _ => QosReliability::Reliable,
        };

        let durability = match self.durability {
            1 => QosDurability::TransientLocal,
            _ => QosDurability::Volatile,
        };

        let history = match self.history {
            1 => QosHistory::KeepAll,
            _ => {
                let depth = if self.history_depth <= 0 {
                    crate::qos::DEFAULT_HISTORY_DEPTH
                } else {
                    self.history_depth as usize
                };
                QosHistory::KeepLast(
                    NonZeroUsize::new(depth)
                        .unwrap_or(NonZeroUsize::new(crate::qos::DEFAULT_HISTORY_DEPTH).unwrap()),
                )
            }
        };

        let deadline = QosDuration {
            sec: self.deadline_sec,
            nsec: self.deadline_nsec,
        };

        let lifespan = QosDuration {
            sec: self.lifespan_sec,
            nsec: self.lifespan_nsec,
        };

        let liveliness = match self.liveliness {
            1 => QosLiveliness::ManualByNode,
            2 => QosLiveliness::ManualByTopic,
            _ => QosLiveliness::Automatic,
        };

        let liveliness_lease_duration = QosDuration {
            sec: self.liveliness_lease_sec,
            nsec: self.liveliness_lease_nsec,
        };

        QosProfile {
            reliability,
            durability,
            history,
            deadline,
            lifespan,
            liveliness,
            liveliness_lease_duration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_qos() {
        let c_qos = CQosProfile {
            reliability: 0,
            durability: 0,
            history: 0,
            history_depth: 10,
            deadline_sec: QosDuration::INFINITE.sec,
            deadline_nsec: QosDuration::INFINITE.nsec,
            lifespan_sec: QosDuration::INFINITE.sec,
            lifespan_nsec: QosDuration::INFINITE.nsec,
            liveliness: 0,
            liveliness_lease_sec: QosDuration::INFINITE.sec,
            liveliness_lease_nsec: QosDuration::INFINITE.nsec,
        };

        let qos = c_qos.as_qos_profile();
        assert_eq!(qos, QosProfile::default());
    }

    #[test]
    fn test_best_effort_transient_local() {
        let c_qos = CQosProfile {
            reliability: 1,
            durability: 1,
            history: 1,
            history_depth: 0,
            deadline_sec: QosDuration::INFINITE.sec,
            deadline_nsec: QosDuration::INFINITE.nsec,
            lifespan_sec: QosDuration::INFINITE.sec,
            lifespan_nsec: QosDuration::INFINITE.nsec,
            liveliness: 0,
            liveliness_lease_sec: QosDuration::INFINITE.sec,
            liveliness_lease_nsec: QosDuration::INFINITE.nsec,
        };

        let qos = c_qos.as_qos_profile();
        assert_eq!(qos.reliability, QosReliability::BestEffort);
        assert_eq!(qos.durability, QosDurability::TransientLocal);
        assert_eq!(qos.history, QosHistory::KeepAll);
    }

    #[test]
    fn test_null_pointer_returns_none() {
        let result = unsafe { CQosProfile::to_qos_profile(std::ptr::null()) };
        assert!(result.is_none());
    }
}
