use super::node::{CNode, get_node_ref};
use super::{ErrorCode, cstr_to_str};
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use zenoh::Wait;
use zenoh::query::Query;

use crate::attachment::{Attachment, GidArray};
use crate::queue::BoundedQueue;

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
    pub(crate) inner: zenoh::query::Querier<'static>,
    pub(crate) tx: flume::Sender<zenoh::sample::Sample>,
    pub(crate) rx: flume::Receiver<zenoh::sample::Sample>,
    pub(crate) _key_expr: zenoh::key_expr::KeyExpr<'static>,
    /// Liveliness token — kept alive so that rmw_zenoh_cpp service servers can
    /// observe this client via Zenoh liveliness.
    pub(crate) _lv_token: zenoh::liveliness::LivelinessToken,
    /// Fully qualified service name used by graph lookups.
    pub(crate) qualified_service: String,
    /// Graph reference for readiness checks (wait_for_service).
    pub(crate) graph: Arc<crate::graph::Graph>,
}

impl RawServiceClient {
    fn new_attachment(&self) -> Attachment {
        Attachment::new(self.sn.fetch_add(1, Ordering::AcqRel) as _, self.gid)
    }

    /// Send a request and wait for the response with a timeout.
    pub fn call_raw(&self, request: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
        let tx = self.tx.clone();
        let attachment = self.new_attachment();

        self.inner
            .get()
            .payload(request.to_vec())
            .attachment(attachment)
            .callback(move |reply| match reply.into_result() {
                Ok(sample) => {
                    let _ = tx.try_send(sample);
                }
                Err(e) => {
                    tracing::warn!("[FFI-CLN] Reply error: {:?}", e);
                }
            })
            .wait()
            .map_err(|e| format!("Failed to send query: {}", e))?;

        let sample = self
            .rx
            .recv_timeout(timeout)
            .map_err(|e| format!("Timeout waiting for response: {}", e))?;

        Ok(sample.payload().to_bytes().to_vec())
    }

    /// Block until at least one matching service server is visible in the graph,
    /// or `timeout` elapses. Polls the graph at a short interval — does not
    /// require a tokio runtime.
    pub fn wait_for_service(&self, timeout: Duration) -> bool {
        use crate::entity::EndpointKind;
        use std::time::Instant;

        let poll = Duration::from_millis(50);
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .graph
                .count_by_service(EndpointKind::Service, &self.qualified_service)
                > 0
            {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(poll));
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_client_create(
    node: *mut CNode,
    service_name: *const c_char,
    req_type_name: *const c_char,
    req_type_hash: *const c_char,
    _resp_type_name: *const c_char,
    _resp_type_hash: *const c_char,
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

        match node_ref.create_raw_service_client(service_str, type_name_str, type_hash_str) {
            Ok(raw_client) => Box::into_raw(Box::new(CServiceClient {
                inner: Box::new(raw_client),
            })),
            Err(e) => {
                tracing::warn!("ros-z: Failed to create service client: {}", e);
                std::ptr::null_mut()
            }
        }
    }
}

/// Call a service (synchronous with timeout).
/// Response bytes are allocated via Rust and must be freed with ros_z_free_bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_client_call(
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
            Err(e) => {
                tracing::warn!("ros-z: Service call failed: {}", e);
                -1
            }
        }
    }
}

/// Destroy a service client
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_client_destroy(client: *mut CServiceClient) -> i32 {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_client_wait_for_service(
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_server_create(
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

        let raw_server =
            match node_ref.create_raw_service_server(service_str, type_name_str, type_hash_str) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("ros-z: Failed to create service server: {}", e);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ros_z_service_server_destroy(server: *mut CServiceServer) -> i32 {
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
