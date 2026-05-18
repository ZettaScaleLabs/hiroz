use crate::traits::{RawClient, RawServer};
use hiroz::service::RequestId;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::time::Duration;

/// Python wrapper for service client
#[pyclass(name = "ZClient")]
pub struct PyZClient {
    inner: Box<dyn RawClient>,
    request_type_name: String,
    response_type_name: String,
}

impl PyZClient {
    pub fn new(inner: Box<dyn RawClient>, service_type: String) -> Self {
        let request_type_name = format!("{}_Request", service_type);
        let response_type_name = format!("{}_Response", service_type);
        Self {
            inner,
            request_type_name,
            response_type_name,
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
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        hiroz_msgs::deserialize_from_cdr(&self.response_type_name, py, &cdr_bytes)
    }

    /// Get the service type name (for debugging)
    unsafe fn get_type_name(&self) -> String {
        format!(
            "request={}, response={}",
            self.request_type_name, self.response_type_name
        )
    }
}

/// Python wrapper for service server
#[pyclass(name = "ZServer")]
pub struct PyZServer {
    inner: std::sync::Mutex<Box<dyn RawServer>>,
    request_type_name: String,
    response_type_name: String,
}

impl PyZServer {
    pub fn new(inner: Box<dyn RawServer>, service_type: String) -> Self {
        let request_type_name = format!("{}_Request", service_type);
        let response_type_name = format!("{}_Response", service_type);
        Self {
            inner: std::sync::Mutex::new(inner),
            request_type_name,
            response_type_name,
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZServer {
    /// Receive the next service request (blocking)
    unsafe fn take_request(&self, py: Python) -> PyResult<(PyObject, PyObject)> {
        let result = py.allow_threads(|| {
            let inner = self
                .inner
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

        py.allow_threads(|| {
            let inner = self
                .inner
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
