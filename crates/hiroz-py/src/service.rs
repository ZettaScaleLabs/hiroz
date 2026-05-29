use crate::traits::{RawClient, RawServer};
use hiroz::graph::Graph;
use hiroz::service::RequestId;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Python wrapper for service client
#[pyclass(name = "ZClient")]
pub struct PyZClient {
    inner: Box<dyn RawClient>,
    request_type_name: String,
    response_type_name: String,
    /// Shared graph + fully-qualified service name, used by `wait_for_service`.
    graph: Arc<Graph>,
    service_name: String,
}

impl PyZClient {
    pub fn new(
        inner: Box<dyn RawClient>,
        service_type: String,
        graph: Arc<Graph>,
        service_name: String,
    ) -> Self {
        let request_type_name = format!("{}_Request", service_type);
        let response_type_name = format!("{}_Response", service_type);
        Self {
            inner,
            request_type_name,
            response_type_name,
            graph,
            service_name,
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZClient {
    /// Call a service request and wait for its response.
    #[pyo3(signature = (data, timeout=None))]
    unsafe fn call(
        &self,
        py: Python,
        data: &Bound<'_, PyAny>,
        timeout: Option<f64>,
    ) -> PyResult<PyObject> {
        let cdr_bytes = hiroz_msgs::serialize_to_cdr(&self.request_type_name, data.py(), data)?;
        let timeout_duration = timeout.map(Duration::from_secs_f64);

        let cdr_bytes = py
            .allow_threads(|| self.inner.call_serialized(&cdr_bytes, timeout_duration))
            .map_err(crate::error::map_call_error)?;
        hiroz_msgs::deserialize_from_cdr(&self.response_type_name, py, &cdr_bytes)
    }

    /// Wait until a service server for this service is available.
    ///
    /// Mirrors rclpy's `Client.wait_for_service(timeout_sec)`. Polls the
    /// discovery graph until a matching server appears. Returns True if a
    /// server was found before `timeout`, False otherwise.
    ///
    /// Args:
    ///     timeout: Maximum seconds to wait. None waits forever.
    #[pyo3(signature = (timeout=None))]
    fn wait_for_service(&self, py: Python, timeout: Option<f64>) -> bool {
        py.allow_threads(|| {
            crate::graph::wait_for_service_server(&self.graph, &self.service_name, timeout)
        })
    }

    /// Get the service type name (for debugging)
    unsafe fn get_type_name(&self) -> String {
        format!(
            "request={}, response={}",
            self.request_type_name, self.response_type_name
        )
    }
}

/// Background-thread state for a callback-mode server (P6).
///
/// Holds an `Arc` to the underlying server (keeping its Zenoh queryable alive)
/// and a stop flag the worker thread checks each poll. Dropping this signals the
/// thread to stop and joins it.
struct CallbackServerState {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    _server: Arc<dyn RawServer>,
}

impl Drop for CallbackServerState {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Python wrapper for service server.
///
/// Pull mode (default): `inner` is `Some`; the caller drives `take_request` /
/// `send_response`. Callback mode (P6): `inner` is `None` and a background
/// thread (held in `_callback`) services requests via the user callback.
#[pyclass(name = "ZServer")]
pub struct PyZServer {
    inner: Option<std::sync::Mutex<Box<dyn RawServer>>>,
    request_type_name: String,
    response_type_name: String,
    _callback: Option<CallbackServerState>,
}

impl PyZServer {
    pub fn new(inner: Box<dyn RawServer>, service_type: String) -> Self {
        let request_type_name = format!("{}_Request", service_type);
        let response_type_name = format!("{}_Response", service_type);
        Self {
            inner: Some(std::sync::Mutex::new(inner)),
            request_type_name,
            response_type_name,
            _callback: None,
        }
    }

    /// Build a callback-mode server: a background thread receives each request,
    /// calls `callback(request)`, and sends the returned object as the response.
    pub fn new_with_callback(
        server: Arc<dyn RawServer>,
        service_type: String,
        callback: PyObject,
    ) -> Self {
        let request_type_name = format!("{}_Request", service_type);
        let response_type_name = format!("{}_Response", service_type);

        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_callback_loop(
            Arc::clone(&server),
            request_type_name.clone(),
            response_type_name.clone(),
            callback,
            Arc::clone(&stop),
        );

        Self {
            inner: None,
            request_type_name,
            response_type_name,
            _callback: Some(CallbackServerState {
                stop,
                handle: Some(handle),
                _server: server,
            }),
        }
    }

    fn require_pull(&self) -> PyResult<&std::sync::Mutex<Box<dyn RawServer>>> {
        self.inner.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "This server runs in callback mode; take_request/send_response are unavailable. \
                 Create it without a callback to use pull mode.",
            )
        })
    }
}

/// Spawn the worker thread for a callback-mode server.
fn spawn_callback_loop(
    server: Arc<dyn RawServer>,
    request_type_name: String,
    response_type_name: String,
    callback: PyObject,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            // Poll for a request without holding the GIL.
            match server.try_take_request_serialized() {
                Ok(Some((request_id, request_bytes))) => {
                    Python::with_gil(|py| {
                        let req_obj = match hiroz_msgs::deserialize_from_cdr(
                            &request_type_name,
                            py,
                            &request_bytes,
                        ) {
                            Ok(o) => o,
                            Err(e) => {
                                eprintln!("hiroz_py: request deserialize error: {}", e);
                                return;
                            }
                        };
                        let resp_obj = match callback.call1(py, (req_obj,)) {
                            Ok(o) => o,
                            Err(e) => {
                                eprintln!("hiroz_py: service callback error: {}", e);
                                return;
                            }
                        };
                        let resp_bytes = match hiroz_msgs::serialize_to_cdr(
                            &response_type_name,
                            py,
                            resp_obj.bind(py),
                        ) {
                            Ok(b) => b,
                            Err(e) => {
                                eprintln!("hiroz_py: response serialize error: {}", e);
                                return;
                            }
                        };
                        if let Err(e) = server.send_response_serialized(&resp_bytes, &request_id) {
                            eprintln!("hiroz_py: send_response error: {}", e);
                        }
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(e) => {
                    eprintln!("hiroz_py: service poll error: {}", e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZServer {
    /// Receive the next service request (blocking)
    unsafe fn take_request(&self, py: Python) -> PyResult<(PyObject, PyObject)> {
        let mutex = self.require_pull()?;
        let result = py.allow_threads(|| {
            let inner = mutex
                .lock()
                .map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
            inner.take_request_serialized()
        });

        let (key, cdr_bytes) =
            result.map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let obj = hiroz_msgs::deserialize_from_cdr(&self.request_type_name, py, &cdr_bytes)?;

        let request_id = PyDict::new_bound(py);
        request_id.set_item("sn", key.sequence_number)?;
        request_id.set_item("gid", key.writer_guid.to_vec())?;

        Ok((request_id.into(), obj))
    }

    /// Send a response to a service request
    unsafe fn send_response(
        &self,
        py: Python,
        response: &Bound<'_, PyAny>,
        request_id: &Bound<'_, PyDict>,
    ) -> PyResult<()> {
        let cdr_bytes =
            hiroz_msgs::serialize_to_cdr(&self.response_type_name, response.py(), response)?;

        let sn: i64 = request_id.get_item("sn")?.unwrap().extract()?;
        let gid_vec: Vec<u8> = request_id.get_item("gid")?.unwrap().extract()?;
        let mut gid = [0u8; 16];
        gid.copy_from_slice(&gid_vec[..16]);
        // source_timestamp is excluded from RequestId Hash/Eq — 0 is the correct sentinel;
        // the pending map lookup succeeds regardless of the original timestamp.
        let key = RequestId {
            sequence_number: sn,
            writer_guid: gid,
            source_timestamp: 0,
        };

        let mutex = self.require_pull()?;
        py.allow_threads(|| {
            let inner = mutex
                .lock()
                .map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;
            inner.send_response_serialized(&cdr_bytes, &key)
        })
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    /// Get the service type name (for debugging)
    unsafe fn get_type_name(&self) -> String {
        format!(
            "request={}, response={}",
            self.request_type_name, self.response_type_name
        )
    }
}
