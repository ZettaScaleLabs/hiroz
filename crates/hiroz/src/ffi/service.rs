use std::{
    collections::HashMap,
    ffi::c_char,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use zenoh::{Wait, query::Query};

use super::{
    ErrorCode, cstr_to_str,
    node::{CNode, get_node_ref},
    qos::CQosProfile,
};
use crate::{
    attachment::{Attachment, GidArray},
    queue::BoundedQueue,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct QueryKey {
    pub sn: i64,
    pub gid: GidArray,
}

impl From<Attachment> for QueryKey {
    fn from(a: Attachment) -> Self {
        Self {
            sn: a.sequence_number,
            gid: a.source_gid,
        }
    }
}

/// Callback type for service requests.
/// Called with (user_data, request bytes, request len, out response bytes, out response len).
/// Must return 0 on success, non-zero on error.
///
/// The callback runs on the server's background thread and must not unwind.
/// `request_data` is readable for `request_len` bytes only during the callback;
/// neither it nor the response output pointers may be retained. The output
/// pointers initially contain null and zero. To send a response, set them to a
/// non-null, C `free`-compatible allocation and its nonzero initialized length,
/// no larger than `isize::MAX`, and return zero. This transfers ownership to the
/// server, which reads and frees the allocation after the callback returns.
/// The callback must not access or free it after returning. On any other result,
/// the server neither reads nor frees a response allocation; the callback remains
/// responsible for it. Empty responses are not sent by this callback interface.
///
/// Any state accessed through `user_data` must remain valid until server
/// destruction returns, and access shared with other threads must be synchronized.
/// Do not destroy the server from its callback: destruction joins this thread.
pub type ServiceCallback = extern "C" fn(
    user_data: usize,
    request_data: *const u8,
    request_len: usize,
    response_data: *mut *mut u8,
    response_len: *mut usize,
) -> i32;

/// Raw service client for FFI (no type parameters)
pub struct RawServiceClient {
    pub(crate) sn: AtomicUsize,
    pub(crate) gid: GidArray,
    pub(crate) _key_expr: zenoh::key_expr::KeyExpr<'static>,
    /// Liveliness token — kept alive so that rmw_zenoh_cpp service servers can
    /// observe this client via Zenoh liveliness.
    pub(crate) _lv_token: zenoh::liveliness::LivelinessToken,
    /// Fully qualified service name used by graph lookups.
    pub(crate) qualified_service: String,
    /// Graph reference for readiness checks (wait_for_service).
    pub(crate) graph: Arc<crate::graph::Graph>,
    // Drop the liveliness token and graph before releasing the session runtime.
    // Fields drop in declaration order; this guard must remain last.
    pub(crate) session: Arc<zenoh::Session>,
}

impl RawServiceClient {
    fn new_attachment(&self) -> Attachment {
        Attachment::new(self.sn.fetch_add(1, Ordering::AcqRel) as _, self.gid)
    }

    /// Send a request and wait for the response with a timeout.
    ///
    /// On timeout this returns [`crate::error::Error::Timeout`] so callers (in
    /// particular the C FFI entry points) can map it to a distinct
    /// [`ErrorCode::ServiceTimeout`](crate::ffi::ErrorCode::ServiceTimeout)
    /// instead of a generic failure code.
    pub fn call_raw(
        &self,
        request: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, crate::error::Error> {
        self.call_raw_with_cancel(request, timeout, &AtomicBool::new(false))
    }

    /// Like call_raw, but cancels the actual transport query when requested.
    pub fn call_raw_with_cancel(
        &self,
        request: &[u8],
        timeout: Duration,
        canceled: &AtomicBool,
    ) -> Result<Vec<u8>, crate::error::Error> {
        if canceled.load(Ordering::Acquire) {
            return Err(crate::error::Error::Canceled);
        }
        let pending = self.start_call(request, timeout)?;
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err(crate::error::Error::Canceled);
            }
            if let Some(result) = pending.poll(Duration::from_millis(20))? {
                return Ok(result);
            }
        }
    }

    fn start_call(
        &self,
        request: &[u8],
        timeout: Duration,
    ) -> Result<CServiceCall, crate::error::Error> {
        let (tx, rx) = flume::bounded(1);
        let cancellation = zenoh::cancellation::CancellationToken::default();
        let deadline = Instant::now().checked_add(timeout);
        // SessionGetBuilder accepts a timeout per request. Querier::get instead
        // inherits its declaration's timeout, silently capping long action waits.
        self.session
            .get(self._key_expr.clone())
            .target(zenoh::query::QueryTarget::AllComplete)
            .consolidation(zenoh::query::ConsolidationMode::None)
            .timeout(timeout)
            .payload(request.to_vec())
            .attachment(self.new_attachment())
            .cancellation_token(cancellation.clone())
            .callback(move |reply| {
                let response = reply
                    .into_result()
                    .map(|sample| sample.payload().to_bytes().to_vec())
                    .map_err(|err| {
                        // Zenoh 1.9 represents its timer expiry as an ordinary
                        // error reply, without a typed timeout discriminator.
                        // Use our deadline, never the server-controlled payload,
                        // so polling and the transport timer agree on the result.
                        if deadline.is_some_and(|end| Instant::now() >= end) {
                            crate::error::Error::Timeout(timeout)
                        } else {
                            crate::error::Error::Other(format!("Service reply failed: {err:?}"))
                        }
                    });
                let _ = tx.try_send(response);
            })
            .wait()
            .map_err(|err| crate::error::Error::Other(format!("Failed to send query: {err}")))?;
        Ok(CServiceCall {
            receiver: rx,
            cancellation,
            deadline,
            timeout,
            _session: self.session.clone(),
        })
    }

    /// Block until at least one matching service server is visible in the graph,
    /// or `timeout` elapses. Wakes immediately when a graph change is signaled.
    pub fn wait_for_service(&self, timeout: Duration) -> bool {
        use std::time::Instant;

        use crate::entity::EndpointKind;

        let deadline = Instant::now().checked_add(timeout);
        let (mu, cvar) = &*self.graph.change_signal;
        loop {
            // Hold the condvar mutex while checking the condition and entering wait so
            // that no signal fired between the check and the wait can be missed.
            let guard = mu.lock().unwrap();
            if self
                .graph
                .count_by_service(EndpointKind::Service, &self.qualified_service)
                > 0
            {
                return true;
            }
            let remaining = deadline
                .map(|end| end.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(3600));
            if remaining.is_zero() {
                return false;
            }
            // Atomically releases `guard` and blocks; re-acquires on wake.
            let _ = cvar.wait_timeout(guard, remaining).unwrap();
        }
    }
}

/// Raw service server for FFI (no type parameters)
pub struct RawServiceServer {
    pub(crate) key_expr: zenoh::key_expr::KeyExpr<'static>,
    pub(crate) _inner: zenoh::query::Queryable<()>,
    /// Liveliness token — kept alive as long as the server exists so that
    /// rmw_zenoh_cpp clients can discover this service via Zenoh liveliness.
    pub(crate) _lv_token: zenoh::liveliness::LivelinessToken,
    pub(crate) queue: Arc<BoundedQueue<Query>>,
    pub(crate) map: HashMap<QueryKey, Query>,
    // Keep the runtime alive through queryable/token/query cleanup, including
    // when foreign callers destroy the node/context before this endpoint.
    // Fields drop in declaration order; this guard must remain last.
    pub(crate) _session: Arc<zenoh::Session>,
}

impl RawServiceServer {
    /// Send a response for a previously received request.
    pub(crate) fn send_response_raw(
        &mut self,
        key: &QueryKey,
        response: &[u8],
    ) -> Result<(), String> {
        match self.map.remove(key) {
            Some(query) => {
                let attachment = Attachment::new(key.sn, key.gid);
                query
                    .reply(&self.key_expr, response.to_vec())
                    .attachment(attachment)
                    .wait()
                    .map_err(|e| format!("Failed to send response: {}", e))
            }
            None => Err(format!("No query found for sn={}", key.sn)),
        }
    }
}

/// Opaque service client handle for FFI
#[repr(C)]
pub struct CServiceClient {
    inner: Box<RawServiceClient>,
}

/// Opaque service server handle for FFI
pub struct CServiceServer {
    #[allow(dead_code)]
    server: Arc<std::sync::Mutex<RawServiceServer>>,
    thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

/// Create a service client
///
/// # Safety
/// The requirements of [`hiroz_service_client_create_with_qos`] apply, with
/// default QoS. The response type pointers are unused and may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_create(
    node: *mut CNode,
    service_name: *const c_char,
    req_type_name: *const c_char,
    req_type_hash: *const c_char,
    _resp_type_name: *const c_char,
    _resp_type_hash: *const c_char,
) -> *mut CServiceClient {
    unsafe {
        hiroz_service_client_create_with_qos(
            node,
            service_name,
            req_type_name,
            req_type_hash,
            _resp_type_name,
            _resp_type_hash,
            std::ptr::null(),
        )
    }
}

/// Create a service client with endpoint QoS. Null qos uses the defaults.
///
/// # Safety
/// A non-null `node` must be a live handle returned by a node creation function
/// and must not be destroyed or mutably accessed during this call. The required
/// `service_name`, `req_type_name`, and `req_type_hash` strings must be readable,
/// NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
/// creation to fail. The response type pointers are unused and may be null.
/// Non-null `qos` must point to an aligned, initialized `CQosProfile` that remains
/// readable and unchanged for this call. These inputs are not retained.
/// The caller owns a non-null result and must release it exactly once with
/// [`hiroz_service_client_destroy`]. A null `node` causes creation to fail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_create_with_qos(
    node: *mut CNode,
    service_name: *const c_char,
    req_type_name: *const c_char,
    req_type_hash: *const c_char,
    _resp_type_name: *const c_char,
    _resp_type_hash: *const c_char,
    qos: *const CQosProfile,
) -> *mut CServiceClient {
    unsafe {
        let node_ref = match get_node_ref(node) {
            Some(n) => n,
            None => return std::ptr::null_mut(),
        };

        let service_str = match cstr_to_str(service_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_name_str = match cstr_to_str(req_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_hash_str = match cstr_to_str(req_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        match node_ref.create_raw_service_client_with_qos(
            service_str,
            type_name_str,
            type_hash_str,
            CQosProfile::to_qos_profile(qos),
        ) {
            Ok(raw_client) => Box::into_raw(Box::new(CServiceClient {
                inner: Box::new(raw_client),
            })),
            Err(e) => {
                tracing::warn!("hiroz: Failed to create service client: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Call a service (synchronous with timeout).
/// Response bytes are allocated via Rust and must be freed with hiroz_free_bytes.
///
/// # Safety
/// Non-null `client_handle` must be a live service client handle and must not be
/// destroyed or mutably accessed until this call returns. `request_data` must
/// describe `request_len` initialized bytes in one allocation, with a length no
/// larger than `isize::MAX`, readable and unchanged for the call. It must be
/// non-null even for an empty request. Non-null `response_data` and `response_len`
/// must point to aligned, exclusively writable storage for their respective
/// types, disjoint from each other and all inputs, for the call's duration.
/// Null arguments are rejected. On success the caller owns the returned bytes
/// and must release them once using `hiroz_free_bytes` with the exact returned
/// length. On failure the output values are unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_call(
    client_handle: *mut CServiceClient,
    request_data: *const u8,
    request_len: usize,
    response_data: *mut *mut u8,
    response_len: *mut usize,
    timeout_ms: u64,
) -> i32 {
    if client_handle.is_null()
        || request_data.is_null()
        || response_data.is_null()
        || response_len.is_null()
    {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let client = &(*client_handle);
        let request = std::slice::from_raw_parts(request_data, request_len);
        let timeout = Duration::from_millis(timeout_ms);

        match client.inner.call_raw(request, timeout) {
            Ok(response) => {
                let boxed = response.into_boxed_slice();
                *response_len = boxed.len();
                *response_data = Box::into_raw(boxed) as *mut u8;
                ErrorCode::Success as i32
            }
            Err(crate::error::Error::Timeout(elapsed)) => {
                tracing::warn!("hiroz: Service call timed out after {:?}", elapsed);
                ErrorCode::ServiceTimeout as i32
            }
            Err(e) => {
                tracing::warn!("hiroz: Service call failed: {}", e);
                ErrorCode::ServiceCallFailed as i32
            }
        }
    }
}

/// Destroy a service client
///
/// # Safety
/// A non-null `client` must be an owned service client handle returned by a
/// client creation function and not previously destroyed. No other operation
/// may access this handle during or after destruction. Independent call handles
/// returned by [`hiroz_service_client_start_call`] may remain alive. Null is
/// rejected without releasing anything.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_destroy(client: *mut CServiceClient) -> i32 {
    if client.is_null() {
        return ErrorCode::NullPointer as i32;
    }

    unsafe {
        let _ = Box::from_raw(client);
    }
    ErrorCode::Success as i32
}

/// Wait until at least one matching service server is visible in the graph,
/// or `timeout_ms` elapses. Returns `Success` (0) if ready, `ServiceTimeout`
/// (-10) on timeout, `NullPointer` (-1) if `client` is null.
///
/// # Safety
/// A non-null `client_handle` must be a live service client handle and must not
/// be destroyed or mutably accessed for the entire wait. Null is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_wait_for_service(
    client_handle: *mut CServiceClient,
    timeout_ms: u64,
) -> i32 {
    if client_handle.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    let client = unsafe { &(*client_handle) };
    if client
        .inner
        .wait_for_service(Duration::from_millis(timeout_ms))
    {
        ErrorCode::Success as i32
    } else {
        ErrorCode::ServiceTimeout as i32
    }
}

unsafe extern "C" {
    fn free(ptr: *mut std::ffi::c_void);
}

/// Create a service server.
/// The server spawns a background thread that polls for incoming requests,
/// invokes the callback for each one, and sends the response.
///
/// # Safety
/// The requirements of [`hiroz_service_server_create_with_qos`] apply, with
/// default QoS, including the callback and `user_data` lifetime requirements.
/// The response type pointers are unused and may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_server_create(
    node: *mut CNode,
    service_name: *const c_char,
    req_type_name: *const c_char,
    req_type_hash: *const c_char,
    _resp_type_name: *const c_char,
    _resp_type_hash: *const c_char,
    callback: ServiceCallback,
    user_data: usize,
) -> *mut CServiceServer {
    unsafe {
        hiroz_service_server_create_with_qos(
            node,
            service_name,
            req_type_name,
            req_type_hash,
            _resp_type_name,
            _resp_type_hash,
            callback,
            user_data,
            std::ptr::null(),
        )
    }
}

/// Create a callback service server with endpoint QoS.
///
/// # Safety
/// A non-null `node` must be a live handle returned by a node creation function
/// and must not be destroyed or mutably accessed during this call. The required
/// `service_name`, `req_type_name`, and `req_type_hash` strings must be readable,
/// NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
/// creation to fail. The response type pointers are unused and may be null.
/// `qos` may be null for defaults; otherwise it must point to an aligned,
/// initialized `CQosProfile`, readable and unchanged for this call. The node,
/// strings, and QoS are not retained. A null `node` causes creation to fail.
///
/// `callback` must be a valid function obeying [`ServiceCallback`]'s memory,
/// ownership, and threading contract. It may run before this function returns.
/// The function and any state it accesses through `user_data` must remain valid
/// until [`hiroz_service_server_destroy`] returns, with shared access properly
/// synchronized. The callback must not destroy this server. The caller owns a
/// non-null result and must destroy it exactly once outside the callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_server_create_with_qos(
    node: *mut CNode,
    service_name: *const c_char,
    req_type_name: *const c_char,
    req_type_hash: *const c_char,
    _resp_type_name: *const c_char,
    _resp_type_hash: *const c_char,
    callback: ServiceCallback,
    user_data: usize,
    qos: *const CQosProfile,
) -> *mut CServiceServer {
    unsafe {
        let node_ref = match get_node_ref(node) {
            Some(n) => n,
            None => return std::ptr::null_mut(),
        };

        let service_str = match cstr_to_str(service_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_name_str = match cstr_to_str(req_type_name) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let type_hash_str = match cstr_to_str(req_type_hash) {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };

        let raw_server = match node_ref.create_raw_service_server_with_qos(
            service_str,
            type_name_str,
            type_hash_str,
            CQosProfile::to_qos_profile(qos),
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("hiroz: Failed to create service server: {}", e);
                return std::ptr::null_mut();
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let server_mutex = Arc::new(std::sync::Mutex::new(raw_server));
        let server_mutex_clone = server_mutex.clone();

        let thread = std::thread::spawn(move || {
            while !shutdown_clone.load(Ordering::Relaxed) {
                // Try to receive a query with a short timeout
                let req = {
                    let server = server_mutex_clone.lock().unwrap();
                    server.queue.recv_timeout(Duration::from_millis(100))
                };

                let query = match req {
                    Some(q) => q,
                    None => continue,
                };

                // Extract attachment and payload outside the lock
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

                // Store query in map
                {
                    let mut server = server_mutex_clone.lock().unwrap();
                    if server.map.contains_key(&key) {
                        continue;
                    }
                    server.map.insert(key.clone(), query);
                }

                // Call the C callback (outside the lock)
                let mut resp_ptr: *mut u8 = std::ptr::null_mut();
                let mut resp_len: usize = 0;

                let result = callback(
                    user_data,
                    payload.as_ptr(),
                    payload.len(),
                    &mut resp_ptr,
                    &mut resp_len,
                );

                if result == 0 && !resp_ptr.is_null() && resp_len > 0 {
                    let response = std::slice::from_raw_parts(resp_ptr, resp_len);
                    let mut server = server_mutex_clone.lock().unwrap();
                    let _ = server.send_response_raw(&key, response);
                    // Free the C-allocated response (allocated by Go via C.malloc)
                    free(resp_ptr as *mut std::ffi::c_void);
                } else {
                    // Remove the query from the map on error
                    let mut server = server_mutex_clone.lock().unwrap();
                    server.map.remove(&key);
                }
            }
        });

        Box::into_raw(Box::new(CServiceServer {
            server: server_mutex,
            thread: Some(thread),
            shutdown,
        }))
    }
}

/// Destroy a service server
///
/// # Safety
/// A non-null `server` must be an owned handle returned by a service server
/// creation function and not previously destroyed. No other FFI operation may
/// access this handle during or after destruction. This function must not be
/// called from the server's callback: it waits for the callback thread to exit.
/// Keep the callback and its `user_data` state valid until this function returns.
/// Previously taken request handles remain independently owned and must still
/// be destroyed. Null is rejected without releasing anything.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_server_destroy(server: *mut CServiceServer) -> i32 {
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

/// An independent in-flight query. Destroying it cancels the transport query.
/// The creating client may be destroyed once this handle has been returned.
pub struct CServiceCall {
    receiver: flume::Receiver<Result<Vec<u8>, crate::error::Error>>,
    cancellation: zenoh::cancellation::CancellationToken,
    deadline: Option<Instant>,
    timeout: Duration,
    _session: Arc<zenoh::Session>,
}

impl CServiceCall {
    fn poll(&self, wait: Duration) -> Result<Option<Vec<u8>>, crate::error::Error> {
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(crate::error::Error::Timeout(self.timeout));
        }
        let remaining = self
            .deadline
            .map(|end| end.saturating_duration_since(Instant::now()));
        let wait = remaining.map(|left| left.min(wait)).unwrap_or(wait);
        let response = self.receiver.recv_timeout(wait);
        // A ready queue does not imply that the caller's deadline is still live.
        // Include time spent descheduled before or during the receive.
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(crate::error::Error::Timeout(self.timeout));
        }
        match response {
            Ok(response) => response.map(Some),
            Err(flume::RecvTimeoutError::Disconnected) => {
                Err(crate::error::Error::Timeout(self.timeout))
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                if self.deadline.is_some_and(|end| Instant::now() >= end) {
                    Err(crate::error::Error::Timeout(self.timeout))
                } else {
                    Ok(None)
                }
            }
        }
    }
}

impl Drop for CServiceCall {
    fn drop(&mut self) {
        let _ = self.cancellation.cancel().wait();
    }
}

/// Start a service request without blocking for a response.
/// The caller owns the handle written to out_call and must destroy it, including
/// after a successful poll.
///
/// # Safety
/// Non-null `client` must be a live service client handle and must not be
/// destroyed or mutably accessed during this call. `data` must describe `len`
/// initialized bytes in one allocation, with a length no larger than
/// `isize::MAX`, readable and unchanged for this call. It must be non-null even
/// when `len` is zero. Non-null `out_call` must point to aligned, exclusively
/// writable pointer storage, disjoint from the inputs, for this call. Null
/// arguments are rejected. On success, destroy the returned handle exactly once
/// with [`hiroz_service_call_destroy`]. The request bytes are copied and the
/// returned handle does not borrow the client. Once arguments pass null checks,
/// `out_call` is set to null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_client_start_call(
    client: *mut CServiceClient,
    data: *const u8,
    len: usize,
    timeout_ms: u64,
    out_call: *mut *mut CServiceCall,
) -> i32 {
    if client.is_null() || data.is_null() || out_call.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        *out_call = std::ptr::null_mut();
        match (*client).inner.start_call(
            std::slice::from_raw_parts(data, len),
            Duration::from_millis(timeout_ms),
        ) {
            Ok(call) => {
                *out_call = Box::into_raw(Box::new(call));
                ErrorCode::Success as i32
            }
            Err(_) => ErrorCode::ServiceCallFailed as i32,
        }
    }
}

/// Poll an independent request. Returns 1 while pending, 0 with result, or a negative error.
/// Result bytes are owned by the caller and freed with hiroz_free_bytes.
///
/// # Safety
/// Non-null `call` must be a live handle returned by
/// [`hiroz_service_client_start_call`] and must not be destroyed or mutably
/// accessed during this call. Non-null `data` and `len` must point to aligned,
/// exclusively writable storage for their respective types, disjoint from each
/// other and the handle, for the entire poll. Null arguments are rejected.
/// After null checks, the outputs are cleared before polling. On success, the
/// caller must release the returned bytes once with `hiroz_free_bytes`, using
/// the exact returned length. Polling does not release the call handle; it must
/// still be destroyed separately.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_call_poll(
    call: *mut CServiceCall,
    wait_ms: u64,
    data: *mut *mut u8,
    len: *mut usize,
) -> i32 {
    if call.is_null() || data.is_null() || len.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        *data = std::ptr::null_mut();
        *len = 0;
        match (*call).poll(Duration::from_millis(wait_ms)) {
            Ok(Some(bytes)) => {
                let bytes = bytes.into_boxed_slice();
                *len = bytes.len();
                *data = Box::into_raw(bytes) as *mut u8;
                0
            }
            Ok(None) => 1,
            Err(crate::error::Error::Timeout(_)) => ErrorCode::ServiceTimeout as i32,
            Err(_) => ErrorCode::ServiceCallFailed as i32,
        }
    }
}

/// Cancel and release a pending/completed request. Never call concurrently with poll.
///
/// # Safety
/// A non-null `call` must be an owned handle returned by
/// [`hiroz_service_client_start_call`] and not previously destroyed. No other
/// operation may access it during or after destruction. Null is rejected without
/// releasing anything. Previously returned result bytes remain caller-owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_call_destroy(call: *mut CServiceCall) -> i32 {
    if call.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        drop(Box::from_raw(call));
    }
    0
}

/// Correlation information attached to a service request.
#[repr(C)]
pub struct CServiceRequestId {
    pub sequence_number: i64,
    pub writer_gid: [u8; 16],
}

/// A request owns its query independently of the server that received it.
pub struct CServiceRequest {
    query: Query,
    key: QueryKey,
}

/// Create a queued service server, for deferred replies and application worker pools.
///
/// # Safety
/// A non-null `node` must be a live handle returned by a node creation function
/// and must not be destroyed or mutably accessed during this call. The required
/// `service`, `type_name`, and `type_hash` strings must be readable,
/// NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
/// creation to fail. `qos` may be null for defaults; otherwise it must point to
/// an aligned, initialized `CQosProfile`, readable and unchanged for this call.
/// These inputs are not retained. The caller owns a non-null result and must
/// release it exactly once with [`hiroz_service_server_destroy`]. A null `node`
/// causes creation to fail.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_server_create_queued(
    node: *mut CNode,
    service: *const c_char,
    type_name: *const c_char,
    type_hash: *const c_char,
    qos: *const CQosProfile,
) -> *mut CServiceServer {
    unsafe {
        let Some(node) = get_node_ref(node) else {
            return std::ptr::null_mut();
        };
        let (Ok(service), Ok(type_name), Ok(type_hash)) = (
            cstr_to_str(service),
            cstr_to_str(type_name),
            cstr_to_str(type_hash),
        ) else {
            return std::ptr::null_mut();
        };
        match node.create_raw_service_server_with_qos(
            service,
            type_name,
            type_hash,
            CQosProfile::to_qos_profile(qos),
        ) {
            Ok(server) => Box::into_raw(Box::new(CServiceServer {
                server: Arc::new(std::sync::Mutex::new(server)),
                thread: None,
                shutdown: Arc::new(AtomicBool::new(false)),
            })),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// Take one queued request. Returns 1 if no request arrives within wait_ms.
/// The request and its copied bytes are independently caller-owned.
///
/// # Safety
/// Non-null `server` must be a live service server handle and must not be
/// destroyed or mutably accessed during the entire wait. Only queued servers
/// support this operation; callback servers are rejected. Every non-null output
/// pointer must address aligned, exclusively writable storage for its declared
/// type, disjoint from the other outputs and the server, throughout this call.
/// Null arguments are rejected. After null checks, `request`, `data`, and `len`
/// are cleared; `id` is written only on success. On success the caller must
/// release the request exactly once with [`hiroz_service_request_destroy`] and
/// the copied bytes once with `hiroz_free_bytes`, using the exact returned
/// length. Neither allocation borrows the other or requires the server handle
/// to remain alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_server_take_request(
    server: *mut CServiceServer,
    wait_ms: u64,
    request: *mut *mut CServiceRequest,
    data: *mut *mut u8,
    len: *mut usize,
    id: *mut CServiceRequestId,
) -> i32 {
    if server.is_null() || request.is_null() || data.is_null() || len.is_null() || id.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        *request = std::ptr::null_mut();
        *data = std::ptr::null_mut();
        *len = 0;
        let server = &*server;
        // Callback servers own their consumer; never compete with that thread.
        if server.thread.is_some() {
            return ErrorCode::ServiceCallFailed as i32;
        }
        let queue = server.server.lock().unwrap().queue.clone();
        let Some(query) = queue.recv_timeout(Duration::from_millis(wait_ms)) else {
            return 1;
        };
        let Some(attachment) = query
            .attachment()
            .and_then(|a| Attachment::try_from(a).ok())
        else {
            return ErrorCode::DeserializationFailed as i32;
        };
        let Some(payload) = query.payload() else {
            return ErrorCode::DeserializationFailed as i32;
        };
        let bytes = payload.to_bytes().to_vec().into_boxed_slice();
        *len = bytes.len();
        *data = Box::into_raw(bytes) as *mut u8;
        *id = CServiceRequestId {
            sequence_number: attachment.sequence_number,
            writer_gid: attachment.source_gid,
        };
        *request = Box::into_raw(Box::new(CServiceRequest {
            query,
            key: attachment.into(),
        }));
        0
    }
}

/// Reply to a deferred request. Caller must still destroy the request handle.
///
/// # Safety
/// Non-null `request` must be a live handle returned by
/// [`hiroz_service_server_take_request`] and must not be destroyed or mutably
/// accessed during this call. `data` must describe `len` initialized bytes in
/// one allocation, with a length no larger than `isize::MAX`, readable and
/// unchanged for this call. It must be non-null even when `len` is zero. Null
/// arguments are rejected. The bytes are copied; ownership of the input buffer
/// and request handle remains with the caller, including on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_request_reply(
    request: *mut CServiceRequest,
    data: *const u8,
    len: usize,
) -> i32 {
    if request.is_null() || data.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        let request = &*request;
        let bytes = std::slice::from_raw_parts(data, len).to_vec();
        match request
            .query
            .reply(request.query.key_expr(), bytes)
            .attachment(Attachment::new(request.key.sn, request.key.gid))
            .wait()
        {
            Ok(()) => 0,
            Err(_) => ErrorCode::ServiceCallFailed as i32,
        }
    }
}

/// Release an answered or abandoned deferred request.
///
/// # Safety
/// A non-null `request` must be an owned handle returned by
/// [`hiroz_service_server_take_request`] and not previously destroyed. No other
/// operation may access it during or after destruction. Null is rejected without
/// releasing anything. This does not release the separately returned request
/// bytes, which remain caller-owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hiroz_service_request_destroy(request: *mut CServiceRequest) -> i32 {
    if request.is_null() {
        return ErrorCode::NullPointer as i32;
    }
    unsafe {
        drop(Box::from_raw(request));
    }
    0
}
