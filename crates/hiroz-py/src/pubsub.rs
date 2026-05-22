use crate::error::IntoPyErr;
use crate::payload_view::ZPayloadView;
use crate::traits::{RawPublisher, RawSubscriber};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::time::Duration;
use zenoh_buffers::buffer::SplitBuffer;

#[pyclass(name = "ZPublisher")]
pub struct PyZPublisher {
    inner: Box<dyn RawPublisher>,
    type_name: String,
}

impl PyZPublisher {
    pub fn new(inner: Box<dyn RawPublisher>, type_name: String) -> Self {
        Self { inner, type_name }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZPublisher {
    /// Publish a message
    ///
    /// Serializes the Python message (msgspec.Struct) to ZBuf and publishes (zero-copy path)
    unsafe fn publish(&self, _py: Python, data: &Bound<'_, PyAny>) -> PyResult<()> {
        // Serialize Python message directly to ZBuf (zero-copy)
        let zbuf = hiroz_msgs::serialize_to_zbuf(&self.type_name, data)?;

        // Publish the ZBuf directly
        self.inner.publish(zbuf.into()).map_err(|e| e.into_pyerr())
    }

    /// Publish pre-serialized CDR bytes directly
    ///
    /// Use this for zero-copy forwarding of received messages (e.g., in a pong responder).
    /// The bytes should be in CDR format (as returned by recv_serialized/try_recv_serialized).
    fn publish_raw(&self, data: &[u8]) -> PyResult<()> {
        self.inner.publish(data.into()).map_err(|e| e.into_pyerr())
    }

    /// Get the topic name (for debugging)
    unsafe fn get_type_name(&self) -> String {
        self.type_name.clone()
    }
}

#[pyclass(name = "ZSubscriber")]
pub struct PyZSubscriber {
    /// None for callback-based subscriptions (no queue)
    inner: Option<Box<dyn RawSubscriber>>,
    type_name: String,
    /// ID used to locate this subscriber in the node's owned_subs for destroy_subscriber.
    /// None for queue-based subscribers (caller owns the handle directly).
    pub(crate) owned_id: Option<u64>,
}

impl PyZSubscriber {
    pub fn new(inner: Box<dyn RawSubscriber>, type_name: String) -> Self {
        Self {
            inner: Some(inner),
            type_name,
            owned_id: None,
        }
    }

    /// Create a callback-based subscriber (no queue).
    /// The subscriber handle is owned by the node under `id`.
    pub fn new_callback(type_name: String, id: u64) -> Self {
        Self {
            inner: None,
            type_name,
            owned_id: Some(id),
        }
    }

    fn require_queue(&self) -> PyResult<&dyn RawSubscriber> {
        self.inner.as_deref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "Cannot recv on a callback-based subscriber. Messages are delivered via the callback.",
            )
        })
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZSubscriber {
    /// Receive the next message (blocking)
    ///
    /// Args:
    ///     timeout: Optional timeout in seconds (None = block forever)
    ///
    /// Returns:
    ///     Message as dict, or None if timeout occurred
    #[pyo3(signature = (timeout=None))]
    unsafe fn recv(&self, py: Python, timeout: Option<f64>) -> PyResult<Option<PyObject>> {
        let inner = self.require_queue()?;
        let timeout_duration = timeout.map(Duration::from_secs_f64);

        // Release GIL while waiting to allow other Python threads to run
        let result = py.allow_threads(|| inner.recv_sample(timeout_duration));

        match result {
            Ok(sample) => {
                // Convert payload to ZBuf for zero-copy deserialization (cheap Arc clones)
                let payload_zbuf: zenoh_buffers::ZBuf = sample.payload().clone().into();
                // Set source for zero-copy sub-ZSlice creation during deserialization
                hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
                    *cell.borrow_mut() = Some(payload_zbuf.clone());
                });
                let cow = payload_zbuf.contiguous();
                let result = hiroz_msgs::deserialize_from_cdr(&self.type_name, py, &cow);
                hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
                    *cell.borrow_mut() = None;
                });
                Ok(Some(result?))
            }
            Err(e) => {
                // Check if it's a timeout error
                let err_str = e.to_string();
                if err_str.contains("timeout")
                    || err_str.contains("Timeout")
                    || err_str.contains("timed out")
                {
                    Ok(None)
                } else {
                    Err(e.into_pyerr())
                }
            }
        }
    }

    /// Try to receive a message without blocking
    ///
    /// Returns:
    ///     Message as dict, or None if no message available
    unsafe fn try_recv(&self, py: Python) -> PyResult<Option<PyObject>> {
        let inner = self.require_queue()?;
        match inner.try_recv_sample().map_err(|e| e.into_pyerr())? {
            Some(sample) => {
                let payload_zbuf: zenoh_buffers::ZBuf = sample.payload().clone().into();
                hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
                    *cell.borrow_mut() = Some(payload_zbuf.clone());
                });
                let cow = payload_zbuf.contiguous();
                let result = hiroz_msgs::deserialize_from_cdr(&self.type_name, py, &cow);
                hiroz_cdr::ZBUF_DESER_SOURCE.with(|cell| {
                    *cell.borrow_mut() = None;
                });
                Ok(Some(result?))
            }
            None => Ok(None),
        }
    }

    /// Receive serialized bytes (for testing/advanced use)
    ///
    /// Returns bytes object containing the CDR-serialized message.
    #[pyo3(signature = (timeout=None))]
    unsafe fn recv_serialized(
        &self,
        py: Python,
        timeout: Option<f64>,
    ) -> PyResult<Option<Py<PyBytes>>> {
        let inner = self.require_queue()?;
        let timeout_duration = timeout.map(Duration::from_secs_f64);

        // Release GIL while waiting to allow other Python threads to run
        let result = py.allow_threads(|| inner.recv_serialized(timeout_duration));

        match result {
            Ok(data) => Ok(Some(PyBytes::new_bound(py, &data).into())),
            Err(e) => {
                // Check if it's a timeout error
                let err_str = e.to_string();
                if err_str.contains("timeout")
                    || err_str.contains("Timeout")
                    || err_str.contains("timed out")
                {
                    Ok(None)
                } else {
                    Err(e.into_pyerr())
                }
            }
        }
    }

    /// Try to receive serialized bytes without blocking
    ///
    /// Returns bytes object containing the CDR-serialized message, or None.
    unsafe fn try_recv_serialized(&self, py: Python) -> PyResult<Option<Py<PyBytes>>> {
        let inner = self.require_queue()?;
        match inner.try_recv_serialized().map_err(|e| e.into_pyerr())? {
            Some(data) => Ok(Some(PyBytes::new_bound(py, &data).into())),
            None => Ok(None),
        }
    }

    /// Receive raw payload as a zero-copy buffer view (blocking)
    ///
    /// Returns a ZPayloadView that implements Python's buffer protocol.
    /// Use memoryview() or numpy.frombuffer() for zero-copy access:
    ///
    /// ```python
    /// payload = subscriber.recv_raw_view()
    /// mv = memoryview(payload)  # Zero-copy view!
    /// # Or with numpy:
    /// arr = np.frombuffer(payload, dtype=np.uint8)
    /// ```
    ///
    /// Args:
    ///     timeout: Optional timeout in seconds (None = block forever)
    ///
    /// Returns:
    ///     ZPayloadView, or None if timeout occurred
    #[pyo3(signature = (timeout=None))]
    unsafe fn recv_raw_view(&self, py: Python, timeout: Option<f64>) -> PyResult<Option<PyObject>> {
        let inner = self.require_queue()?;
        let timeout_duration = timeout.map(Duration::from_secs_f64);

        // Release GIL while waiting to allow other Python threads to run
        let result = py.allow_threads(|| inner.recv_sample(timeout_duration));

        match result {
            Ok(sample) => {
                let view = ZPayloadView::new(sample);
                Ok(Some(Py::new(py, view)?.into_any()))
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("timeout")
                    || err_str.contains("Timeout")
                    || err_str.contains("timed out")
                {
                    Ok(None)
                } else {
                    Err(e.into_pyerr())
                }
            }
        }
    }

    /// Try to receive raw payload as a zero-copy buffer view without blocking
    ///
    /// Returns:
    ///     ZPayloadView, or None if no message available
    unsafe fn try_recv_raw_view(&self, py: Python) -> PyResult<Option<PyObject>> {
        let inner = self.require_queue()?;
        match inner.try_recv_sample().map_err(|e| e.into_pyerr())? {
            Some(sample) => {
                let view = ZPayloadView::new(sample);
                Ok(Some(Py::new(py, view)?.into_any()))
            }
            None => Ok(None),
        }
    }

    /// Whether this is a callback-based subscriber (no recv methods)
    #[getter]
    fn is_callback(&self) -> bool {
        self.inner.is_none()
    }

    /// Get the type name (for debugging)
    unsafe fn get_type_name(&self) -> String {
        self.type_name.clone()
    }
}
