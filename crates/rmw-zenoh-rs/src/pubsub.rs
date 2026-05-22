use std::ffi::CString;

use crate::rmw_impl_has_data_ptr;
use crate::ros::*;
use crate::traits::{BorrowData, Waitable};
use zenoh::{Result, sample::Sample};

// Helper function to convert protocol QoS to hiroz QoS
pub fn protocol_qos_to_hiroz_qos(qos: &hiroz_protocol::qos::QosProfile) -> hiroz::qos::QosProfile {
    hiroz::qos::QosProfile {
        reliability: match qos.reliability {
            hiroz_protocol::qos::QosReliability::Reliable => hiroz::qos::QosReliability::Reliable,
            hiroz_protocol::qos::QosReliability::BestEffort => {
                hiroz::qos::QosReliability::BestEffort
            }
        },
        durability: match qos.durability {
            hiroz_protocol::qos::QosDurability::TransientLocal => {
                hiroz::qos::QosDurability::TransientLocal
            }
            hiroz_protocol::qos::QosDurability::Volatile => hiroz::qos::QosDurability::Volatile,
        },
        history: match qos.history {
            hiroz_protocol::qos::QosHistory::KeepLast(depth) => {
                hiroz::qos::QosHistory::from_depth(depth)
            }
            hiroz_protocol::qos::QosHistory::KeepAll => hiroz::qos::QosHistory::KeepAll,
        },
        ..Default::default()
    }
}

/// Publisher implementation for RMW
pub struct PublisherImpl {
    pub inner: hiroz::pubsub::ZPub<crate::msg::RosMessage, crate::msg::RosSerdes>,
    pub ts: crate::type_support::MessageTypeSupport,
    pub topic: CString,
    pub options: rmw_publisher_options_t,
    pub qos: rmw_qos_profile_t,
    pub graph: std::sync::Arc<hiroz::graph::Graph>,
    pub entity: hiroz::entity::EndpointEntity,
}

impl PublisherImpl {
    pub fn publish(&self, msg: *const ::std::os::raw::c_void) -> Result<()> {
        let ros_msg = crate::msg::RosMessage::new(msg as *const crate::c_void, self.ts);
        self.inner.publish(&ros_msg)
    }

    pub fn publish_serialized_message(&self, msg: &[u8]) -> Result<()> {
        self.inner.publish_serialized(msg)
    }
}

/// Subscription implementation for RMW
pub struct SubscriptionImpl {
    pub inner: hiroz::pubsub::ZSub<crate::msg::RosMessage, Sample, crate::msg::RosSerdes>,
    pub ts: crate::type_support::MessageTypeSupport,
    pub topic: CString,
    pub options: rmw_subscription_options_t,
    pub qos: rmw_qos_profile_t,
    pub callback:
        std::sync::Arc<std::sync::Mutex<crate::ros::rmw_subscription_new_message_callback_t>>,
    pub callback_user_data: std::sync::Arc<std::sync::Mutex<usize>>, // Store pointer as usize for thread safety
    pub unread_count: std::sync::Arc<std::sync::Mutex<usize>>, // Track messages arrived before callback was set
    pub graph: std::sync::Arc<hiroz::graph::Graph>,
    pub entity: hiroz::entity::EndpointEntity,
    pub notifier: std::sync::Arc<crate::utils::Notifier>,
    /// Per-subscriber reception counter. Incremented on every successful take.
    pub reception_sn: std::sync::atomic::AtomicU64,
}

impl SubscriptionImpl {
    pub fn take(&self, ros_message: *mut std::os::raw::c_void, taken: *mut bool) -> Result<()> {
        unsafe {
            *taken = false;
        }
        let queue = self.inner.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;

        if let Some(sample) = queue.try_recv() {
            // Deserialize the sample payload into ros_message using ts
            // Assume the payload is CDR serialized
            let payload = sample.payload();
            let bytes = payload.to_bytes().to_vec();
            unsafe { self.ts.deserialize_message(&bytes, ros_message as *mut _) };
            unsafe {
                *taken = true;
            }
        }
        Ok(())
    }

    pub fn take_with_info(
        &self,
        ros_message: *mut std::os::raw::c_void,
        message_info: *mut rmw_message_info_t,
        taken: *mut bool,
    ) -> Result<()> {
        unsafe {
            *taken = false;
        }
        let queue = self.inner.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        if let Some(sample) = queue.try_recv() {
            // Deserialize the sample payload into ros_message using ts
            let payload = sample.payload();
            let bytes = payload.to_bytes().to_vec();
            unsafe { self.ts.deserialize_message(&bytes, ros_message as *mut _) };

            // Fill in message_info
            if !message_info.is_null() {
                unsafe {
                    // Extract fields from attachment; fall back gracefully if absent.
                    let (source_timestamp, pub_sn, gid) = if let Some(attachment_bytes) =
                        sample.attachment()
                    {
                        if let Ok(att) = hiroz::attachment::Attachment::try_from(attachment_bytes) {
                            (
                                att.source_timestamp,
                                att.sequence_number as u64,
                                att.source_gid,
                            )
                        } else {
                            (0, 0, [0u8; 16])
                        }
                    } else {
                        (0, 0, [0u8; 16])
                    };

                    let received_timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as i64;

                    (*message_info).source_timestamp = source_timestamp;
                    (*message_info).received_timestamp = received_timestamp;
                    (*message_info).publication_sequence_number = pub_sn;
                    (*message_info).reception_sequence_number = self
                        .reception_sn
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (*message_info).publisher_gid.data = gid;
                    (*message_info).publisher_gid.implementation_identifier =
                        crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _;
                    (*message_info).from_intra_process = false;
                }
            }

            unsafe {
                *taken = true;
            }
        }
        Ok(())
    }

    pub fn take_serialized(
        &self,
        serialized_message: *mut rcl_serialized_message_t,
        message_info: *mut rmw_message_info_t,
        taken: *mut bool,
    ) -> Result<()> {
        unsafe {
            *taken = false;
        }
        let queue = self.inner.queue.as_ref().ok_or_else(|| {
            zenoh::Error::from("Subscriber was built with callback, no queue available")
        })?;
        if let Some(sample) = queue.try_recv() {
            let payload = sample.payload();
            let bytes = payload.to_bytes();

            unsafe {
                // Check if there's enough capacity
                if (*serialized_message).buffer_capacity < bytes.len() {
                    // Reallocate buffer if needed
                    if !(*serialized_message).buffer.is_null() {
                        // TODO: Use proper allocator from RMW context
                        let _ = Vec::from_raw_parts(
                            (*serialized_message).buffer,
                            (*serialized_message).buffer_length,
                            (*serialized_message).buffer_capacity,
                        );
                    }
                    let mut new_buffer = vec![0u8; bytes.len()];
                    (*serialized_message).buffer = new_buffer.as_mut_ptr();
                    (*serialized_message).buffer_capacity = new_buffer.len();
                    std::mem::forget(new_buffer);
                }

                // Copy bytes to buffer
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (*serialized_message).buffer,
                    bytes.len(),
                );
                (*serialized_message).buffer_length = bytes.len();
            }

            // Fill in message_info if provided
            if !message_info.is_null() {
                unsafe {
                    let (source_timestamp, pub_sn, gid) = if let Some(attachment_bytes) =
                        sample.attachment()
                    {
                        if let Ok(att) = hiroz::attachment::Attachment::try_from(attachment_bytes) {
                            (
                                att.source_timestamp,
                                att.sequence_number as u64,
                                att.source_gid,
                            )
                        } else {
                            (0, 0, [0u8; 16])
                        }
                    } else {
                        (0, 0, [0u8; 16])
                    };

                    let received_timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as i64;

                    (*message_info).source_timestamp = source_timestamp;
                    (*message_info).received_timestamp = received_timestamp;
                    (*message_info).publication_sequence_number = pub_sn;
                    (*message_info).reception_sequence_number = self
                        .reception_sn
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (*message_info).publisher_gid.data = gid;
                    (*message_info).publisher_gid.implementation_identifier =
                        crate::RMW_ZENOH_IDENTIFIER.as_ptr() as *const _;
                    (*message_info).from_intra_process = false;
                }
            }

            unsafe {
                *taken = true;
            }
        }
        Ok(())
    }
}

impl Waitable for SubscriptionImpl {
    fn is_ready(&self) -> bool {
        if let Some(queue) = self.inner.queue.as_ref() {
            !queue.is_empty()
        } else {
            false
        }
    }
}

rmw_impl_has_data_ptr!(rmw_publisher_t, rmw_publisher_impl_t, PublisherImpl);
rmw_impl_has_data_ptr!(
    rmw_subscription_t,
    rmw_subscription_impl_t,
    SubscriptionImpl
);

// RMW Publisher Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_publish_serialized_message(
    publisher: *const rmw_publisher_t,
    serialized_message: *const rcl_serialized_message_t,
    _allocation: *mut rmw_publisher_allocation_t,
) -> rmw_ret_t {
    if publisher.is_null() || serialized_message.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*publisher).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let publisher_impl = match publisher.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    let msg_slice = unsafe {
        std::slice::from_raw_parts(
            (*serialized_message).buffer,
            (*serialized_message).buffer_length,
        )
    };

    match publisher_impl.publish_serialized_message(msg_slice) {
        Ok(_) => RMW_RET_OK as _,
        Err(e) => {
            tracing::error!("Failed to publish serialized message: {}", e);
            RMW_RET_ERROR as _
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publish_loaned_message(
    _publisher: *const rmw_publisher_t,
    _ros_message: *mut ::std::os::raw::c_void,
    _allocation: *mut rmw_publisher_allocation_t,
) -> rmw_ret_t {
    RMW_RET_UNSUPPORTED as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publisher_count_matched_subscriptions(
    publisher: *const rmw_publisher_t,
    subscription_count: *mut usize,
) -> rmw_ret_t {
    if publisher.is_null() || subscription_count.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*publisher).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let publisher_impl = match publisher.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Get all subscription entities for this topic
    let topic_name = publisher_impl.topic.to_str().unwrap_or("");
    let entities = publisher_impl
        .graph
        .get_entities_by_topic(hiroz::entity::EndpointKind::Subscription, topic_name);

    // Filter by QoS compatibility
    let pub_qos = &publisher_impl.qos;
    let count = entities
        .iter()
        .filter(|entity| {
            if let Some(endpoint) = hiroz::entity::entity_get_endpoint(entity) {
                let sub_qos =
                    crate::qos::hiroz_qos_to_rmw_qos(&protocol_qos_to_hiroz_qos(&endpoint.qos));
                crate::qos::qos_profiles_are_compatible(pub_qos, &sub_qos)
            } else {
                false
            }
        })
        .count();

    unsafe {
        *subscription_count = count;
    }
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publisher_get_actual_qos(
    publisher: *const rmw_publisher_t,
    qos: *mut rmw_qos_profile_t,
) -> rmw_ret_t {
    if publisher.is_null() || qos.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*publisher).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let publisher_impl = match publisher.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Return the original QoS profile with unspecified durations converted to infinite
    // Zenoh doesn't negotiate QoS like DDS, so the "actual" QoS is the requested QoS
    let mut actual_qos = publisher_impl.qos;

    // Convert zero/unspecified durations to RMW_DURATION_INFINITE (i32::MAX seconds)
    // This matches the expected behavior from ROS 2 tests
    const RMW_DURATION_INFINITE_SEC: u64 = i32::MAX as u64; // 2147483647

    if actual_qos.deadline.sec == 0 && actual_qos.deadline.nsec == 0 {
        actual_qos.deadline.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.deadline.nsec = 0;
    }
    if actual_qos.lifespan.sec == 0 && actual_qos.lifespan.nsec == 0 {
        actual_qos.lifespan.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.lifespan.nsec = 0;
    }
    if actual_qos.liveliness_lease_duration.sec == 0
        && actual_qos.liveliness_lease_duration.nsec == 0
    {
        actual_qos.liveliness_lease_duration.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.liveliness_lease_duration.nsec = 0;
    }

    // Convert liveliness SYSTEM_DEFAULT (0) or UNKNOWN to AUTOMATIC (1)
    if actual_qos.liveliness == rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_SYSTEM_DEFAULT
        || actual_qos.liveliness == rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_UNKNOWN
    {
        actual_qos.liveliness = rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC;
    }

    unsafe {
        *qos = actual_qos;
    }
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publisher_assert_liveliness(publisher: *const rmw_publisher_t) -> rmw_ret_t {
    if publisher.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }
    // Assume liveliness is valid
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publisher_wait_for_all_acked(
    publisher: *const rmw_publisher_t,
    _wait_timeout: rmw_time_t,
) -> rmw_ret_t {
    if publisher.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }
    // Not tracking published data, return OK
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_publisher_get_network_flow_endpoints(
    _publisher: *const rmw_publisher_t,
    _allocator: *const rcl_allocator_t,
    _endpoints: *mut rmw_network_flow_endpoint_array_t,
) -> rmw_ret_t {
    RMW_RET_UNSUPPORTED as _
}

// RMW Subscription Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_with_info(
    subscription: *const rmw_subscription_t,
    ros_message: *mut ::std::os::raw::c_void,
    taken: *mut bool,
    message_info: *mut rmw_message_info_t,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    if subscription.is_null() || ros_message.is_null() || taken.is_null() || message_info.is_null()
    {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match subscription_impl.take_with_info(ros_message, message_info, taken) {
        Ok(_) => RMW_RET_OK as _,
        Err(_) => RMW_RET_ERROR as _,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_sequence(
    subscription: *const rmw_subscription_t,
    count: usize,
    message_sequence: *mut rmw_message_sequence_t,
    message_info_sequence: *mut rmw_message_info_sequence_t,
    taken: *mut usize,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    if subscription.is_null()
        || message_sequence.is_null()
        || message_info_sequence.is_null()
        || taken.is_null()
    {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    if count == 0 {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        if count > (*message_sequence).capacity || count > (*message_info_sequence).capacity {
            return RMW_RET_INVALID_ARGUMENT as _;
        }

        *taken = 0;
        while *taken < count {
            let mut one_taken = false;
            let msg_ptr = *(*message_sequence).data.add(*taken);
            let info_ptr =
                (*message_info_sequence).data.add(*taken) as *mut crate::ros::rmw_message_info_t;

            match subscription_impl.take_with_info(msg_ptr, info_ptr, &mut one_taken) {
                Ok(_) => {
                    if !one_taken {
                        break;
                    }
                    *taken += 1;
                }
                Err(_) => break,
            }
        }

        (*message_sequence).size = *taken;
        (*message_info_sequence).size = *taken;
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_serialized_message(
    subscription: *const rmw_subscription_t,
    serialized_message: *mut rcl_serialized_message_t,
    taken: *mut bool,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    if subscription.is_null() || serialized_message.is_null() || taken.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match subscription_impl.take_serialized(serialized_message, std::ptr::null_mut(), taken) {
        Ok(_) => RMW_RET_OK as _,
        Err(_) => RMW_RET_ERROR as _,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_serialized_message_with_info(
    subscription: *const rmw_subscription_t,
    serialized_message: *mut rcl_serialized_message_t,
    taken: *mut bool,
    message_info: *mut rmw_message_info_t,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    if subscription.is_null()
        || serialized_message.is_null()
        || taken.is_null()
        || message_info.is_null()
    {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match subscription_impl.take_serialized(serialized_message, message_info, taken) {
        Ok(_) => RMW_RET_OK as _,
        Err(_) => RMW_RET_ERROR as _,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_loaned_message(
    _subscription: *const rmw_subscription_t,
    _loaned_message: *mut *mut ::std::os::raw::c_void,
    _taken: *mut bool,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    // Loaned messages are not currently supported in this implementation
    // Return RMW_RET_UNSUPPORTED to match the behavior of rmw_zenoh_cpp
    RMW_RET_UNSUPPORTED as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_loaned_message_with_info(
    _subscription: *const rmw_subscription_t,
    _loaned_message: *mut *mut ::std::os::raw::c_void,
    _taken: *mut bool,
    _message_info: *mut rmw_message_info_t,
    _allocation: *mut rmw_subscription_allocation_t,
) -> rmw_ret_t {
    // Loaned messages are not currently supported in this implementation
    // Return RMW_RET_UNSUPPORTED to match the behavior of rmw_zenoh_cpp
    RMW_RET_UNSUPPORTED as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_subscription_count_matched_publishers(
    subscription: *const rmw_subscription_t,
    publisher_count: *mut usize,
) -> rmw_ret_t {
    if subscription.is_null() || publisher_count.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Get all publisher entities for this topic
    let topic_name = subscription_impl.topic.to_str().unwrap_or("");
    let entities = subscription_impl
        .graph
        .get_entities_by_topic(hiroz::entity::EndpointKind::Publisher, topic_name);

    // Filter by QoS compatibility
    let sub_qos = &subscription_impl.qos;
    let count = entities
        .iter()
        .filter(|entity| {
            if let Some(endpoint) = hiroz::entity::entity_get_endpoint(entity) {
                let pub_qos =
                    crate::qos::hiroz_qos_to_rmw_qos(&protocol_qos_to_hiroz_qos(&endpoint.qos));
                crate::qos::qos_profiles_are_compatible(&pub_qos, sub_qos)
            } else {
                false
            }
        })
        .count();

    unsafe {
        *publisher_count = count;
    }
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_subscription_get_actual_qos(
    subscription: *const rmw_subscription_t,
    qos: *mut rmw_qos_profile_t,
) -> rmw_ret_t {
    if subscription.is_null() || qos.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*subscription).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let subscription_impl = match subscription.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    // Return the original QoS profile with unspecified durations converted to infinite
    // Zenoh doesn't negotiate QoS like DDS, so the "actual" QoS is the requested QoS
    let mut actual_qos = subscription_impl.qos;

    // Convert zero/unspecified durations to RMW_DURATION_INFINITE (i32::MAX seconds)
    // This matches the expected behavior from ROS 2 tests
    const RMW_DURATION_INFINITE_SEC: u64 = i32::MAX as u64; // 2147483647

    if actual_qos.deadline.sec == 0 && actual_qos.deadline.nsec == 0 {
        actual_qos.deadline.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.deadline.nsec = 0;
    }
    if actual_qos.lifespan.sec == 0 && actual_qos.lifespan.nsec == 0 {
        actual_qos.lifespan.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.lifespan.nsec = 0;
    }
    if actual_qos.liveliness_lease_duration.sec == 0
        && actual_qos.liveliness_lease_duration.nsec == 0
    {
        actual_qos.liveliness_lease_duration.sec = RMW_DURATION_INFINITE_SEC;
        actual_qos.liveliness_lease_duration.nsec = 0;
    }

    // Convert liveliness SYSTEM_DEFAULT (0) or UNKNOWN to AUTOMATIC (1)
    if actual_qos.liveliness == rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_SYSTEM_DEFAULT
        || actual_qos.liveliness == rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_UNKNOWN
    {
        actual_qos.liveliness = rmw_qos_liveliness_policy_e_RMW_QOS_POLICY_LIVELINESS_AUTOMATIC;
    }

    unsafe {
        *qos = actual_qos;
    }
    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_subscription_set_content_filter(
    _subscription: *const rmw_subscription_t,
    _content_filter: *const rmw_subscription_content_filter_options_t,
) -> rmw_ret_t {
    // Content filtering is not supported yet
    RMW_RET_UNSUPPORTED as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_subscription_get_content_filter(
    _subscription: *const rmw_subscription_t,
    _allocator: *const rcl_allocator_t,
    _content_filter: *mut rmw_subscription_content_filter_options_t,
) -> rmw_ret_t {
    // Content filtering is not supported yet
    RMW_RET_UNSUPPORTED as _
}
