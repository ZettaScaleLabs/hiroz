use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Mutex;

use crate::c_void;
use crate::rmw_impl_has_data_ptr;
use crate::ros::*;
use crate::traits::{BorrowData, OwnData, Waitable};
use zenoh::Result;

/// Client implementation for RMW
pub struct ClientImpl {
    pub inner: hiroz::service::ZClient<crate::msg::RosService>,
    pub service_name: CString,
    pub options: rmw_client_options_t,
    pub request_ts: crate::type_support::ServiceTypeSupport,
    pub response_ts: crate::type_support::ServiceTypeSupport,
    pub callback: std::sync::Arc<Mutex<rmw_client_new_response_callback_t>>,
    pub callback_user_data: std::sync::Arc<Mutex<usize>>,
    pub notifier: std::sync::Arc<crate::utils::Notifier>,
    /// Tracks responses that arrived while no callback was set
    pub unread_count: std::sync::Arc<Mutex<usize>>,
    pub graph: std::sync::Arc<hiroz::graph::Graph>,
    pub entity: hiroz::entity::EndpointEntity,
}

impl ClientImpl {
    pub fn send_request(&self, request: *const c_void, sequence_id: *mut i64) -> Result<()> {
        let req = crate::msg::RosMessage::new(request, self.request_ts.request);

        let notifier = self.notifier.clone();
        let callback_holder = self.callback.clone();
        let user_data_holder = self.callback_user_data.clone();
        let unread_count_holder = self.unread_count.clone();
        let notify_callback = move || {
            notifier.notify_all();
            if let Ok(cb) = callback_holder.lock() {
                if let Some(callback_fn) = *cb {
                    if let Ok(user_data_usize) = user_data_holder.lock() {
                        unsafe {
                            let user_data_ptr = *user_data_usize as *const std::ffi::c_void;
                            callback_fn(user_data_ptr, 1);
                        }
                    }
                } else if let Ok(mut unread) = unread_count_holder.lock() {
                    *unread += 1;
                }
            }
        };

        // rmw_send_request returns the sequence number stamped into the attachment,
        // which is the same value the server will echo back. Use it directly as the
        // rcl sequence ID so correlation is exact with no offset.
        let sn = self.inner.rmw_send_request(&req, notify_callback)?;

        unsafe {
            *sequence_id = sn;
        }

        tracing::debug!(
            "[ClientImpl::send_request] Request sent successfully, returned sn: {}",
            sn
        );
        Ok(())
    }

    pub fn take_response(
        &self,
        request_header: *mut rmw_service_info_t,
        response: *mut c_void,
        taken: *mut bool,
    ) -> Result<()> {
        unsafe {
            *taken = false;
        }

        tracing::debug!(
            "[ClientImpl::take_response] Attempting to take response, rx has {} items",
            if self.inner.rmw_has_responses() { 1 } else { 0 }
        );

        // Try to receive a response
        if let Some(sample) = self.inner.rmw_try_take_response_sample()? {
            tracing::debug!("[ClientImpl::take_response] Got response sample");

            let payload = sample.payload();
            let bytes = payload.to_bytes().to_vec();

            // Deserialize response using response MessageTypeSupport
            unsafe {
                self.response_ts
                    .response
                    .deserialize_message(&bytes, response as *mut _);
            }

            // Fill request_header
            if !request_header.is_null() {
                // Extract sequence number and GID from attachment if available
                let (sn, gid, source_timestamp) = if let Some(attachment_bytes) =
                    sample.attachment()
                {
                    match hiroz::attachment::Attachment::try_from(attachment_bytes) {
                        Ok(attachment) => {
                            tracing::debug!(
                                "[ClientImpl::take_response] Extracted attachment: sn={}, gid={:?}",
                                attachment.sequence_number,
                                attachment.source_gid
                            );
                            (
                                attachment.sequence_number,
                                attachment.source_gid,
                                attachment.source_timestamp,
                            )
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[ClientImpl::take_response] Failed to extract attachment: {}",
                                e
                            );
                            (0, [0u8; 16], 0)
                        }
                    }
                } else {
                    tracing::warn!("[ClientImpl::take_response] No attachment in response");
                    (0, [0u8; 16], 0)
                };

                // Set received_timestamp to current time
                let received_timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |v| v.as_nanos() as i64);

                unsafe {
                    (*request_header).request_id.sequence_number = sn;
                    (*request_header)
                        .request_id
                        .writer_guid
                        .copy_from_slice(&gid);
                    (*request_header).source_timestamp = source_timestamp;
                    (*request_header).received_timestamp = received_timestamp;
                }
            }

            unsafe {
                *taken = true;
            }
            tracing::debug!("[ClientImpl::take_response] Response taken successfully");
        } else {
            tracing::debug!("[ClientImpl::take_response] No response available in rx channel");
        }
        Ok(())
    }
}

/// Service implementation for RMW
pub struct ServiceImpl {
    pub inner: hiroz::service::ZServer<crate::msg::RosService>,
    pub pending:
        HashMap<hiroz::service::RequestId, hiroz::service::ServiceReply<crate::msg::RosService>>,
    pub service_name: CString,
    pub request_ts: crate::type_support::ServiceTypeSupport,
    pub response_ts: crate::type_support::ServiceTypeSupport,
    pub qos: rmw_qos_profile_t,
    pub callback: std::sync::Arc<Mutex<rmw_service_new_request_callback_t>>,
    pub callback_user_data: std::sync::Arc<Mutex<usize>>,
    /// Tracks requests that arrived while no callback was set
    pub unread_count: std::sync::Arc<Mutex<usize>>,
    pub graph: std::sync::Arc<hiroz::graph::Graph>,
    pub entity: hiroz::entity::EndpointEntity,
}

impl ServiceImpl {
    pub fn take_request(
        &mut self,
        request_header: *mut rmw_service_info_t,
        request: *mut c_void,
        taken: *mut bool,
    ) -> Result<()> {
        unsafe {
            *taken = false;
        }

        if let Some((bytes, reply)) = self.inner.try_take_request_raw()? {
            let request_id = reply.id().clone();
            let source_timestamp = request_id.source_timestamp;

            // Set received_timestamp to current time
            let received_timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |v| v.as_nanos() as i64);

            // Store the reply context for later response
            tracing::debug!(
                "[ServiceImpl::take_request] Storing reply context with sn:{}",
                request_id.sequence_number
            );
            self.pending.insert(request_id.clone(), reply);

            // Deserialize into the provided request buffer using request MessageTypeSupport
            unsafe {
                self.request_ts
                    .request
                    .deserialize_message(&bytes, request as *mut _);
            }

            // Fill request_header with sequence info and timestamps
            if !request_header.is_null() {
                unsafe {
                    (*request_header).request_id.sequence_number = request_id.sequence_number;
                    for (i, &byte) in request_id.writer_guid.iter().enumerate() {
                        if i < 16 {
                            (*request_header).request_id.writer_guid[i] = byte;
                        }
                    }
                    (*request_header).source_timestamp = source_timestamp;
                    (*request_header).received_timestamp = received_timestamp;
                }
            }

            unsafe {
                *taken = true;
            }
        }
        Ok(())
    }

    pub fn send_response(
        &mut self,
        request_header: *const rmw_request_id_t,
        response: *const c_void,
    ) -> Result<()> {
        let request_id = unsafe {
            let mut gid = [0u8; 16];
            gid.copy_from_slice(&(*request_header).writer_guid);
            hiroz::service::RequestId {
                writer_guid: gid,
                sequence_number: (*request_header).sequence_number,
                source_timestamp: 0,
            }
        };

        tracing::debug!(
            "[ServiceImpl::send_response] Sending response for key sn:{}, gid:{:?}",
            request_id.sequence_number,
            request_id.writer_guid
        );

        // Create RosMessage Response from the raw pointer using response MessageTypeSupport
        let resp = crate::msg::RosMessage::new(response, self.response_ts.response);

        match self.pending.remove(&request_id) {
            Some(reply) => match reply.reply_blocking(&resp) {
                Ok(_) => {
                    tracing::debug!("[ServiceImpl::send_response] Response sent successfully");
                    Ok(())
                }
                Err(e) => {
                    tracing::error!(
                        "[ServiceImpl::send_response] Failed to send response: {}",
                        e
                    );
                    Err(e)
                }
            },
            None => Err(zenoh::Error::from("Pending request not found")),
        }
    }
}

impl Waitable for ClientImpl {
    fn is_ready(&self) -> bool {
        // Acquire fence to ensure we see the latest channel state from other threads
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        self.inner.rmw_has_responses()
    }
}

impl Waitable for ServiceImpl {
    fn is_ready(&self) -> bool {
        // Acquire fence to ensure we see the latest channel state from other threads
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        self.inner.try_queue().is_some_and(|q| !q.is_empty())
    }
}

rmw_impl_has_data_ptr!(rmw_client_t, rmw_client_impl_t, ClientImpl);
rmw_impl_has_data_ptr!(rmw_service_t, rmw_service_impl_t, ServiceImpl);

// RMW Service Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_request(
    service: *const rmw_service_t,
    request_header: *mut rmw_service_info_t,
    ros_request: *mut c_void,
    taken: *mut bool,
) -> rmw_ret_t {
    if service.is_null() || request_header.is_null() || ros_request.is_null() || taken.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*service).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let service_impl = match (service as *mut rmw_service_t).borrow_mut_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match service_impl.take_request(request_header, ros_request, taken) {
        Ok(_) => RMW_RET_OK as _,
        Err(e) => {
            tracing::error!("Failed to take request: {}", e);
            RMW_RET_ERROR as _
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_send_response(
    service: *const rmw_service_t,
    response_header: *mut rmw_request_id_t,
    ros_response: *mut c_void,
) -> rmw_ret_t {
    if service.is_null() || response_header.is_null() || ros_response.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*service).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let service_impl = match (service as *mut rmw_service_t).borrow_mut_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match service_impl.send_response(response_header, ros_response) {
        Ok(_) => RMW_RET_OK as _,
        Err(e) => {
            tracing::error!("Failed to send response: {}", e);
            RMW_RET_ERROR as _
        }
    }
}

// RMW Client Functions
#[unsafe(no_mangle)]
pub extern "C" fn rmw_send_request(
    client: *const rmw_client_t,
    ros_request: *const c_void,
    sequence_id: *mut i64,
) -> rmw_ret_t {
    if client.is_null() || ros_request.is_null() || sequence_id.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*client).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let client_impl = match client.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match client_impl.send_request(ros_request, sequence_id) {
        Ok(_) => RMW_RET_OK as _,
        Err(e) => {
            tracing::error!("Failed to send request: {}", e);
            RMW_RET_ERROR as _
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_take_response(
    client: *const rmw_client_t,
    request_header: *mut rmw_service_info_t,
    ros_response: *mut c_void,
    taken: *mut bool,
) -> rmw_ret_t {
    if client.is_null() || request_header.is_null() || ros_response.is_null() || taken.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    unsafe {
        let ret = crate::context::check_impl_id_ret((*client).implementation_identifier);
        if ret != RMW_RET_OK as rmw_ret_t {
            return ret;
        }
    }

    let client_impl = match client.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    match client_impl.take_response(request_header, ros_response, taken) {
        Ok(_) => RMW_RET_OK as _,
        Err(e) => {
            tracing::error!("Failed to take response: {}", e);
            RMW_RET_ERROR as _
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_client_request_publisher_get_actual_qos(
    client: *const rmw_client_t,
    qos: *mut rmw_qos_profile_t,
) -> rmw_ret_t {
    if client.is_null() || qos.is_null() {
        return RMW_RET_INVALID_ARGUMENT as _;
    }

    let client_impl = match client.borrow_data() {
        Ok(impl_) => impl_,
        Err(_) => return RMW_RET_INVALID_ARGUMENT as _,
    };

    unsafe {
        *qos = client_impl.options.qos;
    }

    RMW_RET_OK as _
}

#[unsafe(no_mangle)]
pub extern "C" fn rmw_client_response_subscription_get_actual_qos(
    client: *const rmw_client_t,
    qos: *mut rmw_qos_profile_t,
) -> rmw_ret_t {
    rmw_client_request_publisher_get_actual_qos(client, qos)
}
