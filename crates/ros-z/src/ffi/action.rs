use super::node::{CNode, get_node_ref};
use super::service::QueryKey;
use super::service::{RawServiceClient, RawServiceServer};
use super::{ErrorCode, cstr_to_str};
use crate::attachment::Attachment;
use crate::ffi::publisher::RawPublisher;
use crate::ffi::subscriber::RawSubscriber;
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Callback type for goal acceptance.
/// Returns 1 for accept, 0 for reject.
pub type ActionGoalCallback =
    extern "C" fn(user_data: usize, goal_data: *const u8, goal_len: usize) -> i32;

/// Callback type for goal execution.
/// Must write result bytes and return 0 on success.
/// The goal_id (16 bytes) identifies this specific goal for feedback publishing.
pub type ActionExecuteCallback = extern "C" fn(
    user_data: usize,
    goal_id: *const u8,
    goal_data: *const u8,
    goal_len: usize,
    result_data: *mut *mut u8,
    result_len: *mut usize,
) -> i32;

/// Callback type for feedback
pub type ActionFeedbackCallback =
    extern "C" fn(user_data: usize, feedback_data: *const u8, feedback_len: usize);

/// Raw action client for FFI.
/// Contains: service clients for SendGoal/GetResult/CancelGoal + subscriber for Feedback.
pub struct RawActionClient {
    pub(crate) send_goal_client: RawServiceClient,
    pub(crate) get_result_client: RawServiceClient,
    pub(crate) cancel_goal_client: RawServiceClient,
    pub(crate) _feedback_sub: RawSubscriber,
}

/// Raw action server for FFI.
/// Contains: service servers for SendGoal/GetResult/CancelGoal + publishers for Feedback/Status.
pub struct RawActionServer {
    pub(crate) send_goal_server: RawServiceServer,
    pub(crate) get_result_server: RawServiceServer,
    pub(crate) cancel_goal_server: RawServiceServer,
    pub(crate) feedback_pub: RawPublisher,
    pub(crate) _status_pub: RawPublisher,
    pub(crate) pending_results: HashMap<[u8; 16], Vec<u8>>,
    pub(crate) pending_result_queries: HashMap<[u8; 16], QueryKey>,
}

/// Opaque action client handle for FFI
pub struct CActionClient {
    inner: Box<RawActionClient>,
    _feedback_shutdown: Arc<AtomicBool>,
}

/// Opaque action server handle for FFI
pub struct CActionServer {
    server: Arc<Mutex<RawActionServer>>,
    /// Cancel flags per goal, shared with the dedicated cancel-polling thread.
    cancel_flags: Arc<Mutex<HashMap<[u8; 16], bool>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    _cancel_thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

/// Opaque goal handle for FFI (client-side)
pub struct CGoalHandle {
    goal_id: [u8; 16],
    client: *mut CActionClient,
}

/// Create an action client
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_client_create(
    node: *mut CNode,
    action_name: *const c_char,
    action_type_name: *const c_char,
    goal_type_name: *const c_char,
    goal_type_hash: *const c_char,
    result_type_name: *const c_char,
    result_type_hash: *const c_char,
    feedback_type_name: *const c_char,
    feedback_type_hash: *const c_char,
) -> *mut CActionClient {
    unsafe {
        let node_ref = match get_node_ref(node) {
            Some(n) => n,
            None => return std::ptr::null_mut(),
        };

        let action_str = match cstr_to_str(action_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let action_type = match cstr_to_str(action_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let goal_type = match cstr_to_str(goal_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let goal_hash = match cstr_to_str(goal_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let result_type = match cstr_to_str(result_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let result_hash = match cstr_to_str(result_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let feedback_type = match cstr_to_str(feedback_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let feedback_hash = match cstr_to_str(feedback_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        match node_ref.create_raw_action_client(
            action_str,
            action_type,
            goal_type,
            goal_hash,
            result_type,
            result_hash,
            feedback_type,
            feedback_hash,
        ) {
            Ok(raw_client) => {
                let shutdown = Arc::new(AtomicBool::new(false));
                Box::into_raw(Box::new(CActionClient {
                    inner: Box::new(raw_client),
                    _feedback_shutdown: shutdown,
                }))
            }
            Err(e) => {
                tracing::warn!("ros-z: Failed to create action client: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Send a goal to an action server.
/// On success, writes the goal_id (16 bytes) and creates a goal handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_client_send_goal(
    client_handle: *mut CActionClient,
    goal_data: *const u8,
    goal_len: usize,
    goal_id: *mut [u8; 16],
    handle: *mut *mut CGoalHandle,
) -> i32 {
    if client_handle.is_null() || goal_data.is_null() || goal_id.is_null() || handle.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let client = &(*client_handle);
        let goal = std::slice::from_raw_parts(goal_data, goal_len);

        // Generate a UUID for the goal
        let uuid = uuid::Uuid::new_v4();
        let uuid_bytes: [u8; 16] = *uuid.as_bytes();

        // Build CDR-encoded SendGoal_Request_: [CDR header 4B][UUID 16B][goal raw bytes]
        // goal is CDR-serialized (with 4-byte header); strip that header for the nested field.
        let goal_raw = if goal.len() >= 4 { &goal[4..] } else { goal };
        let mut request = Vec::with_capacity(4 + 16 + goal_raw.len());
        request.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // CDR LE header
        request.extend_from_slice(&uuid_bytes);
        request.extend_from_slice(goal_raw);

        match client
            .inner
            .send_goal_client
            .call_raw(&request, Duration::from_secs(10))
        {
            Ok(response) => {
                // Check if goal was accepted.
                // Response is CDR-encoded SendGoal_Response_:
                // [0..4] CDR header, [4] accepted bool, [5..7] padding, [8..16] stamp.
                // Legacy single-byte response: [0] accepted (for backward compat with old servers).
                let accepted = if response.len() >= 5 {
                    response[4] != 0 // CDR-encoded response
                } else if response.len() == 1 {
                    response[0] != 0 // legacy raw byte
                } else {
                    false
                };
                if !accepted {
                    return super::ErrorCode::ActionGoalRejected as i32;
                }

                *goal_id = uuid_bytes;

                let goal_handle = Box::new(CGoalHandle {
                    goal_id: uuid_bytes,
                    client: client_handle,
                });
                *handle = Box::into_raw(goal_handle);
                ErrorCode::Success as i32
            }
            Err(e) => {
                tracing::warn!("ros-z: Send goal failed: {}", e);
                -1
            }
        }
    }
}

/// Get result for a goal
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_client_get_result(
    goal_handle: *mut CGoalHandle,
    result_data: *mut *mut u8,
    result_len: *mut usize,
) -> i32 {
    if goal_handle.is_null() || result_data.is_null() || result_len.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let gh = &(*goal_handle);
        let client = &(*gh.client);

        // Build CDR-encoded GetResult_Request_: [CDR header 4B][UUID 16B]
        let mut request = Vec::with_capacity(4 + 16);
        request.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // CDR LE header
        request.extend_from_slice(&gh.goal_id);
        match client
            .inner
            .get_result_client
            .call_raw(&request, Duration::from_secs(30))
        {
            Ok(response) => {
                // Parse CDR-encoded GetResult_Response_:
                // [CDR header 4B][status uint8][3 padding][result raw bytes]
                // Return just the result field re-wrapped with a CDR header so the
                // caller can use DeserializeCDR() directly.
                let result_raw = if response.len() >= 8 {
                    &response[8..] // skip CDR header(4) + status(1) + padding(3)
                } else {
                    &response[..]
                };
                let mut result_cdr = Vec::with_capacity(4 + result_raw.len());
                result_cdr.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // CDR header
                result_cdr.extend_from_slice(result_raw);
                let boxed = result_cdr.into_boxed_slice();
                *result_len = boxed.len();
                *result_data = Box::into_raw(boxed) as *mut u8;
                ErrorCode::Success as i32
            }
            Err(e) => {
                tracing::warn!("ros-z: Get result failed: {}", e);
                -1
            }
        }
    }
}

/// Cancel a goal
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_client_cancel_goal(goal_handle: *mut CGoalHandle) -> i32 {
    if goal_handle.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let gh = &(*goal_handle);
        let client = &(*gh.client);

        match client
            .inner
            .cancel_goal_client
            .call_raw(&gh.goal_id, Duration::from_secs(10))
        {
            Ok(_) => ErrorCode::Success as i32,
            Err(e) => {
                tracing::warn!("ros-z: Cancel goal failed: {}", e);
                -1
            }
        }
    }
}

/// Destroy an action client
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_client_destroy(client: *mut CActionClient) -> i32 {
    if client.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let inner = Box::from_raw(client);
        inner._feedback_shutdown.store(true, Ordering::Relaxed);
    }
    ErrorCode::Success as i32
}

unsafe extern "C" {
    fn free(ptr: *mut std::ffi::c_void);
}

/// Create an action server.
/// Spawns a background thread that polls for goals, calls the goal callback,
/// and if accepted, calls the execute callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_create(
    node: *mut CNode,
    action_name: *const c_char,
    action_type_name: *const c_char,
    goal_type_name: *const c_char,
    goal_type_hash: *const c_char,
    result_type_name: *const c_char,
    result_type_hash: *const c_char,
    feedback_type_name: *const c_char,
    feedback_type_hash: *const c_char,
    goal_callback: ActionGoalCallback,
    execute_callback: ActionExecuteCallback,
    user_data: usize,
) -> *mut CActionServer {
    unsafe {
        let node_ref = match get_node_ref(node) {
            Some(n) => n,
            None => return std::ptr::null_mut(),
        };

        let action_str = match cstr_to_str(action_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let action_type = match cstr_to_str(action_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let goal_type = match cstr_to_str(goal_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let goal_hash = match cstr_to_str(goal_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let result_type = match cstr_to_str(result_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let result_hash = match cstr_to_str(result_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let feedback_type = match cstr_to_str(feedback_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        let feedback_hash = match cstr_to_str(feedback_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let raw_server = match node_ref.create_raw_action_server(
            action_str,
            action_type,
            goal_type,
            goal_hash,
            result_type,
            result_hash,
            feedback_type,
            feedback_hash,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("ros-z: Failed to create action server: {}", e);
                return std::ptr::null_mut();
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let shutdown_cancel = shutdown.clone();

        let server_mutex = Arc::new(Mutex::new(raw_server));
        let server_mutex_clone = server_mutex.clone();
        let server_mutex_cancel = server_mutex.clone();

        // Shared cancel flags — written by the cancel thread, read via FFI by execute callbacks.
        let cancel_flags: Arc<Mutex<HashMap<[u8; 16], bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let cancel_flags_clone = cancel_flags.clone();

        // Extract queue Arcs before spawning threads so recv_timeout never holds the mutex.
        let (cancel_queue, send_goal_queue, get_result_queue) = {
            let server = server_mutex.lock().unwrap();
            (
                server.cancel_goal_server.queue.clone(),
                server.send_goal_server.queue.clone(),
                server.get_result_server.queue.clone(),
            )
        };

        // Dedicated cancel-polling thread: runs independently of goal execution.
        let cancel_thread = std::thread::spawn(move || {
            while !shutdown_cancel.load(Ordering::Relaxed) {
                if let Some(query) = cancel_queue.recv_timeout(Duration::from_millis(20)) {
                    let attachment: Attachment = match query.attachment() {
                        Some(att) => match att.try_into() {
                            Ok(a) => a,
                            Err(_) => continue,
                        },
                        None => continue,
                    };
                    let key: QueryKey = attachment.into();
                    let payload = match query.payload() {
                        Some(p) => p.to_bytes().to_vec(),
                        None => continue,
                    };

                    // Cancel request payload: 16-byte raw goal_id (Go client sends raw UUID).
                    if payload.len() >= 16 {
                        let mut goal_id = [0u8; 16];
                        goal_id.copy_from_slice(&payload[..16]);
                        cancel_flags_clone.lock().unwrap().insert(goal_id, true);
                        // Store the query and reply with empty response.
                        let mut server = server_mutex_cancel.lock().unwrap();
                        server.cancel_goal_server.map.insert(key.clone(), query);
                        let _ = server.cancel_goal_server.send_response_raw(&key, &[]);
                    }
                }
            }
        });

        // Background driver thread: polls for SendGoal and GetResult requests.
        // Execute callbacks run in sub-threads so this loop stays responsive.
        let thread = std::thread::spawn(move || {
            while !shutdown_clone.load(Ordering::Relaxed) {
                // Poll for SendGoal requests — poll queue directly, no mutex held during wait.
                let goal_req = send_goal_queue.recv_timeout(Duration::from_millis(20));

                if let Some(query) = goal_req {
                    let attachment: Attachment = match query.attachment() {
                        Some(att) => match att.try_into() {
                            Ok(a) => a,
                            Err(_) => continue,
                        },
                        None => continue,
                    };
                    let key: QueryKey = attachment.into();
                    let payload = match query.payload() {
                        Some(p) => p.to_bytes().to_vec(),
                        None => continue,
                    };

                    // Parse CDR-encoded SendGoal_Request_: [CDR header 4B][UUID 16B][goal raw]
                    if payload.len() < 20 {
                        continue;
                    }
                    let mut goal_id = [0u8; 16];
                    goal_id.copy_from_slice(&payload[4..20]); // skip CDR header
                    let goal_raw = &payload[20..]; // goal bytes (no CDR header)
                    // Re-wrap with CDR header so execute callback can call DeserializeCDR().
                    let mut goal_with_header = Vec::with_capacity(4 + goal_raw.len());
                    goal_with_header.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
                    goal_with_header.extend_from_slice(goal_raw);
                    let goal_data = goal_with_header;

                    // Store the query for later reply
                    {
                        let mut server = server_mutex_clone.lock().unwrap();
                        server.send_goal_server.map.insert(key.clone(), query);
                    }

                    // Call goal_callback to check acceptance
                    let accepted = goal_callback(user_data, goal_data.as_ptr(), goal_data.len());

                    if accepted == 1 {
                        // Reply with CDR-encoded SendGoal_Response_: accepted=true, stamp=zero.
                        // CDR layout: [header 4B][accepted 1B][pad 3B][sec 4B][nanosec 4B]
                        let accept_response: [u8; 16] = [
                            0x00, 0x01, 0x00, 0x00, // CDR LE header
                            0x01, 0x00, 0x00, 0x00, // accepted=true + 3-byte padding
                            0x00, 0x00, 0x00, 0x00, // stamp.sec = 0
                            0x00, 0x00, 0x00, 0x00, // stamp.nanosec = 0
                        ];
                        {
                            let mut server = server_mutex_clone.lock().unwrap();
                            let _ = server
                                .send_goal_server
                                .send_response_raw(&key, &accept_response);
                        }

                        // Run execute_callback in a sub-thread so the driver loop stays
                        // responsive to GetResult polls while the goal is executing.
                        let server_for_exec = server_mutex_clone.clone();
                        std::thread::spawn(move || {
                            let mut result_ptr: *mut u8 = std::ptr::null_mut();
                            let mut result_len: usize = 0;

                            let exec_result = execute_callback(
                                user_data,
                                goal_id.as_ptr(),
                                goal_data.as_ptr(),
                                goal_data.len(),
                                &mut result_ptr,
                                &mut result_len,
                            );

                            if exec_result == 0 && !result_ptr.is_null() && result_len > 0 {
                                let result_cdr =
                                    std::slice::from_raw_parts(result_ptr, result_len).to_vec();
                                free(result_ptr as *mut std::ffi::c_void);
                                // Build CDR-encoded GetResult_Response_:
                                // [CDR header 4B][status=SUCCEEDED 1B][3 padding][result raw]
                                let result_raw = if result_cdr.len() >= 4 {
                                    &result_cdr[4..]
                                } else {
                                    &result_cdr[..]
                                };
                                let mut response = Vec::with_capacity(8 + result_raw.len());
                                response.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
                                response.push(0x04); // status = SUCCEEDED (4)
                                response.extend_from_slice(&[0x00, 0x00, 0x00]);
                                response.extend_from_slice(result_raw);
                                let mut server = server_for_exec.lock().unwrap();
                                server.pending_results.insert(goal_id, response);
                                if let Some(result_key) =
                                    server.pending_result_queries.remove(&goal_id)
                                {
                                    let result_clone = server
                                        .pending_results
                                        .get(&goal_id)
                                        .cloned()
                                        .unwrap_or_default();
                                    let _ = server
                                        .get_result_server
                                        .send_response_raw(&result_key, &result_clone);
                                }
                            } else {
                                let mut server = server_for_exec.lock().unwrap();
                                server.pending_results.insert(goal_id, vec![]);
                                if !result_ptr.is_null() {
                                    free(result_ptr as *mut std::ffi::c_void);
                                }
                            }
                        });
                    } else {
                        // Reply with CDR-encoded SendGoal_Response_: accepted=false, stamp=zero.
                        let reject_response: [u8; 16] = [
                            0x00, 0x01, 0x00, 0x00, // CDR LE header
                            0x00, 0x00, 0x00, 0x00, // accepted=false + 3-byte padding
                            0x00, 0x00, 0x00, 0x00, // stamp.sec = 0
                            0x00, 0x00, 0x00, 0x00, // stamp.nanosec = 0
                        ];
                        let mut server = server_mutex_clone.lock().unwrap();
                        let _ = server
                            .send_goal_server
                            .send_response_raw(&key, &reject_response);
                    }
                }

                // Also poll for GetResult requests — no mutex held during wait.
                let result_req = get_result_queue.recv_timeout(Duration::from_millis(10));

                if let Some(query) = result_req {
                    let attachment: Attachment = match query.attachment() {
                        Some(att) => match att.try_into() {
                            Ok(a) => a,
                            Err(_) => continue,
                        },
                        None => continue,
                    };
                    let key: QueryKey = attachment.into();
                    let payload = match query.payload() {
                        Some(p) => p.to_bytes().to_vec(),
                        None => continue,
                    };

                    // CDR-encoded GetResult_Request_: [CDR header 4B][UUID 16B]
                    if payload.len() >= 20 {
                        let mut goal_id = [0u8; 16];
                        goal_id.copy_from_slice(&payload[4..20]); // skip CDR header

                        let mut server = server_mutex_clone.lock().unwrap();
                        server.get_result_server.map.insert(key.clone(), query);

                        if let Some(result) = server.pending_results.get(&goal_id) {
                            let result_clone = result.clone();
                            let _ = server
                                .get_result_server
                                .send_response_raw(&key, &result_clone);
                        } else {
                            // Store for later when result becomes available
                            server.pending_result_queries.insert(goal_id, key);
                        }
                    }
                }
            }
        });

        Box::into_raw(Box::new(CActionServer {
            server: server_mutex,
            cancel_flags,
            thread: Some(thread),
            _cancel_thread: Some(cancel_thread),
            shutdown,
        }))
    }
}

/// Publish feedback for a goal
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_publish_feedback(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
    feedback_data: *const u8,
    feedback_len: usize,
) -> i32 {
    if server_handle.is_null() || goal_id.is_null() || feedback_data.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let server_wrapper = &(*server_handle);
        let goal_id_bytes = &*goal_id;
        let feedback = std::slice::from_raw_parts(feedback_data, feedback_len);

        // Build CDR-encoded FeedbackMessage_: [CDR header 4B][UUID 16B][feedback raw bytes]
        // feedback is CDR-serialized (with 4-byte header); strip it for the nested field.
        let feedback_raw = if feedback.len() >= 4 {
            &feedback[4..]
        } else {
            feedback
        };
        let mut msg = Vec::with_capacity(4 + 16 + feedback_raw.len());
        msg.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // CDR LE header
        msg.extend_from_slice(goal_id_bytes);
        msg.extend_from_slice(feedback_raw);

        let server = server_wrapper.server.lock().unwrap();
        match server.feedback_pub.publish_bytes(&msg) {
            Ok(_) => ErrorCode::Success as i32,
            Err(e) => {
                tracing::warn!("ros-z: Publish feedback failed: {}", e);
                ErrorCode::PublishFailed as i32
            }
        }
    }
}

/// Mark a goal as succeeded
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_succeed(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
    result_data: *const u8,
    result_len: usize,
) -> i32 {
    unsafe { store_result(server_handle, goal_id, result_data, result_len) }
}

/// Mark a goal as aborted
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_abort(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
    result_data: *const u8,
    result_len: usize,
) -> i32 {
    unsafe { store_result(server_handle, goal_id, result_data, result_len) }
}

/// Mark a goal as canceled
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_canceled(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
    result_data: *const u8,
    result_len: usize,
) -> i32 {
    unsafe { store_result(server_handle, goal_id, result_data, result_len) }
}

unsafe fn store_result(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
    result_data: *const u8,
    result_len: usize,
) -> i32 {
    if server_handle.is_null() || goal_id.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let server_wrapper = &(*server_handle);
        let gid = *goal_id;
        let result = if result_data.is_null() || result_len == 0 {
            vec![]
        } else {
            std::slice::from_raw_parts(result_data, result_len).to_vec()
        };

        let mut server = server_wrapper.server.lock().unwrap();
        server.pending_results.insert(gid, result.clone());

        // If there's a pending get_result query, reply now
        if let Some(result_key) = server.pending_result_queries.remove(&gid) {
            let _ = server
                .get_result_server
                .send_response_raw(&result_key, &result);
        }

        ErrorCode::Success as i32
    }
}

/// Check whether a cancel has been requested for the given goal.
/// Returns 1 if cancel was requested, 0 otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_is_cancel_requested(
    server_handle: *mut CActionServer,
    goal_id: *const [u8; 16],
) -> i32 {
    if server_handle.is_null() || goal_id.is_null() {
        return 0;
    }
    unsafe {
        let server_wrapper = &(*server_handle);
        let gid = *goal_id;
        let flags = server_wrapper.cancel_flags.lock().unwrap();
        if flags.get(&gid).copied().unwrap_or(false) {
            1
        } else {
            0
        }
    }
}

/// Destroy an action server
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_action_server_destroy(server: *mut CActionServer) -> i32 {
    if server.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let mut inner = Box::from_raw(server);
        inner.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = inner.thread.take() {
            let _ = thread.join();
        }
    }
    ErrorCode::Success as i32
}

/// Destroy a goal handle
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_goal_handle_destroy(handle: *mut CGoalHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(handle);
    }
    ErrorCode::Success as i32
}
