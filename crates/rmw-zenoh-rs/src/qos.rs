use crate::ros::*;

/// Normalize RMW QoS profile by resolving SYSTEM_DEFAULT (0) to concrete values.
/// This ensures get_actual_qos returns concrete policies, not SYSTEM_DEFAULT placeholders.
pub fn normalize_rmw_qos(qos: &rmw_qos_profile_t) -> rmw_qos_profile_t {
    let mut normalized = *qos;

    // Resolve SYSTEM_DEFAULT history to KEEP_LAST
    if normalized.history == rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_SYSTEM_DEFAULT {
        normalized.history = rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_LAST;
    }

    // Normalize KEEP_LAST with depth=0 to default depth (matches rmw_zenoh_cpp)
    if normalized.history == rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_LAST
        && normalized.depth == 0
    {
        normalized.depth = hiroz::qos::DEFAULT_HISTORY_DEPTH;
    }

    // Resolve SYSTEM_DEFAULT reliability to RELIABLE
    if normalized.reliability
        == rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_SYSTEM_DEFAULT
    {
        normalized.reliability = rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_RELIABLE;
    }

    // Resolve SYSTEM_DEFAULT durability to VOLATILE
    if normalized.durability == rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_SYSTEM_DEFAULT
    {
        normalized.durability = rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE;
    }

    // Resolve SYSTEM_DEFAULT liveliness to AUTOMATIC
    if normalized.liveliness == rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_SYSTEM_DEFAULT
    {
        normalized.liveliness = rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC;
    }

    normalized
}

/// Check if publisher and subscriber QoS profiles are compatible for matching
/// Returns true if they can be matched, false otherwise
///
/// Following rmw_zenoh_cpp's approach: Accept both OK and WARNING as compatible.
/// Only ERROR means incompatible. This matches Zenoh's lenient QoS philosophy.
pub fn qos_profiles_are_compatible(
    pub_qos: &rmw_qos_profile_t,
    sub_qos: &rmw_qos_profile_t,
) -> bool {
    let mut compatibility = rmw_qos_compatibility_type_t::RMW_QOS_COMPATIBILITY_OK;
    let ret = rmw_qos_profile_check_compatible(
        *pub_qos,
        *sub_qos,
        &mut compatibility as *mut _,
        std::ptr::null_mut(),
        0,
    );

    if ret != (RMW_RET_OK as rmw_ret_t) {
        return false;
    }

    // Accept both OK and WARNING as compatible (matches rmw_zenoh_cpp behavior)
    // Only ERROR means incompatible
    !matches!(
        compatibility,
        rmw_qos_compatibility_type_t::RMW_QOS_COMPATIBILITY_ERROR
    )
}

/// Check QoS compatibility and return the incompatible policy kind
/// Returns (compatible, policy_kind) where policy_kind is the first incompatible policy found
pub fn check_qos_compatibility_with_policy(
    pub_qos: &rmw_qos_profile_t,
    sub_qos: &rmw_qos_profile_t,
) -> (bool, rmw_qos_policy_kind_t) {
    use crate::ros::*;

    // Check reliability compatibility first
    // A RELIABLE subscriber cannot be matched with a BEST_EFFORT publisher
    if pub_qos.reliability == rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT
        && sub_qos.reliability == rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_RELIABLE
    {
        return (false, rmw_qos_policy_kind_e_RMW_QOS_POLICY_RELIABILITY);
    }

    // Check durability compatibility
    // A TRANSIENT_LOCAL subscriber cannot be matched with a VOLATILE publisher
    if pub_qos.durability == rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE
        && sub_qos.durability
            == rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL
    {
        return (false, rmw_qos_policy_kind_e_RMW_QOS_POLICY_DURABILITY);
    }

    // Check deadline compatibility
    // Publisher deadline must be <= subscriber deadline (if not infinite)
    let pub_deadline_ns =
        pub_qos.deadline.sec as i64 * 1_000_000_000 + pub_qos.deadline.nsec as i64;
    let sub_deadline_ns =
        sub_qos.deadline.sec as i64 * 1_000_000_000 + sub_qos.deadline.nsec as i64;
    // If subscriber has a deadline (not infinite/default) and publisher's is greater, incompatible
    if sub_deadline_ns > 0 && pub_deadline_ns > 0 && pub_deadline_ns > sub_deadline_ns {
        return (false, rmw_qos_policy_kind_e_RMW_QOS_POLICY_DEADLINE);
    }

    // Check liveliness policy compatibility
    // More strict policy (e.g., MANUAL_BY_TOPIC) from subscriber requires same or stricter from publisher
    if pub_qos.liveliness != sub_qos.liveliness {
        // AUTOMATIC (0) < MANUAL_BY_NODE (4) < MANUAL_BY_TOPIC (3)
        // Actually in ROS2: AUTOMATIC (1) is least strict, MANUAL_BY_TOPIC (0) is most strict
        // Subscriber's liveliness must be <= publisher's liveliness (more lenient or equal)
        if sub_qos.liveliness > pub_qos.liveliness {
            return (false, rmw_qos_policy_kind_e_RMW_QOS_POLICY_LIVELINESS);
        }
    }

    // Check liveliness lease duration compatibility
    // Publisher lease must be <= subscriber lease (if not infinite)
    let pub_lease_ns = pub_qos.liveliness_lease_duration.sec as i64 * 1_000_000_000
        + pub_qos.liveliness_lease_duration.nsec as i64;
    let sub_lease_ns = sub_qos.liveliness_lease_duration.sec as i64 * 1_000_000_000
        + sub_qos.liveliness_lease_duration.nsec as i64;
    if sub_lease_ns > 0 && pub_lease_ns > 0 && pub_lease_ns > sub_lease_ns {
        // Report as LIVELINESS policy, not LIVELINESS_LEASE_DURATION
        // This matches DDS/RMW convention where lease duration is part of liveliness policy
        return (false, rmw_qos_policy_kind_e_RMW_QOS_POLICY_LIVELINESS);
    }

    // All checks passed
    (true, 0)
}

/// Convert hiroz Duration to rmw_time_t.
/// hiroz uses {0, 0} for "infinite/no limit", but RMW uses RMW_DURATION_INFINITE.
/// {0, 0} in RMW means RMW_DURATION_UNSPECIFIED which displays as "0 nanoseconds".
fn duration_to_rmw_time(dur: &hiroz::qos::QosDuration) -> rmw_time_t {
    if dur.sec == 0 && dur.nsec == 0 {
        // RMW_DURATION_INFINITE = {9223372036, 854775807}
        rmw_time_t {
            sec: 9223372036,
            nsec: 854775807,
        }
    } else {
        rmw_time_t {
            sec: dur.sec,
            nsec: dur.nsec,
        }
    }
}

/// Convert rmw_time_t to hiroz Duration.
/// RMW_DURATION_INFINITE maps back to hiroz {0, 0} (infinite).
fn rmw_time_to_duration(time: &rmw_time_t) -> hiroz::qos::QosDuration {
    if time.sec == 9223372036 && time.nsec == 854775807 {
        hiroz::qos::QosDuration { sec: 0, nsec: 0 }
    } else {
        hiroz::qos::QosDuration {
            sec: time.sec,
            nsec: time.nsec,
        }
    }
}

/// Convert hiroz QoS profile to RMW QoS profile
pub fn hiroz_qos_to_rmw_qos(qos: &hiroz::qos::QosProfile) -> rmw_qos_profile_t {
    use hiroz::qos::*;

    #[allow(non_upper_case_globals)]
    let (history, depth) = match &qos.history {
        QosHistory::KeepLast(n) => (
            rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_LAST,
            n.get(),
        ),
        QosHistory::KeepAll => (rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_ALL, 0),
    };

    #[allow(non_upper_case_globals)]
    let reliability = match qos.reliability {
        QosReliability::Reliable => {
            rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_RELIABLE
        }
        QosReliability::BestEffort => {
            rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT
        }
        _ => rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_RELIABLE,
    };

    #[allow(non_upper_case_globals)]
    let durability = match qos.durability {
        QosDurability::TransientLocal => {
            rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL
        }
        QosDurability::Volatile => rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE,
        _ => rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE,
    };

    #[allow(non_upper_case_globals)]
    let liveliness = match qos.liveliness {
        QosLiveliness::Automatic => rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC,
        // MANUAL_BY_NODE has been deprecated in ROS 2 and removed from RMW.
        // Map it to MANUAL_BY_TOPIC as recommended by the deprecation message.
        QosLiveliness::ManualByNode => {
            rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_MANUAL_BY_TOPIC
        }
        QosLiveliness::ManualByTopic => {
            rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_MANUAL_BY_TOPIC
        }
        _ => rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC,
    };

    // Convert duration: hiroz uses {0, 0} for "infinite/no limit",
    // but RMW uses {9223372036, 854775807} (RMW_DURATION_INFINITE).
    // {0, 0} in RMW means RMW_DURATION_UNSPECIFIED which displays as "0 nanoseconds".
    let deadline = duration_to_rmw_time(&qos.deadline);
    let lifespan = duration_to_rmw_time(&qos.lifespan);
    let liveliness_lease_duration = duration_to_rmw_time(&qos.liveliness_lease_duration);

    rmw_qos_profile_t {
        history,
        depth,
        reliability,
        durability,
        deadline,
        lifespan,
        liveliness,
        liveliness_lease_duration,
        avoid_ros_namespace_conventions: false,
    }
}

/// Convert RMW QoS profile to hiroz QoS profile
pub fn rmw_qos_to_hiroz_qos(qos: &rmw_qos_profile_t) -> hiroz::qos::QosProfile {
    use hiroz::qos::*;

    #[allow(non_upper_case_globals)]
    let history = match qos.history {
        rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_LAST => {
            // Use hiroz normalization function to handle depth=0 (SYSTEM_DEFAULT)
            QosHistory::from_depth(qos.depth)
        }
        rmw_qos_history_policy_e_RMW_QOS_POLICY_HISTORY_KEEP_ALL => QosHistory::KeepAll,
        _ => QosHistory::default(), // Default
    };

    #[allow(non_upper_case_globals)]
    let reliability = match qos.reliability {
        rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_RELIABLE => {
            QosReliability::Reliable
        }
        rmw_qos_reliability_policy_e_RMW_QOS_POLICY_RELIABILITY_BEST_EFFORT => {
            QosReliability::BestEffort
        }
        _ => QosReliability::Reliable, // Default
    };

    #[allow(non_upper_case_globals)]
    let durability = match qos.durability {
        rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL => {
            QosDurability::TransientLocal
        }
        rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE => QosDurability::Volatile,
        _ => QosDurability::Volatile, // Default
    };

    #[allow(non_upper_case_globals)]
    let liveliness = match qos.liveliness {
        rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC => QosLiveliness::Automatic,
        rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_MANUAL_BY_TOPIC => {
            QosLiveliness::ManualByTopic
        }
        _ => QosLiveliness::Automatic, // Default
    };

    QosProfile {
        history,
        reliability,
        durability,
        deadline: rmw_time_to_duration(&qos.deadline),
        lifespan: rmw_time_to_duration(&qos.lifespan),
        liveliness,
        liveliness_lease_duration: rmw_time_to_duration(&qos.liveliness_lease_duration),
    }
}

// RMW QoS Functions
// Following rmw_zenoh_cpp's approach: Zenoh handles QoS internally, so we're very lenient.
// Only warn about TRANSIENT_LOCAL publisher + VOLATILE subscriber edge case.
#[unsafe(no_mangle)]
pub extern "C" fn rmw_qos_profile_check_compatible(
    publisher_profile: rmw_qos_profile_t,
    subscription_profile: rmw_qos_profile_t,
    compatibility: *mut rmw_qos_compatibility_type_t,
    reason: *mut ::std::os::raw::c_char,
    reason_size: usize,
) -> rmw_ret_t {
    if compatibility.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }
    if reason.is_null() && reason_size > 0 {
        return RMW_RET_INVALID_ARGUMENT as _;
    }
    if !reason.is_null() && reason_size == 0 {
        return RMW_RET_INVALID_ARGUMENT as _;
    }
    // Initialize reason buffer
    if !reason.is_null() && reason_size > 0 {
        unsafe {
            *reason = 0;
        }
    }

    // In Zenoh, there are no QoS incompatibilities.
    // The one scenario where transport may not occur is where a publisher with
    // TRANSIENT_LOCAL durability publishes a message before a subscription with
    // VOLATILE durability spins up. However, any subsequent messages published
    // will be received by the subscription.
    if publisher_profile.durability
        == rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_TRANSIENT_LOCAL
        && subscription_profile.durability
            == rmw_qos_durability_policy_e_RMW_QOS_POLICY_DURABILITY_VOLATILE
    {
        unsafe {
            *compatibility = rmw_qos_compatibility_type_t::RMW_QOS_COMPATIBILITY_WARNING;
        }
        return RMW_RET_OK as _;
    }

    unsafe {
        *compatibility = rmw_qos_compatibility_type_t::RMW_QOS_COMPATIBILITY_OK;
    }
    RMW_RET_OK as _
}
