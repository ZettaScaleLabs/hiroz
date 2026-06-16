//! Action client/server support for Python bindings.
//!
//! Uses `RawBytesAction` (Goal/Result/Feedback = `DynActionMessage`) so any
//! msgspec type works without per-type Rust code or type registry lookup.
//!
//! # Wire format
//!
//! Goal/result/feedback payloads are encoded with `msgspec.msgpack.encode` and
//! decoded with `msgspec.msgpack.decode(bytes, type=Cls)`. This is Python-to-Python
//! only — not CDR-compatible with rmw_zenoh_cpp.
//!
//! # ROS 2 interop
//!
//! Not supported via this Python API — the zero-hash TypeInfo and msgpack
//! encoding are incompatible with rmw_zenoh_cpp. Use typed Rust action
//! implementations for ROS 2 interop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hiroz::action::{GoalId, GoalStatus};
use once_cell::sync::OnceCell;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::raw_bytes::{DynActionMessage, RawBytesAction};

// ---- Global tokio runtime ----

/// Returns the global tokio runtime used for all action async operations.
///
/// The runtime is created once and lives for the program lifetime,
/// ensuring `tokio::spawn` calls from `ZActionServer::build()` persist.
pub(crate) fn get_tokio_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceCell<tokio::runtime::Runtime> = OnceCell::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for hiroz_py actions")
    })
}

// ---- Serialization helpers ----

/// Encode a Python object to msgpack bytes using `msgspec.msgpack.encode`.
fn msgspec_encode(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    py.import_bound("msgspec.msgpack")?
        .call_method1("encode", (obj,))?
        .extract::<Vec<u8>>()
}

/// Decode msgpack bytes to a Python object of the given class using
/// `msgspec.msgpack.decode(bytes, type=cls)`.
fn msgspec_decode(py: Python<'_>, bytes: &[u8], cls: &Py<PyAny>) -> PyResult<PyObject> {
    let kwargs = PyDict::new_bound(py);
    kwargs.set_item("type", cls.bind(py))?;
    let result =
        py.import_bound("msgspec.msgpack")?
            .call_method("decode", (bytes,), Some(&kwargs))?;
    Ok(result.unbind())
}

// ---- Goal status (unchanged, already registered) ----

/// Goal status enum matching ROS 2 action_msgs/msg/GoalStatus
#[pyclass(name = "GoalStatus")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PyGoalStatus {
    value: i8,
}

#[pymethods]
impl PyGoalStatus {
    #[classattr]
    const UNKNOWN: i8 = 0;
    #[classattr]
    const ACCEPTED: i8 = 1;
    #[classattr]
    const EXECUTING: i8 = 2;
    #[classattr]
    const CANCELING: i8 = 3;
    #[classattr]
    const SUCCEEDED: i8 = 4;
    #[classattr]
    const CANCELED: i8 = 5;
    #[classattr]
    const ABORTED: i8 = 6;

    #[new]
    fn new(value: i8) -> Self {
        Self { value }
    }

    #[getter]
    fn value(&self) -> i8 {
        self.value
    }

    fn is_active(&self) -> bool {
        matches!(self.value, 1..=3)
    }

    fn is_terminal(&self) -> bool {
        matches!(self.value, 4..=6)
    }

    fn __repr__(&self) -> String {
        let name = match self.value {
            0 => "UNKNOWN",
            1 => "ACCEPTED",
            2 => "EXECUTING",
            3 => "CANCELING",
            4 => "SUCCEEDED",
            5 => "CANCELED",
            6 => "ABORTED",
            _ => "INVALID",
        };
        format!("GoalStatus.{}", name)
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

// ---- Action Client ----

type RawActionClient = hiroz::action::client::ZActionClient<RawBytesAction>;
type RawClientGoalHandle =
    hiroz::action::client::GoalHandle<RawBytesAction, hiroz::action::client::goal_state::Active>;

/// Python wrapper for an action client.
///
/// Created via `node.create_action_client(action_name, goal_type, result_type, feedback_type)`.
#[pyclass(name = "ZActionClient")]
pub struct PyZActionClient {
    inner: Arc<RawActionClient>,
    goal_type: Py<PyAny>,
    result_type: Py<PyAny>,
    feedback_type: Py<PyAny>,
    /// Shared graph + the action's `send_goal` service name, used by `wait_for_server`.
    graph: Arc<hiroz::graph::Graph>,
    send_goal_service: String,
}

impl PyZActionClient {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: RawActionClient,
        goal_type: Py<PyAny>,
        result_type: Py<PyAny>,
        feedback_type: Py<PyAny>,
        graph: Arc<hiroz::graph::Graph>,
        send_goal_service: String,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
            goal_type,
            result_type,
            feedback_type,
            graph,
            send_goal_service,
        }
    }
}

#[pymethods]
impl PyZActionClient {
    /// Send a goal and block until the server accepts/rejects it.
    ///
    /// Returns an `ActionGoalHandle` on success, raises on rejection or error.
    #[pyo3(signature = (goal))]
    fn send_goal(&self, py: Python, goal: &Bound<'_, PyAny>) -> PyResult<PyZClientGoalHandle> {
        let encoded = msgspec_encode(py, goal)?;
        let goal_msg = DynActionMessage(encoded);

        let client = Arc::clone(&self.inner);
        let rt = get_tokio_rt();

        // send_goal is async — release GIL while blocking.
        // Apply a timeout slightly above the Zenoh querier timeout (10 s) so that
        // callers get a clear RuntimeError when no server is present instead of
        // blocking forever (the shared flume channel keeps the receiver alive even
        // after the Zenoh query expires and its error is discarded).
        let mut handle: RawClientGoalHandle = py.allow_threads(move || {
            rt.block_on(async move {
                tokio::time::timeout(Duration::from_secs(11), client.send_goal(goal_msg))
                    .await
                    .map_err(|_| {
                        crate::error::TimeoutError::new_err(
                            "send_goal timed out: no action server responded",
                        )
                    })?
                    .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
            })
        })?;

        let goal_id = *handle.id().as_bytes();

        // Take feedback receiver from handle (needed for feedback routing).
        let tokio_feedback_rx = handle.feedback();

        // Subscribe to status watch WITHOUT taking status_rx from handle.
        // Preserving handle.status_rx lets handle.result() wait for terminal
        // status before querying the server — without this, the server drops
        // get_result queries for goals that aren't yet terminated.
        let tokio_status_rx = self.inner.status_watch(GoalId::from_bytes(goal_id));

        // Bridge feedback: tokio mpsc → flume channel (blocking-friendly for Python)
        let (flume_tx, flume_feedback_rx) = flume::unbounded::<DynActionMessage>();
        if let Some(mut rx) = tokio_feedback_rx {
            rt.spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if flume_tx.send(msg).is_err() {
                        break; // Python side dropped the receiver
                    }
                }
            });
        }

        // Bridge status: watch → Arc<Mutex<GoalStatus>>
        // Initialize to Accepted: send_goal succeeded so the goal is at least Accepted.
        let status_arc = Arc::new(Mutex::new(GoalStatus::Accepted));
        if let Some(mut rx) = tokio_status_rx {
            // Read the current watch value immediately — the status may have already
            // advanced to Executing (or beyond) by the time we subscribe.
            let current = *rx.borrow_and_update();
            if current != GoalStatus::Unknown {
                *status_arc.lock().unwrap() = current;
            }
            let status_clone = Arc::clone(&status_arc);
            rt.spawn(async move {
                while rx.changed().await.is_ok() {
                    let s = *rx.borrow_and_update();
                    if let Ok(mut guard) = status_clone.lock() {
                        *guard = s;
                    }
                }
            });
        }

        Ok(PyZClientGoalHandle {
            goal_id,
            client_arc: Arc::clone(&self.inner),
            handle: Mutex::new(Some(handle)),
            flume_feedback_rx,
            status_arc,
            result_type: self.result_type.clone_ref(py),
            feedback_type: self.feedback_type.clone_ref(py),
        })
    }

    /// Wait until an action server for this action is available.
    ///
    /// Mirrors rclpy's `ActionClient.wait_for_server(timeout_sec)`. Polls the
    /// discovery graph for the action's `send_goal` service. Returns True if a
    /// server was found before `timeout`, False otherwise.
    ///
    /// Args:
    ///     timeout: Maximum seconds to wait. None waits forever.
    #[pyo3(signature = (timeout=None))]
    fn wait_for_server(&self, py: Python, timeout: Option<f64>) -> bool {
        py.allow_threads(|| {
            crate::graph::wait_for_service_server(&self.graph, &self.send_goal_service, timeout)
        })
    }

    /// Get the goal type class (for debugging).
    #[getter]
    fn goal_type(&self, py: Python) -> PyObject {
        self.goal_type.clone_ref(py)
    }
}

// ---- Client Goal Handle ----

/// Handle for a goal sent to an action server.
///
/// Returned by `ZActionClient.send_goal()`. Allows receiving feedback,
/// checking status, canceling, and retrieving the final result.
#[pyclass(name = "ActionGoalHandle")]
pub struct PyZClientGoalHandle {
    goal_id: [u8; 16],
    /// Client reference for cancellation (independent of handle ownership).
    client_arc: Arc<RawActionClient>,
    /// The Rust GoalHandle — None after `get_result()` is called.
    handle: Mutex<Option<RawClientGoalHandle>>,
    /// Feedback from server (forwarded from tokio mpsc to flume).
    flume_feedback_rx: flume::Receiver<DynActionMessage>,
    /// Latest goal status (updated by background task).
    status_arc: Arc<Mutex<GoalStatus>>,
    result_type: Py<PyAny>,
    feedback_type: Py<PyAny>,
}

#[pymethods]
impl PyZClientGoalHandle {
    /// Return the goal ID as bytes (16-byte UUID).
    #[getter]
    fn goal_id(&self) -> Vec<u8> {
        self.goal_id.to_vec()
    }

    /// Current goal status as an integer (matches GoalStatus constants).
    #[getter]
    fn status(&self) -> i8 {
        self.status_arc.lock().map(|g| *g as i8).unwrap_or(0)
    }

    /// Receive the next feedback message, optionally with a timeout (seconds).
    ///
    /// Returns None on timeout or if the feedback channel is closed.
    #[pyo3(signature = (timeout=None))]
    fn recv_feedback(&self, py: Python, timeout: Option<f64>) -> PyResult<Option<PyObject>> {
        let rx = self.flume_feedback_rx.clone();
        let bytes_opt = py.allow_threads(move || {
            if let Some(t) = timeout.map(Duration::from_secs_f64) {
                rx.recv_timeout(t).ok().map(|m| m.0)
            } else {
                rx.recv().ok().map(|m| m.0)
            }
        });
        match bytes_opt {
            Some(bytes) => Ok(Some(msgspec_decode(py, &bytes, &self.feedback_type)?)),
            None => Ok(None),
        }
    }

    /// Try to receive feedback without blocking.
    fn try_recv_feedback(&self, py: Python) -> PyResult<Option<PyObject>> {
        match self.flume_feedback_rx.try_recv() {
            Ok(msg) => Ok(Some(msgspec_decode(py, &msg.0, &self.feedback_type)?)),
            Err(_) => Ok(None),
        }
    }

    /// Wait for and return the final result, optionally with a timeout (seconds).
    ///
    /// Consumes the goal handle internally. Returns None on timeout.
    /// Raises RuntimeError if called more than once.
    #[pyo3(signature = (timeout=None))]
    fn get_result(&self, py: Python, timeout: Option<f64>) -> PyResult<Option<PyObject>> {
        let handle =
            self.handle.lock().unwrap().take().ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err("Result already retrieved")
            })?;

        let rt = get_tokio_rt();
        let result = py.allow_threads(move || {
            rt.block_on(async move {
                if let Some(t) = timeout.map(Duration::from_secs_f64) {
                    match tokio::time::timeout(t, handle.result()).await {
                        Ok(Ok(msg)) => Ok(Some(msg.0)),
                        Ok(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                        Err(_) => Ok(None), // timeout
                    }
                } else {
                    handle
                        .result()
                        .await
                        .map(|msg| Some(msg.0))
                        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
            })
        })?;

        match result {
            Some(bytes) => Ok(Some(msgspec_decode(py, &bytes, &self.result_type)?)),
            None => Ok(None),
        }
    }

    /// Request cancellation of this goal.
    fn cancel(&self, py: Python) -> PyResult<()> {
        let goal_id = GoalId::from_bytes(self.goal_id);
        let client = Arc::clone(&self.client_arc);
        let rt = get_tokio_rt();
        py.allow_threads(|| {
            rt.block_on(client.cancel_goal(goal_id))
                .map(|_| ())
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })
    }
}

// ---- Action Server ----

type RawActionServer = hiroz::action::server::ZActionServer<RawBytesAction>;

/// Python wrapper for an action server.
///
/// Created via `node.create_action_server(action_name, goal_type, result_type, feedback_type)`.
///
/// # Server loop pattern
///
/// ```python
/// server = node.create_action_server("/navigate", Goal, Result, Feedback)
/// while True:
///     request = server.recv_goal(timeout=1.0)
///     if request is None:
///         continue
///     executing = server.accept_and_execute(request)
///     executing.publish_feedback(Feedback(progress=0.5))
///     executing.succeed(Result(success=True))
/// ```
#[pyclass(name = "ZActionServer")]
pub struct PyZActionServer {
    inner: Arc<RawActionServer>,
    goal_type: Py<PyAny>,
}

impl PyZActionServer {
    pub fn new(
        inner: RawActionServer,
        goal_type: Py<PyAny>,
        _result_type: Py<PyAny>,
        _feedback_type: Py<PyAny>,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
            goal_type,
        }
    }
}

#[pymethods]
impl PyZActionServer {
    /// Wait for the next goal request, optionally with a timeout (seconds).
    ///
    /// Returns a `ServerGoalRequest` on success, or None on timeout.
    #[pyo3(signature = (timeout=None))]
    fn recv_goal(
        &self,
        py: Python,
        timeout: Option<f64>,
    ) -> PyResult<Option<PyZServerGoalRequest>> {
        let inner = Arc::clone(&self.inner);
        let rt = get_tokio_rt();

        let handle_opt = py.allow_threads(move || {
            rt.block_on(async move {
                if let Some(t) = timeout.map(Duration::from_secs_f64) {
                    match tokio::time::timeout(t, inner.recv_goal()).await {
                        Ok(Ok(h)) => Ok(Some(h)),
                        Ok(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                        Err(_) => Ok(None),
                    }
                } else {
                    inner
                        .recv_goal()
                        .await
                        .map(Some)
                        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
            })
        })?;

        match handle_opt {
            Some(handle) => {
                let goal_id = *handle.info.goal_id.as_bytes();
                let goal_bytes = handle.goal.0.clone();
                Ok(Some(PyZServerGoalRequest {
                    handle: Mutex::new(Some(handle)),
                    goal_id,
                    goal_bytes,
                    goal_type: self.goal_type.clone_ref(py),
                }))
            }
            None => Ok(None),
        }
    }
}

// ---- Server Goal Request (Requested state) ----

type RawRequestedHandle =
    hiroz::action::server::GoalHandle<RawBytesAction, hiroz::action::Requested>;

/// A goal request received by the action server (not yet accepted/rejected).
///
/// Call `server.accept_and_execute(request)` or `server.reject(request)`.
#[pyclass(name = "ServerGoalRequest")]
pub struct PyZServerGoalRequest {
    handle: Mutex<Option<RawRequestedHandle>>,
    goal_id: [u8; 16],
    goal_bytes: Vec<u8>,
    goal_type: Py<PyAny>,
}

#[pymethods]
impl PyZServerGoalRequest {
    /// The goal ID as bytes (16-byte UUID).
    #[getter]
    fn goal_id(&self) -> Vec<u8> {
        self.goal_id.to_vec()
    }

    /// The goal as a Python object.
    fn goal(&self, py: Python) -> PyResult<PyObject> {
        msgspec_decode(py, &self.goal_bytes, &self.goal_type)
    }

    /// Accept this goal request and begin execution.
    ///
    /// Returns a `ServerGoalHandle` for the executing goal.
    /// Raises RuntimeError if already accepted/rejected.
    fn accept_and_execute(&self, py: Python) -> PyResult<PyZServerExecutingHandle> {
        let handle = self.handle.lock().unwrap().take().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Goal request already handled")
        })?;

        // accept() → Accepted, then execute() → Executing
        let executing = handle.accept().execute();

        Ok(PyZServerExecutingHandle {
            handle: Mutex::new(Some(executing)),
            goal_id: self.goal_id,
            goal_bytes: self.goal_bytes.clone(),
            goal_type: self.goal_type.clone_ref(py),
        })
    }

    /// Reject this goal request. The client will receive an error.
    fn reject(&self) -> PyResult<()> {
        let handle = self.handle.lock().unwrap().take().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("Goal request already handled")
        })?;

        handle
            .reject()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }
}

// ---- Server Goal Handle (Executing state) ----

type RawExecutingHandle =
    hiroz::action::server::GoalHandle<RawBytesAction, hiroz::action::Executing>;

/// Handle for an accepted and executing action goal.
///
/// Returned by `ServerGoalRequest.accept_and_execute()`.
#[pyclass(name = "ServerGoalHandle")]
pub struct PyZServerExecutingHandle {
    handle: Mutex<Option<RawExecutingHandle>>,
    goal_id: [u8; 16],
    goal_bytes: Vec<u8>,
    goal_type: Py<PyAny>,
}

#[pymethods]
impl PyZServerExecutingHandle {
    /// The goal ID as bytes (16-byte UUID).
    #[getter]
    fn goal_id(&self) -> Vec<u8> {
        self.goal_id.to_vec()
    }

    /// The goal as a Python object.
    fn goal(&self, py: Python) -> PyResult<PyObject> {
        msgspec_decode(py, &self.goal_bytes, &self.goal_type)
    }

    /// Whether the client has requested cancellation of this goal.
    ///
    /// Also processes any pending cancel service request from the cancel queue
    /// (polling mode: no background handler). Non-blocking.
    #[getter]
    fn is_cancel_requested(&self) -> bool {
        self.handle
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|h| h.try_process_cancel()))
            .unwrap_or(false)
    }

    /// Publish a feedback message to the client.
    fn publish_feedback(&self, py: Python, feedback: &Bound<'_, PyAny>) -> PyResult<()> {
        let encoded = msgspec_encode(py, feedback)?;
        let msg = DynActionMessage(encoded);

        self.handle
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("Goal already terminated"))?
            .publish_feedback(msg)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    /// Mark the goal as succeeded with the given result.
    fn succeed(&self, py: Python, result: &Bound<'_, PyAny>) -> PyResult<()> {
        self.terminate(py, result, "succeed")
    }

    /// Mark the goal as aborted with the given result.
    fn abort(&self, py: Python, result: &Bound<'_, PyAny>) -> PyResult<()> {
        self.terminate(py, result, "abort")
    }

    /// Mark the goal as canceled with the given result.
    fn canceled(&self, py: Python, result: &Bound<'_, PyAny>) -> PyResult<()> {
        self.terminate(py, result, "canceled")
    }
}

impl PyZServerExecutingHandle {
    fn terminate(&self, py: Python, result: &Bound<'_, PyAny>, how: &str) -> PyResult<()> {
        let encoded = msgspec_encode(py, result)?;
        let msg = DynActionMessage(encoded);

        let handle =
            self.handle.lock().unwrap().take().ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err("Goal already terminated")
            })?;

        let result = match how {
            "succeed" => handle.succeed(msg),
            "abort" => handle.abort(msg),
            "canceled" => handle.canceled(msg),
            _ => unreachable!(),
        };

        result.map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }
}
