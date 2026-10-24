//! C-compatible QoS types for FFI

use super::action::ActionQos;
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

/// Versioned C-compatible QoS profile for FFI
///
/// `struct_size` comes first. The caller sets it to `sizeof(hiroz_qos_profile_v2_t)` of the
/// header it compiled against, or calls `hiroz_qos_profile_v2_init`, which sets it. A later
/// release can add fields at the end. hiroz reads only the fields inside `struct_size` and takes
/// the default for the others, so a program built against an older header keeps working. A
/// `struct_size` below 4 is invalid: the creator returns null.
///
/// The fields after `struct_size` have the meaning that they have in `hiroz_qos_profile_t`.
#[repr(C)]
pub struct CQosProfileV2 {
    /// `sizeof(hiroz_qos_profile_v2_t)` as the caller knows it
    pub struct_size: u32,
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

/// A `struct_size` that cannot hold the size field. The value is the size the caller gave.
#[derive(Debug)]
pub struct InvalidStructSize(pub u32);

impl CQosProfileV2 {
    /// The size of the struct in this build.
    const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// The smallest valid `struct_size`: the size field itself.
    const MIN_SIZE: u32 = std::mem::size_of::<u32>() as u32;

    /// The default profile. It has the same values as `QosProfile::default()`.
    fn defaults() -> Self {
        Self {
            struct_size: Self::SIZE,
            reliability: 0,
            durability: 0,
            history: 0,
            history_depth: crate::qos::DEFAULT_HISTORY_DEPTH as i32,
            deadline_sec: QosDuration::INFINITE.sec,
            deadline_nsec: QosDuration::INFINITE.nsec,
            lifespan_sec: QosDuration::INFINITE.sec,
            lifespan_nsec: QosDuration::INFINITE.nsec,
            liveliness: 0,
            liveliness_lease_sec: QosDuration::INFINITE.sec,
            liveliness_lease_nsec: QosDuration::INFINITE.nsec,
        }
    }

    /// Convert a nullable pointer to an optional QosProfile.
    ///
    /// Null gives `Ok(None)`: the default QoS. A `struct_size` below the size field gives `Err`.
    /// A `struct_size` below the full size gives the default for every field that it does not
    /// cover. A `struct_size` above the full size is accepted and the extra bytes are not read.
    ///
    /// # Safety
    /// The pointer must be null, or point to at least `struct_size` readable bytes.
    pub unsafe fn to_qos_profile(ptr: *const CQosProfileV2) -> Result<Option<QosProfile>, InvalidStructSize> {
        if ptr.is_null() {
            return Ok(None);
        }
        let struct_size = unsafe { std::ptr::read_unaligned(ptr as *const u32) };
        if struct_size < Self::MIN_SIZE {
            return Err(InvalidStructSize(struct_size));
        }
        let mut full = Self::defaults();
        let covered = struct_size.min(Self::SIZE) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                ptr as *const u8,
                &mut full as *mut Self as *mut u8,
                covered,
            );
        }
        Ok(Some(full.as_v1().as_qos_profile()))
    }

    fn as_v1(&self) -> CQosProfile {
        CQosProfile {
            reliability: self.reliability,
            durability: self.durability,
            history: self.history,
            history_depth: self.history_depth,
            deadline_sec: self.deadline_sec,
            deadline_nsec: self.deadline_nsec,
            lifespan_sec: self.lifespan_sec,
            lifespan_nsec: self.lifespan_nsec,
            liveliness: self.liveliness,
            liveliness_lease_sec: self.liveliness_lease_sec,
            liveliness_lease_nsec: self.liveliness_lease_nsec,
        }
    }
}

/// Write the default QoS profile, and the size of the struct, to `out`
///
/// Call this before you change single fields. A zeroed struct has deadline, lifespan and lease
/// of zero, which is not the default of infinite.
///
/// Returns 0 on success and a negative `ErrorCode` if `out` is null.
///
/// # Safety
/// `out` must be null or point to writable memory of `sizeof(hiroz_qos_profile_v2_t)` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_qos_profile_v2_init(out: *mut CQosProfileV2) -> i32 {
    if out.is_null() {
        return super::ErrorCode::NullPointer as i32;
    }
    unsafe { out.write(CQosProfileV2::defaults()) };
    super::ErrorCode::Success as i32
}

/// Versioned C-compatible QoS of the five entities of an action
///
/// `struct_size` comes first and works as in `hiroz_qos_profile_v2_t`: the caller sets it to
/// `sizeof(hiroz_action_qos_v2_t)`, hiroz reads only the members inside it, and a member outside
/// it counts as null. A null member asks for the default profile. A `struct_size` below 4 is
/// invalid: the creator returns null.
///
/// Each member points to a `hiroz_qos_profile_v2_t` that must stay valid for the creator call only.
#[repr(C)]
pub struct CActionQosV2 {
    /// `sizeof(hiroz_action_qos_v2_t)` as the caller knows it
    pub struct_size: u32,
    /// The SendGoal service
    pub goal_service: *const CQosProfileV2,
    /// The GetResult service
    pub result_service: *const CQosProfileV2,
    /// The CancelGoal service
    pub cancel_service: *const CQosProfileV2,
    /// The feedback topic
    pub feedback_topic: *const CQosProfileV2,
    /// The status topic. An action client has no status subscriber and ignores it.
    pub status_topic: *const CQosProfileV2,
}

impl CActionQosV2 {
    /// The size of the struct in this build.
    const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// Convert a nullable pointer to the QoS of the five entities.
    ///
    /// Null gives the default for every entity. An invalid `struct_size`, here or in a member,
    /// gives `Err`.
    ///
    /// # Safety
    /// The pointer must be null, or point to at least `struct_size` readable bytes, and each
    /// non-null member inside that size must meet the safety rule of
    /// [`CQosProfileV2::to_qos_profile`].
    pub unsafe fn to_action_qos(ptr: *const CActionQosV2) -> Result<ActionQos, InvalidStructSize> {
        if ptr.is_null() {
            return Ok(ActionQos::default());
        }
        let struct_size = unsafe { std::ptr::read_unaligned(ptr as *const u32) };
        if struct_size < CQosProfileV2::MIN_SIZE {
            return Err(InvalidStructSize(struct_size));
        }
        let mut full = Self {
            struct_size: Self::SIZE,
            goal_service: std::ptr::null(),
            result_service: std::ptr::null(),
            cancel_service: std::ptr::null(),
            feedback_topic: std::ptr::null(),
            status_topic: std::ptr::null(),
        };
        let covered = struct_size.min(Self::SIZE) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                ptr as *const u8,
                &mut full as *mut Self as *mut u8,
                covered,
            );
            Ok(ActionQos {
                goal_service: CQosProfileV2::to_qos_profile(full.goal_service)?,
                result_service: CQosProfileV2::to_qos_profile(full.result_service)?,
                cancel_service: CQosProfileV2::to_qos_profile(full.cancel_service)?,
                feedback_topic: CQosProfileV2::to_qos_profile(full.feedback_topic)?,
                status_topic: CQosProfileV2::to_qos_profile(full.status_topic)?,
            })
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
