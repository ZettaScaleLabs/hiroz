use crate::error::IntoPyErr;
use crate::node::PyZNodeBuilder;
use hiroz::Builder;
use hiroz::context::{ZContext, ZContextBuilder};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use std::sync::Arc;

/// Read `ZENOH_SHM_ALLOC_SIZE` env var (bytes). Mirrors rmw_zenoh_cpp behavior.
fn read_shm_alloc_size() -> Option<usize> {
    std::env::var("ZENOH_SHM_ALLOC_SIZE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
}

/// Read `ZENOH_SHM_MESSAGE_SIZE_THRESHOLD` env var (bytes). Must be a power of two.
/// Mirrors rmw_zenoh_cpp behavior.
fn read_shm_threshold() -> Option<usize> {
    std::env::var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0 && n.is_power_of_two())
}

#[pyclass(name = "ZContextBuilder")]
#[derive(Default)]
pub struct PyZContextBuilder {
    builder: ZContextBuilder,
}

#[pymethods]
impl PyZContextBuilder {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    /// Set the ROS domain ID
    pub fn with_domain_id(mut slf: PyRefMut<'_, Self>, domain_id: usize) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_domain_id(domain_id);
        slf
    }

    /// Set the default namespace inherited by nodes created from this context.
    pub fn with_namespace(mut slf: PyRefMut<'_, Self>, namespace: String) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_namespace(namespace);
        slf
    }

    /// Enable Zenoh logging initialization with default level "error"
    pub fn with_logging_enabled(mut slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_logging_enabled();
        slf
    }

    /// Connect to specific Zenoh endpoints (e.g., "tcp/127.0.0.1:7447")
    pub fn with_connect_endpoints(
        mut slf: PyRefMut<'_, Self>,
        endpoints: Vec<String>,
    ) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_connect_endpoints(endpoints);
        slf
    }

    /// Disable multicast scouting (useful for isolated tests)
    pub fn disable_multicast_scouting(mut slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).disable_multicast_scouting();
        slf
    }

    /// Set Zenoh mode: "peer", "client", or "router"
    pub fn with_mode(mut slf: PyRefMut<'_, Self>, mode: String) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_mode(mode);
        slf
    }

    /// Connect to a router at the given endpoint (e.g., "tcp/192.168.1.1:7447")
    pub fn with_router_endpoint(
        mut slf: PyRefMut<'_, Self>,
        endpoint: String,
    ) -> PyResult<PyRefMut<'_, Self>> {
        slf.builder = std::mem::take(&mut slf.builder)
            .with_router_endpoint(endpoint)
            .map_err(|e| e.into_pyerr())?;
        Ok(slf)
    }

    /// Load Zenoh configuration from a file
    pub fn with_config_file(mut slf: PyRefMut<'_, Self>, path: String) -> PyRefMut<'_, Self> {
        slf.builder =
            std::mem::take(&mut slf.builder).with_config_file(std::path::PathBuf::from(path));
        slf
    }

    /// Add a JSON config override (e.g., key="transport/link/tx/sequence_number_resolution", value="256")
    pub fn with_json(
        mut slf: PyRefMut<'_, Self>,
        key: String,
        value: String,
    ) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_json(key, value);
        slf
    }

    /// Add a name remap rule (format: "from:=to")
    pub fn with_remap_rule(
        mut slf: PyRefMut<'_, Self>,
        rule: String,
    ) -> PyResult<PyRefMut<'_, Self>> {
        slf.builder = std::mem::take(&mut slf.builder)
            .with_remap_rule(rule)
            .map_err(|e| e.into_pyerr())?;
        Ok(slf)
    }

    /// Add multiple name remap rules (format: ["from1:=to1", "from2:=to2"])
    pub fn with_remap_rules(
        mut slf: PyRefMut<'_, Self>,
        rules: Vec<String>,
    ) -> PyResult<PyRefMut<'_, Self>> {
        slf.builder = std::mem::take(&mut slf.builder)
            .with_remap_rules(rules)
            .map_err(|e| e.into_pyerr())?;
        Ok(slf)
    }

    /// Set the security enclave name
    pub fn with_enclave(mut slf: PyRefMut<'_, Self>, enclave: String) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_enclave(enclave);
        slf
    }

    /// Connect to a local zenohd router (equivalent to with_connect_endpoints(["tcp/localhost:7447"]))
    pub fn connect_to_local_zenohd(mut slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).connect_to_local_zenohd();
        slf
    }

    /// Enable Zenoh shared memory transport with default pool size (48 MiB).
    ///
    /// SHM transparently accelerates large message transfers between nodes on the same
    /// host. Both publisher and subscriber must have SHM enabled. Also enable SHM on
    /// the router if messages must be routed.
    ///
    /// Pool size and threshold can be overridden via env vars ``ZENOH_SHM_ALLOC_SIZE``
    /// and ``ZENOH_SHM_MESSAGE_SIZE_THRESHOLD`` (must be power of two), matching
    /// rmw_zenoh_cpp behavior.
    pub fn with_shm_enabled(mut slf: PyRefMut<'_, Self>) -> PyResult<PyRefMut<'_, Self>> {
        let pool_size = read_shm_alloc_size();
        let threshold = read_shm_threshold();

        let builder = std::mem::take(&mut slf.builder);
        let builder = match pool_size {
            Some(size) => builder
                .with_shm_pool_size(size)
                .map_err(|e| e.into_pyerr())?,
            None => builder.with_shm_enabled().map_err(|e| e.into_pyerr())?,
        };
        let builder = match threshold {
            Some(t) => builder.with_shm_threshold(t),
            None => builder,
        };
        slf.builder = builder;
        Ok(slf)
    }

    /// Enable Zenoh shared memory transport with a custom pool size in bytes.
    ///
    /// ``ZENOH_SHM_MESSAGE_SIZE_THRESHOLD`` env var still applies if set.
    pub fn with_shm_pool_size(
        mut slf: PyRefMut<'_, Self>,
        size_bytes: usize,
    ) -> PyResult<PyRefMut<'_, Self>> {
        let threshold = read_shm_threshold();
        let builder = std::mem::take(&mut slf.builder)
            .with_shm_pool_size(size_bytes)
            .map_err(|e| e.into_pyerr())?;
        slf.builder = match threshold {
            Some(t) => builder.with_shm_threshold(t),
            None => builder,
        };
        Ok(slf)
    }

    /// Set the minimum message size (bytes) for SHM transport.
    ///
    /// Messages smaller than this threshold are sent via network. Only effective
    /// after ``with_shm_enabled()`` or ``with_shm_pool_size()``.
    pub fn with_shm_threshold(mut slf: PyRefMut<'_, Self>, threshold: usize) -> PyRefMut<'_, Self> {
        slf.builder = std::mem::take(&mut slf.builder).with_shm_threshold(threshold);
        slf
    }

    /// Build the context
    pub fn build(&mut self) -> PyResult<PyZContext> {
        let builder = std::mem::take(&mut self.builder);
        let ctx = builder.build().map_err(|e| e.into_pyerr())?;
        Ok(PyZContext { ctx: Arc::new(ctx) })
    }
}

#[pyclass(name = "ZContext")]
pub struct PyZContext {
    pub(crate) ctx: Arc<ZContext>,
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZContext {
    fn __enter__<'a, 'py>(this: &'a Bound<'py, Self>) -> &'a Bound<'py, Self> {
        this
    }

    #[pyo3(signature = (*_args, **_kwargs))]
    fn __exit__(
        &mut self,
        _py: Python,
        _args: &Bound<PyTuple>,
        _kwargs: Option<&Bound<PyDict>>,
    ) -> PyResult<()> {
        self.shutdown()
    }

    /// Shutdown the context and release all resources
    pub fn shutdown(&self) -> PyResult<()> {
        self.ctx.shutdown().map_err(|e| e.into_pyerr())
    }

    /// Create a node builder
    pub fn create_node(&self, name: String) -> PyZNodeBuilder {
        PyZNodeBuilder {
            ctx: self.ctx.clone(),
            name,
            namespace: None,
        }
    }
}
