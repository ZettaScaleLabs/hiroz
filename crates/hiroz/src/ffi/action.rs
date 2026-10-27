//! Action protocol transport and goal lifecycle used by foreign-language bindings.
//! Result envelopes are encoded by the binding's offset-aware message codec. Rust
//! never guesses the alignment of an opaque action result.
use std::{
    collections::HashMap,
    ffi::c_char,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use zenoh::Wait;

use super::{
    ErrorCode, cstr_to_str,
    node::{CNode, get_node_ref},
    qos::CQosProfile,
    service::{QueryKey, RawServiceClient, RawServiceServer},
};
use crate::{
    action::{
        GoalId, GoalInfo, GoalStatus, Time,
        messages::{
            CancelGoalServiceRequest, CancelGoalServiceResponse, GoalStatusInfo, SendGoalResponse,
            StatusMessage,
        },
    },
    attachment::Attachment,
    ffi::{publisher::RawPublisher, subscriber::RawSubscriber},
    msg::ZMessage,
};

pub type ActionGoalCallback = extern "C" fn(usize, *const u8, *const u8, usize) -> i32;
pub type ActionExecuteCallback =
    extern "C" fn(usize, *const u8, *const u8, usize, *mut *mut u8, *mut usize) -> i32;
pub type ActionCancelCallback = extern "C" fn(usize, *const u8) -> i32;
/// kind=0 is a complete FeedbackMessage, kind=1 a complete GoalStatusArray.
pub type ActionEventCallback = extern "C" fn(usize, i32, *const u8, usize);
/// Called once after all native references to the foreign event data are released.
pub type ActionEventDropCallback = extern "C" fn(usize);

struct ActionEventLifetime {
    user_data: usize,
    drop_callback: Option<ActionEventDropCallback>,
    open: AtomicBool,
    active_deliveries: AtomicUsize,
    // Legacy callers release their own data after destroy returns. Owned callers
    // instead use the last-reference notification and may destroy from a callback.
    legacy_delivery: Mutex<()>,
}

struct ActionEventDelivery<'a>(&'a AtomicUsize);

impl Drop for ActionEventDelivery<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ActionEventLifetime {
    fn disable(&self) {
        self.open.store(false, Ordering::SeqCst);
        if self.drop_callback.is_none() {
            drop(self.legacy_delivery.lock().unwrap());
        }
    }

    fn deliver(&self, callback: ActionEventCallback, kind: i32, bytes: &[u8]) {
        // Register before checking open. Together with disable/destroy's SeqCst
        // operations, a callback that observes open cannot be missed by destroy.
        self.active_deliveries.fetch_add(1, Ordering::SeqCst);
        let _active = ActionEventDelivery(&self.active_deliveries);
        // A guard in an `if` condition is dropped before its body executes.
        // Bind this guard so legacy destroy really drains the foreign call.
        let _delivery = self
            .drop_callback
            .is_none()
            .then(|| self.legacy_delivery.lock().unwrap());
        if self.open.load(Ordering::SeqCst) {
            callback(self.user_data, kind, bytes.as_ptr(), bytes.len());
        }
    }
}

impl Drop for ActionEventLifetime {
    fn drop(&mut self) {
        if let Some(callback) = self.drop_callback {
            callback(self.user_data);
        }
    }
}

#[repr(C)]
pub struct CActionQos {
    pub goal: CQosProfile,
    pub result: CQosProfile,
    pub cancel: CQosProfile,
    pub feedback: CQosProfile,
    pub status: CQosProfile,
}

pub struct RawActionClient {
    pub(crate) send_goal_client: RawServiceClient,
    pub(crate) get_result_client: RawServiceClient,
    pub(crate) cancel_goal_client: RawServiceClient,
    pub(crate) _feedback_sub: RawSubscriber,
    pub(crate) _status_sub: RawSubscriber,
}

pub struct RawActionServer {
    pub(crate) send_goal_server: RawServiceServer,
    pub(crate) get_result_server: RawServiceServer,
    pub(crate) cancel_goal_server: RawServiceServer,
    pub(crate) feedback_pub: RawPublisher,
    pub(crate) status_pub: RawPublisher,
    pub(crate) goals: HashMap<[u8; 16], GoalRecord>,
}

pub(crate) struct GoalRecord {
    info: GoalInfo,
    status: GoalStatus,
    result: Option<Vec<u8>>,
    completed: Option<Instant>,
    accepted_at: Instant,
    waiters: Vec<QueryKey>,
}

pub struct CActionClient {
    inner: Arc<RawActionClient>,
    events: Arc<ActionEventLifetime>,
}

pub struct CActionRequest {
    canceled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Vec<u8>, crate::error::Error>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

pub struct CActionServer {
    server: Arc<Mutex<RawActionServer>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    // Go Close drains callbacks asynchronously, so its node and context can be
    // destroyed before this handle. Zenoh entities retain only WeakSession:
    // keep the session alive until workers and all server endpoints are gone.
    // Fields drop in declaration order; this guard must remain last.
    _session: Arc<zenoh::Session>,
}

unsafe extern "C" {
    fn free(ptr: *mut std::ffi::c_void);
}

/// Legacy constructor: foreign callback data remains caller-owned. Destroy waits
/// for active events, so an event callback must not destroy its own client. Use
/// `hiroz_action_client_create_owned` for callback-safe, deferred data release.
///
/// # Safety
/// `node` must be null or a live Hiroz node handle that is not destroyed during
/// this call. Non-null string pointers must reference valid, immutable,
/// NUL-terminated C strings for the call. `qos` must be null or point to an
/// aligned, initialized `CActionQos` that remains readable and unchanged for the
/// call. These arguments are not retained.
///
/// `callback` and any state identified by `user_data` must remain valid until
/// this call fails or `hiroz_action_client_destroy` returns. Events may arrive
/// on native threads before construction returns; the callback must synchronize
/// access to shared foreign state, must not unwind across the FFI boundary, and
/// must not mutate its borrowed byte buffer or retain it beyond the invocation.
/// The callback must not destroy its own client or wait for its destruction.
/// The caller retains ownership of `user_data` and must destroy a non-null
/// returned handle exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_client_create_with_options(
    node: *mut CNode,
    name: *const c_char,
    action_type: *const c_char,
    goal_hash: *const c_char,
    result_hash: *const c_char,
    cancel_hash: *const c_char,
    feedback_hash: *const c_char,
    status_hash: *const c_char,
    qos: *const CActionQos,
    callback: ActionEventCallback,
    user_data: usize,
) -> *mut CActionClient {
    unsafe {
        create_action_client_owned(
            node,
            name,
            action_type,
            goal_hash,
            result_hash,
            cancel_hash,
            feedback_hash,
            status_hash,
            qos,
            callback,
            user_data,
            None,
        )
    }
}

/// Create a client taking ownership of its foreign event callback data.
/// `drop_callback` runs exactly once, including on construction failure, after
/// every in-progress delivery and native transport/request clone has released
/// the data. Destroy disables new delivery without waiting for active callbacks,
/// so a callback may destroy its own client. Do not release user_data separately.
///
/// # Safety
/// `node` must be null or a live Hiroz node handle that is not destroyed during
/// this call. Non-null string pointers must reference valid, immutable,
/// NUL-terminated C strings for the call. `qos` must be null or point to an
/// aligned, initialized `CActionQos` that remains readable and unchanged for the
/// call. These arguments are not retained.
///
/// Ownership of the state identified by `user_data` transfers to
/// `drop_callback`, even if construction fails. The state must remain valid
/// until that callback releases it, and both callback functions must remain
/// callable until it finishes. The caller must not release the state separately.
/// Events may begin before construction returns and may run concurrently on
/// native threads. The callbacks must synchronize foreign state and must not
/// unwind across the FFI boundary.
/// Event bytes are immutable and borrowed only for the invocation. The drop
/// callback must be safe to run on whichever thread releases the last reference.
/// A non-null returned handle must be destroyed exactly once. An event may
/// destroy its own client but must not wait for the drop callback or teardown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_client_create_owned(
    node: *mut CNode,
    name: *const c_char,
    action_type: *const c_char,
    goal_hash: *const c_char,
    result_hash: *const c_char,
    cancel_hash: *const c_char,
    feedback_hash: *const c_char,
    status_hash: *const c_char,
    qos: *const CActionQos,
    callback: ActionEventCallback,
    user_data: usize,
    drop_callback: ActionEventDropCallback,
) -> *mut CActionClient {
    unsafe {
        create_action_client_owned(
            node,
            name,
            action_type,
            goal_hash,
            result_hash,
            cancel_hash,
            feedback_hash,
            status_hash,
            qos,
            callback,
            user_data,
            Some(drop_callback),
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_action_client_owned(
    node: *mut CNode,
    name: *const c_char,
    action_type: *const c_char,
    goal_hash: *const c_char,
    result_hash: *const c_char,
    cancel_hash: *const c_char,
    feedback_hash: *const c_char,
    status_hash: *const c_char,
    qos: *const CActionQos,
    callback: ActionEventCallback,
    user_data: usize,
    drop_callback: Option<ActionEventDropCallback>,
) -> *mut CActionClient {
    let events = Arc::new(ActionEventLifetime {
        user_data,
        drop_callback,
        open: AtomicBool::new(true),
        active_deliveries: AtomicUsize::new(0),
        legacy_delivery: Mutex::new(()),
    });
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        let strings = [
            name,
            action_type,
            goal_hash,
            result_hash,
            cancel_hash,
            feedback_hash,
            status_hash,
        ]
        .map(|s| cstr_to_str(s));
        let [
            Ok(name),
            Ok(action_type),
            Ok(goal),
            Ok(result),
            Ok(cancel),
            Ok(feedback),
            Ok(status),
        ] = strings
        else {
            return std::ptr::null_mut();
        };
        let delivery = events.clone();
        let event = move |kind, bytes: &[u8]| {
            // Keep an independent reference across foreign code, even when that
            // code destroys the client and its subscriber declarations.
            let lifetime = delivery.clone();
            lifetime.deliver(callback, kind, bytes);
        };
        match node.create_raw_action_client(
            name,
            action_type,
            goal,
            result,
            cancel,
            feedback,
            status,
            qos.as_ref(),
            event,
        ) {
            Ok(inner) => Box::into_raw(Box::new(CActionClient {
                inner: Arc::new(inner),
                events,
            })),
            Err(e) => {
                events.disable();
                tracing::warn!("Action client creation failed: {e}");
                std::ptr::null_mut()
            }
        }
    }
}

/// Start a request without retaining foreign memory. kind: 0=SendGoal, 1=GetResult, 2=CancelGoal.
///
/// # Safety
/// `client` must be null or a live handle returned by an action client
/// constructor, and must not be destroyed during this call. If non-null,
/// `bytes` must reference `len` initialized, immutable bytes within one live
/// allocation for the call, with `len <= isize::MAX` (including when `len` is
/// zero). The bytes are copied, and the request retains its own transport
/// reference, so neither input needs to outlive this call. A non-null returned
/// request must be destroyed exactly once with `hiroz_action_request_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_request_create(
    client: *mut CActionClient,
    kind: i32,
    bytes: *const u8,
    len: usize,
    timeout_ms: u64,
) -> *mut CActionRequest {
    if client.is_null() || bytes.is_null() || !(0..=2).contains(&kind) || timeout_ms == 0 {
        return std::ptr::null_mut();
    }
    unsafe {
        let inner = (*client).inner.clone();
        let bytes = std::slice::from_raw_parts(bytes, len).to_vec();
        let canceled = Arc::new(AtomicBool::new(false));
        let flag = canceled.clone();
        let (tx, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let service = match kind {
                0 => &inner.send_goal_client,
                1 => &inner.get_result_client,
                _ => &inner.cancel_goal_client,
            };
            let _ = tx.send(service.call_raw_with_cancel(
                &bytes,
                Duration::from_millis(timeout_ms),
                &flag,
            ));
        });
        Box::into_raw(Box::new(CActionRequest {
            canceled,
            receiver,
            worker: Some(worker),
        }))
    }
}

/// Returns 1 while pending, 0 with owned response bytes, or a negative error code.
///
/// # Safety
/// `request` must be null or a live handle returned by
/// `hiroz_action_request_create`. All polling and destruction of the same
/// request must be serialized. Non-null `data` and `len` must point to aligned,
/// writable storage of their declared types, exclusively accessible for the
/// call and not overlapping each other or the request. On success, the caller
/// owns the returned buffer and must pass its exact pointer and length once to
/// `hiroz_free_bytes`; it must not use C `free` or access the buffer after release.
/// Output storage is unchanged while pending or on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_request_poll(
    request: *mut CActionRequest,
    data: *mut *mut u8,
    len: *mut usize,
) -> i32 {
    if request.is_null() || data.is_null() || len.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        match (*request).receiver.try_recv() {
            Ok(Ok(bytes)) => {
                let bytes = bytes.into_boxed_slice();
                *len = bytes.len();
                *data = Box::into_raw(bytes) as *mut u8;
                0
            }
            Ok(Err(crate::error::Error::Timeout(_))) => ErrorCode::ServiceTimeout as i32,
            Ok(Err(e)) => {
                tracing::debug!("Action request failed: {e}");
                ErrorCode::ActionResultFailed as i32
            }
            Err(mpsc::TryRecvError::Empty) => 1,
            Err(_) => ErrorCode::ActionResultFailed as i32,
        }
    }
}

/// Cancel an outstanding request, join its worker, and release its handle.
///
/// # Safety
/// `request` must be null or a live handle returned by
/// `hiroz_action_request_create`. A non-null handle must be destroyed exactly
/// once, with no concurrent poll, destruction, or later use of that handle.
/// Buffers previously returned by polling remain separately caller-owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_request_destroy(request: *mut CActionRequest) {
    if !request.is_null() {
        unsafe {
            let mut request = Box::from_raw(request);
            request.canceled.store(true, Ordering::Release);
            if let Some(worker) = request.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

/// Check whether all three action services are available.
///
/// # Safety
/// `client` must be null or a live handle returned by an action client
/// constructor. The handle must remain alive throughout the call; destruction
/// must not run concurrently. Other shared client operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_client_ready(client: *mut CActionClient) -> bool {
    if client.is_null() {
        return false;
    }
    unsafe {
        let inner = &(*client).inner;
        inner.send_goal_client.wait_for_service(Duration::ZERO)
            && inner.get_result_client.wait_for_service(Duration::ZERO)
            && inner.cancel_goal_client.wait_for_service(Duration::ZERO)
    }
}

/// Disable event delivery and destroy the client handle. Owned clients release
/// foreign data through their drop callback after outstanding references drain.
/// Legacy clients wait for callbacks here and must not call this from an event.
///
/// # Safety
/// `client` must be null or a live handle returned by an action client
/// constructor. A non-null handle must be destroyed exactly once, with no
/// concurrent use of the handle and no later use, including from callbacks.
/// Existing requests retain their own transport references and may outlive it.
/// For a legacy client, callback code and `user_data` must remain valid until
/// this function returns, and this function must not be called from its event
/// callback or a thread that the callback is waiting for. For an owned client,
/// callback code must remain valid until its drop callback finishes, and
/// `user_data` must remain valid until that callback releases it. An event may
/// destroy an owned client but must not wait for its drop callback or teardown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_client_destroy(client: *mut CActionClient) -> i32 {
    if client.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let client = Box::from_raw(client);
        client.events.disable();
        if client.events.drop_callback.is_some()
            && client.events.active_deliveries.load(Ordering::SeqCst) != 0
        {
            // Historical/recovery delivery holds AdvancedSubscriber's state
            // mutex outside our closure. Dropping that subscriber here can
            // recursively lock it. A separate thread may wait for the complete
            // transport callback to unwind; the foreign callback must not wait.
            // Ordinary close without active delivery remains synchronous.
            std::thread::spawn(move || drop(client));
        }
    }
    0
}

fn publish_status(server: &RawActionServer) {
    let status = StatusMessage {
        status_list: server
            .goals
            .values()
            .map(|goal| GoalStatusInfo {
                goal_info: goal.info.clone(),
                status: goal.status,
            })
            .collect(),
    };
    if let Err(e) = server.status_pub.publish_bytes(&status.serialize()) {
        tracing::warn!("Action status publication failed: {e}");
    }
}

/// Exactly one terminal transition owns the result, including explicit completion in a callback.
fn finish(server: &mut RawActionServer, id: [u8; 16], response: Vec<u8>) -> bool {
    if response.len() < 5 {
        return false;
    }
    let Ok(status) = GoalStatus::try_from(response[4] as i8) else {
        return false;
    };
    if !status.is_terminal() {
        return false;
    }
    let Some(goal) = server.goals.get_mut(&id) else {
        return false;
    };
    if goal.status.is_terminal()
        || (status == GoalStatus::Canceled && goal.status != GoalStatus::Canceling)
    {
        return false;
    }
    goal.status = status;
    goal.result = Some(response.clone());
    goal.completed = Some(Instant::now());
    let waiters = std::mem::take(&mut goal.waiters);
    for key in waiters {
        let _ = server.get_result_server.send_response_raw(&key, &response);
    }
    publish_status(server);
    true
}

fn selected(info: &GoalInfo, request: &GoalInfo) -> bool {
    let all = request.goal_id == GoalId::from_bytes([0; 16]);
    let zero_stamp = request.stamp == Time::zero();
    (all && zero_stamp)
        || (!all && info.goal_id == request.goal_id)
        || (!zero_stamp
            && (info.stamp.sec, info.stamp.nanosec) <= (request.stamp.sec, request.stamp.nanosec))
}

fn query_parts(query: &zenoh::query::Query) -> Option<(QueryKey, Vec<u8>)> {
    let attachment: Attachment = query.attachment()?.try_into().ok()?;
    Some((attachment.into(), query.payload()?.to_bytes().to_vec()))
}

/// Create an action server with foreign goal, execution, and cancellation callbacks.
///
/// # Safety
/// `node` must be null or a live Hiroz node handle that is not destroyed during
/// this call. Non-null string pointers must reference valid, immutable,
/// NUL-terminated C strings for the call. `qos` must be null or point to an
/// aligned, initialized `CActionQos` that remains readable and unchanged for the
/// call. If non-null, `default_result` must reference `default_result_len`
/// initialized, immutable bytes within one live allocation for the call, with
/// `default_result_len <= isize::MAX`. These inputs are not retained.
///
/// Callback code and the caller-owned state identified by `user_data` must
/// remain valid until construction fails or `hiroz_action_server_destroy`
/// returns. Callbacks may start before construction returns and execute
/// concurrently on native threads; they must synchronize foreign state and
/// must not unwind across the FFI boundary. Callback goal IDs reference 16
/// immutable bytes, and goal payloads are immutable buffers of the supplied
/// length; these pointers are borrowed only for the invocation.
///
/// The execution callback's writable output pointers are borrowed only for the
/// invocation. Any non-null result it returns, even on error, must transfer
/// ownership of an allocation compatible with C `free`, containing the stated
/// number of initialized bytes within one allocation, with length at most
/// `isize::MAX`. Rust copies and frees that allocation after the callback
/// returns; foreign code must neither access nor free it thereafter.
/// A non-null server handle must be destroyed exactly once. Callbacks must not
/// destroy their own server or wait for its destruction; they may request
/// shutdown with `hiroz_action_server_shutdown` instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_create_with_options(
    node: *mut CNode,
    name: *const c_char,
    action_type: *const c_char,
    goal_hash: *const c_char,
    result_hash: *const c_char,
    cancel_hash: *const c_char,
    feedback_hash: *const c_char,
    status_hash: *const c_char,
    qos: *const CActionQos,
    goal_callback: ActionGoalCallback,
    execute_callback: ActionExecuteCallback,
    cancel_callback: ActionCancelCallback,
    user_data: usize,
    default_result: *const u8,
    default_result_len: usize,
    result_timeout_ms: u64,
    goal_timeout_ms: u64,
) -> *mut CActionServer {
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        if default_result.is_null() || default_result_len < 5 {
            return std::ptr::null_mut();
        }
        let unknown = std::slice::from_raw_parts(default_result, default_result_len).to_vec();
        let mut aborted = unknown.clone();
        aborted[4] = GoalStatus::Aborted as u8;
        let strings = [
            name,
            action_type,
            goal_hash,
            result_hash,
            cancel_hash,
            feedback_hash,
            status_hash,
        ]
        .map(|s| cstr_to_str(s));
        let [
            Ok(name),
            Ok(action_type),
            Ok(goal),
            Ok(result),
            Ok(cancel),
            Ok(feedback),
            Ok(status),
        ] = strings
        else {
            return std::ptr::null_mut();
        };
        let raw = match node.create_raw_action_server(
            name,
            action_type,
            goal,
            result,
            cancel,
            feedback,
            status,
            qos.as_ref(),
        ) {
            Ok(server) => server,
            Err(e) => {
                tracing::warn!("Action server creation failed: {e}");
                return std::ptr::null_mut();
            }
        };
        let goals = raw.send_goal_server.queue.clone();
        let results = raw.get_result_server.queue.clone();
        let cancels = raw.cancel_goal_server.queue.clone();
        let server = Arc::new(Mutex::new(raw));
        let state = server.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let thread = std::thread::spawn(move || {
            let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
            while !stop.load(Ordering::Acquire) {
                // Reap completed workers; retain every active callback until destruction.
                let mut index = 0;
                while index < workers.len() {
                    if workers[index].is_finished() {
                        let _ = workers.swap_remove(index).join();
                    } else {
                        index += 1;
                    }
                }
                if let Some(query) = goals.recv_timeout(Duration::from_millis(5))
                    && let Some((key, payload)) = query_parts(&query)
                    && payload.len() >= 20
                    && payload[..4] == [0, 1, 0, 0]
                {
                    // UUID is 16 bytes: moving a CDR1 body by 16 preserves every primitive's alignment.
                    let id: [u8; 16] = payload[4..20].try_into().unwrap();
                    let mut data = payload[..4].to_vec();
                    data.extend_from_slice(&payload[20..]);
                    let duplicate = state.lock().unwrap().goals.contains_key(&id);
                    let decision = if duplicate {
                        0
                    } else {
                        goal_callback(user_data, id.as_ptr(), data.as_ptr(), data.len())
                    };
                    let accepted = decision == 1 || decision == 2;
                    let stamp = if accepted { Time::now() } else { Time::zero() };
                    let response = SendGoalResponse {
                        accepted,
                        stamp_sec: stamp.sec,
                        stamp_nanosec: stamp.nanosec,
                    }
                    .serialize();
                    {
                        let mut server = state.lock().unwrap();
                        if accepted {
                            server.goals.insert(
                                id,
                                GoalRecord {
                                    info: GoalInfo {
                                        goal_id: GoalId::from_bytes(id),
                                        stamp,
                                    },
                                    status: GoalStatus::Accepted,
                                    result: None,
                                    completed: None,
                                    accepted_at: Instant::now(),
                                    waiters: Vec::new(),
                                },
                            );
                        }
                        server.send_goal_server.map.insert(key.clone(), query);
                        let _ = server.send_goal_server.send_response_raw(&key, &response);
                        if accepted {
                            publish_status(&server);
                        }
                    }
                    if accepted {
                        let state = state.clone();
                        let aborted = aborted.clone();
                        workers.push(std::thread::spawn(move || {
                            if decision == 1 {
                                let mut server = state.lock().unwrap();
                                if let Some(goal) = server.goals.get_mut(&id)
                                    && goal.status == GoalStatus::Accepted
                                {
                                    goal.status = GoalStatus::Executing;
                                }
                                publish_status(&server);
                            }
                            let mut ptr = std::ptr::null_mut();
                            let mut len = 0;
                            let rc = execute_callback(
                                user_data,
                                id.as_ptr(),
                                data.as_ptr(),
                                data.len(),
                                &mut ptr,
                                &mut len,
                            );
                            let response = if !ptr.is_null() {
                                let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
                                free(ptr.cast());
                                Some(bytes)
                            } else {
                                None
                            };
                            // Deferred callbacks deliberately leave an accepted goal without a result.
                            if decision != 2 || rc != 0 || response.is_some() {
                                let mut server = state.lock().unwrap();
                                finish(
                                    &mut server,
                                    id,
                                    if rc == 0 {
                                        response.unwrap_or(aborted)
                                    } else {
                                        aborted
                                    },
                                );
                            }
                        }));
                    }
                }
                while let Some(query) = results.recv_timeout(Duration::ZERO) {
                    if let Some((key, payload)) = query_parts(&query) {
                        if payload.len() != 20 || payload[..4] != [0, 1, 0, 0] {
                            continue;
                        }
                        let id: [u8; 16] = payload[4..20].try_into().unwrap();
                        let mut server = state.lock().unwrap();
                        server.get_result_server.map.insert(key.clone(), query);
                        match server.goals.get_mut(&id) {
                            Some(goal) if goal.result.is_none() => goal.waiters.push(key),
                            Some(goal) => {
                                let response = goal.result.clone().unwrap();
                                let _ = server.get_result_server.send_response_raw(&key, &response);
                            }
                            None => {
                                let _ = server.get_result_server.send_response_raw(&key, &unknown);
                            }
                        }
                    }
                }
                while let Some(query) = cancels.recv_timeout(Duration::ZERO) {
                    if let Some((key, payload)) = query_parts(&query) {
                        let Ok(request) = CancelGoalServiceRequest::deserialize(&payload) else {
                            continue;
                        };
                        let candidates: Vec<_> = {
                            let server = state.lock().unwrap();
                            server
                                .goals
                                .iter()
                                .filter(|(_, goal)| {
                                    goal.status.is_active()
                                        && selected(&goal.info, &request.goal_info)
                                })
                                .map(|(id, _)| *id)
                                .collect()
                        };
                        // Never hold the state lock across application cancellation policy.
                        let accepted: Vec<_> = candidates
                            .iter()
                            .filter(|id| cancel_callback(user_data, id.as_ptr()) != 0)
                            .copied()
                            .collect();
                        let mut server = state.lock().unwrap();
                        let mut canceling = Vec::new();
                        for id in accepted {
                            if let Some(goal) = server.goals.get_mut(&id)
                                && goal.status.is_active()
                            {
                                goal.status = GoalStatus::Canceling;
                                canceling.push(goal.info.clone());
                            }
                        }
                        let specific = request.goal_info.goal_id != GoalId::from_bytes([0; 16]);
                        let return_code = if !canceling.is_empty() {
                            0
                        } else if specific {
                            match server.goals.get(request.goal_info.goal_id.as_bytes()) {
                                None => 2,
                                Some(goal) if goal.status.is_terminal() => 3,
                                _ => 1,
                            }
                        } else if candidates.is_empty() {
                            0
                        } else {
                            1
                        };
                        let response = CancelGoalServiceResponse {
                            return_code,
                            goals_canceling: canceling,
                        }
                        .serialize();
                        server.cancel_goal_server.map.insert(key.clone(), query);
                        let _ = server.cancel_goal_server.send_response_raw(&key, &response);
                        publish_status(&server);
                    }
                }
                let mut server = state.lock().unwrap();
                if goal_timeout_ms > 0 {
                    let expired: Vec<_> = server
                        .goals
                        .iter()
                        .filter(|(_, g)| {
                            g.status.is_active()
                                && g.accepted_at.elapsed() >= Duration::from_millis(goal_timeout_ms)
                        })
                        .map(|(id, _)| *id)
                        .collect();
                    for id in expired {
                        finish(&mut server, id, aborted.clone());
                    }
                }
                let before = server.goals.len();
                server.goals.retain(|_, goal| {
                    goal.completed.is_none_or(|completed| {
                        completed.elapsed() < Duration::from_millis(result_timeout_ms)
                    })
                });
                if server.goals.len() != before {
                    publish_status(&server);
                }
            }
            // A server cannot free Go callbacks while a worker is inside one.
            for worker in workers {
                let _ = worker.join();
            }
        });
        Box::into_raw(Box::new(CActionServer {
            server,
            shutdown,
            thread: Some(thread),
            _session: node.session.clone(),
        }))
    }
}

/// Read the current status of a goal, or zero when it is unknown.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor, with no concurrent destruction. If non-null, `id` must reference
/// 16 initialized, immutable bytes within one live allocation for the call.
/// Other server operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_goal_status(
    server: *mut CActionServer,
    id: *const u8,
) -> i32 {
    if server.is_null() || id.is_null() {
        return 0;
    }
    unsafe {
        let id: [u8; 16] = std::slice::from_raw_parts(id, 16).try_into().unwrap();
        (*server)
            .server
            .lock()
            .unwrap()
            .goals
            .get(&id)
            .map_or(0, |goal| goal.status as i32)
    }
}

/// Transition an accepted goal to executing.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor, with no concurrent destruction. If non-null, `id` must reference
/// 16 initialized, immutable bytes within one live allocation for the call.
/// Other server operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_execute(
    server: *mut CActionServer,
    id: *const u8,
) -> i32 {
    if server.is_null() || id.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let id: [u8; 16] = std::slice::from_raw_parts(id, 16).try_into().unwrap();
        let mut server = (*server).server.lock().unwrap();
        let Some(goal) = server.goals.get_mut(&id) else {
            return ErrorCode::ActionResultFailed as i32;
        };
        if goal.status != GoalStatus::Accepted {
            return ErrorCode::ActionResultFailed as i32;
        }
        goal.status = GoalStatus::Executing;
        publish_status(&server);
        0
    }
}

/// Publish a fully encoded FeedbackMessage supplied by the binding.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor, with no concurrent destruction. If non-null, `bytes` must
/// reference `len` initialized, immutable bytes within one live allocation for
/// the call, with `len <= isize::MAX`. The buffer is not retained. Other server
/// operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_publish_feedback_message(
    server: *mut CActionServer,
    bytes: *const u8,
    len: usize,
) -> i32 {
    if server.is_null() || bytes.is_null() || len < 20 {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let bytes = std::slice::from_raw_parts(bytes, len);
        let server = (*server).server.lock().unwrap();
        let id: [u8; 16] = bytes[4..20].try_into().unwrap();
        if !server
            .goals
            .get(&id)
            .is_some_and(|g| matches!(g.status, GoalStatus::Executing | GoalStatus::Canceling))
        {
            return ErrorCode::ActionFeedbackFailed as i32;
        }
        if server.feedback_pub.publish_bytes(bytes).is_ok() {
            0
        } else {
            ErrorCode::ActionFeedbackFailed as i32
        }
    }
}

/// Complete a goal with a fully encoded result response.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor, with no concurrent destruction. If non-null, `id` must reference
/// 16 initialized, immutable bytes, and `bytes` must reference `len` initialized,
/// immutable bytes, each within one live allocation for the call, with
/// `len <= isize::MAX`. The result is copied; neither buffer is retained. Other
/// server operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_finish(
    server: *mut CActionServer,
    id: *const u8,
    bytes: *const u8,
    len: usize,
) -> i32 {
    if server.is_null() || id.is_null() || bytes.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let id: [u8; 16] = std::slice::from_raw_parts(id, 16).try_into().unwrap();
        let bytes = std::slice::from_raw_parts(bytes, len).to_vec();
        let mut server = (*server).server.lock().unwrap();
        if server
            .goals
            .get(&id)
            .is_some_and(|goal| goal.status == GoalStatus::Accepted)
        {
            return ErrorCode::ActionResultFailed as i32;
        }
        if finish(&mut server, id, bytes) {
            0
        } else {
            ErrorCode::ActionResultFailed as i32
        }
    }
}

/// Read acceptance metadata without exposing a borrowed goal record.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor, with no concurrent destruction. If non-null, `id` must reference
/// 16 initialized, immutable bytes within one live allocation for the call.
/// Non-null `sec` and `nanosec` must point to aligned, writable storage of their
/// declared types, exclusively accessible for the call and not overlapping
/// each other, `id`, or the server. Other server operations may run concurrently
/// with separate output storage. Outputs are unchanged when this returns false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_goal_info(
    server: *mut CActionServer,
    id: *const u8,
    sec: *mut i32,
    nanosec: *mut u32,
) -> bool {
    if server.is_null() || id.is_null() || sec.is_null() || nanosec.is_null() {
        return false;
    }
    unsafe {
        let id: [u8; 16] = std::slice::from_raw_parts(id, 16).try_into().unwrap();
        let server = (*server).server.lock().unwrap();
        if let Some(goal) = server.goals.get(&id) {
            *sec = goal.info.stamp.sec;
            *nanosec = goal.info.stamp.nanosec;
            true
        } else {
            false
        }
    }
}

/// Check whether a feedback subscriber is matched.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor. The handle must remain alive throughout the call, with no
/// concurrent destruction. Other server operations may run concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_feedback_ready(server: *mut CActionServer) -> bool {
    if server.is_null() {
        return false;
    }
    unsafe {
        (*server)
            .server
            .lock()
            .unwrap()
            .feedback_pub
            .inner
            .matching_status()
            .wait()
            .is_ok_and(|status| status.matching())
    }
}

/// Stop accepting requests without waiting for callbacks. Safe from a callback.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor. The handle must remain alive throughout the call, with no
/// concurrent destruction. This may run alongside other server operations,
/// including callbacks. It does not release the handle or callback state;
/// those must remain valid until `hiroz_action_server_destroy` completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_shutdown(server: *mut CActionServer) {
    if !server.is_null() {
        unsafe {
            (*server).shutdown.store(true, Ordering::Release);
        }
    }
}

/// Shut down the server, drain its callbacks, and release its handle.
///
/// # Safety
/// `server` must be null or a live handle returned by the action server
/// constructor. A non-null handle must be destroyed exactly once, with no
/// concurrent or later use of that handle, including from callbacks. Callback
/// code and `user_data` must remain valid until this function returns. This
/// function must not run inside one of this server's callbacks or on a thread
/// that a callback is waiting for, because it joins all callback workers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_action_server_destroy(server: *mut CActionServer) -> i32 {
    if server.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let mut server = Box::from_raw(server);
        server.shutdown.store(true, Ordering::Release);
        if let Some(thread) = server.thread.take() {
            let _ = thread.join();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_selection_obeys_uuid_and_timestamp_union() {
        let id = GoalId::from_bytes([1; 16]);
        let other = GoalInfo {
            goal_id: GoalId::from_bytes([2; 16]),
            stamp: Time { sec: 4, nanosec: 0 },
        };
        assert!(!selected(
            &other,
            &GoalInfo {
                goal_id: id,
                stamp: Time::zero()
            }
        ));
        assert!(selected(
            &other,
            &GoalInfo {
                goal_id: GoalId::from_bytes([0; 16]),
                stamp: Time::zero()
            }
        ));
        assert!(selected(
            &other,
            &GoalInfo {
                goal_id: id,
                stamp: Time { sec: 4, nanosec: 0 }
            }
        ));
        assert!(!selected(
            &other,
            &GoalInfo {
                goal_id: GoalId::from_bytes([0; 16]),
                stamp: Time {
                    sec: 3,
                    nanosec: 999999999
                }
            }
        ));
    }
}
