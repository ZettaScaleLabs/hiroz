use crate::action::{PyZActionClient, PyZActionServer, get_tokio_rt};
use crate::error::IntoPyErr;
use crate::graph::GraphQueries;
use crate::pubsub::{PyZPublisher, PyZSubscriber};
use crate::qos::extract_qos;
use crate::raw_bytes::{RawBytesAction, RawBytesCdrSerdes, RawBytesMessage, RawBytesService};
use crate::service::{PyZClient, PyZServer};
use crate::traits::{
    GenericClientWrapper, GenericPubWrapper, GenericServerWrapper, GenericSubWrapper,
};
use crate::utils::python_type_to_rust_type;
use hiroz::Builder;
use hiroz::context::ZContext;
use hiroz::entity::{TypeHash, TypeInfo};
use hiroz::node::ZNode;
use pyo3::prelude::*;
use std::any::Any;
use std::sync::Arc;

/// Try to extract type info from a message class.
///
/// Returns `None` if `__msgtype__` or `__hash__` are absent or not strings —
/// e.g. for inline msgspec structs without a registered type hash.
fn try_extract_type_info(msg_class: &Bound<'_, PyAny>) -> Option<TypeInfo> {
    let msg_type: String = msg_class.getattr("__msgtype__").ok()?.extract().ok()?;
    let type_hash_str: String = msg_class.getattr("__hash__").ok()?.extract().ok()?;
    let type_hash = TypeHash::from_rihs_string(&type_hash_str)?;
    let rust_type_name = python_type_to_rust_type(&msg_type);
    Some(TypeInfo::new(&rust_type_name, type_hash))
}

/// Extract type information from a msgspec message class.
///
/// `__msgtype__` (a string like `"my_pkg/msg/Goal"`) is required.
/// `__hash__` (a RIHS01 string) is optional — absent or non-string values
/// result in a zero hash, which is fine for Python-to-Python communication.
fn extract_type_info_from_class(msg_class: &Bound<'_, PyAny>) -> PyResult<(String, TypeInfo)> {
    let msg_type: String = msg_class
        .getattr("__msgtype__")
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Message class must have __msgtype__ class attribute",
            )
        })?
        .extract()
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err("Message class __msgtype__ must be a string")
        })?;

    // __hash__ is optional. If it's a valid RIHS01 string, use it; otherwise zero hash.
    let type_hash = msg_class
        .getattr("__hash__")
        .ok()
        .and_then(|v| v.extract::<String>().ok())
        .and_then(|s| TypeHash::from_rihs_string(&s))
        .unwrap_or_else(TypeHash::zero);

    let rust_type_name = python_type_to_rust_type(&msg_type);
    let type_info = TypeInfo::new(&rust_type_name, type_hash);

    Ok((msg_type, type_info))
}

/// Extract service type information from a service Request class.
fn extract_service_type_from_request_class(
    request_class: &Bound<'_, PyAny>,
) -> PyResult<(String, TypeInfo)> {
    let msg_type: String = request_class
        .getattr("__msgtype__")
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Service Request class must have __msgtype__ class attribute",
            )
        })?
        .extract()
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Service Request class __msgtype__ must be a string",
            )
        })?;

    let type_hash_str: String = request_class
        .getattr("__hash__")
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Service Request class must have __hash__ class attribute",
            )
        })?
        .extract()
        .map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Service Request class __hash__ must be a string",
            )
        })?;

    let type_hash = TypeHash::from_rihs_string(&type_hash_str).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "Invalid type hash format: {}",
            type_hash_str
        ))
    })?;

    // Convert "example_interfaces/msg/AddTwoIntsRequest" to "example_interfaces/srv/AddTwoInts"
    let srv_type = msg_type
        .replace("/msg/", "/srv/")
        .trim_end_matches("Request")
        .trim_end_matches("Response")
        .to_string();

    let rust_type_name = python_type_to_rust_type(&srv_type);
    let type_info = TypeInfo::new(&rust_type_name, type_hash);

    Ok((srv_type, type_info))
}

/// Extract service type info from either a service grouping class (P4, rclpy-style)
/// or a bare Request class (back-compat).
///
/// A grouping class exposes `__srvtype__` (e.g. `"example_interfaces/srv/AddTwoInts"`)
/// plus `Request` / `Response` member classes. We read the type hash from the
/// `Request` member. Anything without `__srvtype__` falls through to the legacy
/// string-munging path on the Request class itself.
fn extract_service_type_info(srv_type: &Bound<'_, PyAny>) -> PyResult<(String, TypeInfo)> {
    if let Ok(srvtype_attr) = srv_type.getattr("__srvtype__")
        && let Ok(srv_type_str) = srvtype_attr.extract::<String>()
    {
        // Grouping class: pull the type hash from the Request member.
        let request_cls = srv_type.getattr("Request").map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Service grouping class with __srvtype__ must define a Request member",
            )
        })?;
        let type_hash = request_cls
            .getattr("__hash__")
            .ok()
            .and_then(|v| v.extract::<String>().ok())
            .and_then(|s| TypeHash::from_rihs_string(&s))
            .unwrap_or_else(TypeHash::zero);
        let rust_type_name = python_type_to_rust_type(&srv_type_str);
        return Ok((srv_type_str, TypeInfo::new(&rust_type_name, type_hash)));
    }
    // Back-compat: bare Request class.
    extract_service_type_from_request_class(srv_type)
}

/// If `topic` is not a string but `msg_type` is, the caller almost certainly used
/// the rclpy positional order `(msg_type, topic)`. Raise a self-explaining error
/// instead of a confusing downstream type failure (P2).
fn reject_swapped_args(
    topic: &Bound<'_, PyAny>,
    msg_type: &Bound<'_, PyAny>,
    func: &str,
) -> PyResult<()> {
    let topic_is_str = topic.is_instance_of::<pyo3::types::PyString>();
    let msg_is_str = msg_type.is_instance_of::<pyo3::types::PyString>();
    if !topic_is_str && msg_is_str {
        return Err(pyo3::exceptions::PyTypeError::new_err(format!(
            "arguments look swapped — hiroz uses ({func}(topic, msg_type, ...)) but rclpy uses \
             (msg_type, topic, ...). Pass by keyword: {func}(topic=..., msg_type=...)"
        )));
    }
    Ok(())
}

/// Resolve a topic argument to a `String`, with a clear error if it isn't a str.
fn extract_topic(topic: &Bound<'_, PyAny>) -> PyResult<String> {
    topic.extract::<String>().map_err(|_| {
        pyo3::exceptions::PyTypeError::new_err(
            "topic must be a string (e.g. \"/chatter\"). Pass by keyword if unsure: topic=...",
        )
    })
}

/// Extract Goal/Result/Feedback classes from either an action grouping class
/// (P7, rclpy-style — exposes `__actiontype__`, `Goal`, `Result`, `Feedback`)
/// or fall back to three explicitly-passed classes.
///
/// Returns the three member classes as owned `PyObject`s.
fn resolve_action_types(
    action_type: &Bound<'_, PyAny>,
    result_type: Option<&Bound<'_, PyAny>>,
    feedback_type: Option<&Bound<'_, PyAny>>,
) -> PyResult<(PyObject, PyObject, PyObject)> {
    // Grouping class path: a single action type with member classes.
    if action_type.hasattr("__actiontype__").unwrap_or(false) {
        let goal = action_type.getattr("Goal").map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Action grouping class with __actiontype__ must define a Goal member",
            )
        })?;
        let result = action_type.getattr("Result").map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Action grouping class with __actiontype__ must define a Result member",
            )
        })?;
        let feedback = action_type.getattr("Feedback").map_err(|_| {
            pyo3::exceptions::PyTypeError::new_err(
                "Action grouping class with __actiontype__ must define a Feedback member",
            )
        })?;
        return Ok((goal.unbind(), result.unbind(), feedback.unbind()));
    }

    // Back-compat: three separate classes.
    let (Some(result), Some(feedback)) = (result_type, feedback_type) else {
        return Err(pyo3::exceptions::PyTypeError::new_err(
            "create_action_*: pass either a single action grouping class (with __actiontype__) \
             or all three of goal_type, result_type, feedback_type",
        ));
    };
    Ok((
        action_type.clone().unbind(),
        result.clone().unbind(),
        feedback.clone().unbind(),
    ))
}

#[pyclass(name = "ZNodeBuilder")]
pub struct PyZNodeBuilder {
    pub(crate) ctx: Arc<ZContext>,
    pub(crate) name: String,
    pub(crate) namespace: Option<String>,
}

#[pymethods]
impl PyZNodeBuilder {
    /// Set the namespace for the node
    pub fn with_namespace(mut slf: PyRefMut<'_, Self>, namespace: String) -> PyRefMut<'_, Self> {
        slf.namespace = Some(namespace);
        slf
    }

    /// Build the node
    pub fn build(&self) -> PyResult<PyZNode> {
        let mut builder = self.ctx.create_node(&self.name);
        if let Some(ref ns) = self.namespace {
            builder = builder.with_namespace(ns);
        }

        let node = builder.build().map_err(|e| e.into_pyerr())?;
        Ok(PyZNode {
            inner: Arc::new(node),
            name: self.name.clone(),
            namespace: self.namespace.clone().unwrap_or_else(|| "/".to_string()),
            owned_subs: Vec::new(),
            next_sub_id: 0,
        })
    }
}

#[pyclass(name = "ZNode")]
pub struct PyZNode {
    pub(crate) inner: Arc<ZNode>,
    name: String,
    namespace: String,
    /// Keeps callback-based subscribers alive for the node's lifetime.
    /// Matches rmw_zenoh_cpp's NodeData::subs_ and rclpy's _subscriptions ownership patterns.
    /// Keyed by a monotonic ID so destroy_subscriber can remove a specific entry.
    owned_subs: Vec<(u64, Box<dyn Any + Send>)>,
    next_sub_id: u64,
}

#[allow(unsafe_op_in_unsafe_fn)]
#[pymethods]
impl PyZNode {
    /// Get the node name
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    /// Get the node namespace
    #[getter]
    fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Get the fully qualified node name (namespace + name)
    #[getter]
    fn fully_qualified_name(&self) -> String {
        if self.namespace == "/" {
            format!("/{}", self.name)
        } else {
            format!("{}/{}", self.namespace, self.name)
        }
    }

    /// Create a publisher for a given topic and message type.
    ///
    /// Works with any registered message type — no factory limitations.
    #[pyo3(signature = (topic, msg_type, qos=None))]
    fn create_publisher(
        &self,
        topic: &Bound<'_, PyAny>,
        msg_type: &Bound<'_, PyAny>,
        qos: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyZPublisher> {
        reject_swapped_args(topic, msg_type, "create_publisher")?;
        let topic = extract_topic(topic)?;
        let (msg_type_str, type_info) = extract_type_info_from_class(msg_type)?;
        let qos_profile = extract_qos(qos)?;

        let pub_builder = self
            .inner
            .create_pub_impl::<RawBytesMessage>(&topic, Some(type_info))
            .with_serdes::<RawBytesCdrSerdes>()
            .with_qos(qos_profile);
        let zpub = pub_builder.build().map_err(|e| e.into_pyerr())?;
        let wrapper = GenericPubWrapper::new(zpub);
        Ok(PyZPublisher::new(Box::new(wrapper), msg_type_str))
    }

    /// Create a subscriber for a given topic and message type.
    ///
    /// Works with any registered message type — no factory limitations.
    #[pyo3(signature = (topic, msg_type, qos=None, callback=None))]
    fn create_subscriber(
        &mut self,
        _py: Python,
        topic: &Bound<'_, PyAny>,
        msg_type: &Bound<'_, PyAny>,
        qos: Option<&Bound<'_, PyAny>>,
        callback: Option<PyObject>,
    ) -> PyResult<PyZSubscriber> {
        reject_swapped_args(topic, msg_type, "create_subscriber")?;
        let topic = extract_topic(topic)?;
        let (msg_type_str, type_info) = extract_type_info_from_class(msg_type)?;
        let qos_profile = extract_qos(qos)?;

        let sub_builder = self
            .inner
            .create_sub_impl::<RawBytesMessage>(&topic, Some(type_info))
            .with_serdes::<RawBytesCdrSerdes>()
            .with_qos(qos_profile);

        if let Some(py_callback) = callback {
            // Callback-based subscription: no queue, callback fires on each message.
            // The ZSub handle is stored in owned_subs so it lives as long as the node,
            // matching rmw_zenoh_cpp's NodeData::subs_ pattern. The caller does not
            // need to assign the returned PyZSubscriber to keep the subscription active.
            let type_name = msg_type_str.clone();
            let zsub = sub_builder
                .build_with_callback(move |raw_msg: RawBytesMessage| {
                    let payload = raw_msg.0;
                    Python::with_gil(|py| {
                        match hiroz_msgs::deserialize_from_cdr(&type_name, py, &payload) {
                            Ok(obj) => {
                                if let Err(e) = py_callback.call1(py, (obj,)) {
                                    eprintln!("hiroz_py: callback error: {}", e);
                                }
                            }
                            Err(e) => {
                                eprintln!("hiroz_py: deserialization error in callback: {}", e);
                            }
                        }
                    });
                })
                .map_err(|e| e.into_pyerr())?;

            let id = self.next_sub_id;
            self.next_sub_id += 1;
            self.owned_subs.push((id, Box::new(zsub)));
            Ok(PyZSubscriber::new_callback(msg_type_str, id))
        } else {
            let zsub = sub_builder.build().map_err(|e| e.into_pyerr())?;
            let wrapper = GenericSubWrapper::new(zsub);
            Ok(PyZSubscriber::new(Box::new(wrapper), msg_type_str))
        }
    }

    /// Create a service client.
    ///
    /// `srv_type` may be a service grouping class (rclpy-style, e.g.
    /// `example_interfaces.AddTwoInts`) or the bare Request class (back-compat).
    fn create_client(&self, service: String, srv_type: &Bound<'_, PyAny>) -> PyResult<PyZClient> {
        let (srv_type_str, type_info) = extract_service_type_info(srv_type)?;

        let client_builder = self
            .inner
            .create_client_impl::<RawBytesService>(&service, Some(type_info));
        let zclient = client_builder.build().map_err(|e| e.into_pyerr())?;
        let wrapper = GenericClientWrapper::new(zclient);
        let qualified = self.qualify_service_name(&service);
        Ok(PyZClient::new(
            Box::new(wrapper),
            srv_type_str,
            Arc::clone(self.inner.graph()),
            qualified,
        ))
    }

    // -- Graph discovery methods --

    /// Get all topic names and their types.
    /// Returns list of (topic_name, type_name) tuples.
    fn get_topic_names_and_types(&self) -> Vec<(String, String)> {
        GraphQueries::get_topic_names_and_types(self.inner.graph())
    }

    /// Get all node names.
    /// Returns list of (name, namespace) tuples.
    fn get_node_names(&self) -> Vec<(String, String)> {
        GraphQueries::get_node_names(self.inner.graph())
    }

    /// Get all service names and their types.
    /// Returns list of (service_name, type_name) tuples.
    fn get_service_names_and_types(&self) -> Vec<(String, String)> {
        GraphQueries::get_service_names_and_types(self.inner.graph())
    }

    /// Count publishers for a topic.
    fn count_publishers(&self, topic: String) -> usize {
        GraphQueries::count_publishers(self.inner.graph(), &topic)
    }

    /// Count subscribers for a topic.
    fn count_subscribers(&self, topic: String) -> usize {
        GraphQueries::count_subscribers(self.inner.graph(), &topic)
    }

    /// Create an action client.
    ///
    /// `goal_type`, `result_type`, `feedback_type` must be msgspec classes with
    /// `__msgtype__` and `__hash__` attributes (from `hiroz_msgs_py`).
    ///
    /// Returns a `ZActionClient` for sending goals and receiving results.
    #[pyo3(signature = (action_name, goal_type, result_type=None, feedback_type=None))]
    fn create_action_client(
        &self,
        py: Python,
        action_name: String,
        goal_type: &Bound<'_, PyAny>,
        result_type: Option<&Bound<'_, PyAny>>,
        feedback_type: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyZActionClient> {
        let (goal_obj, result_obj, feedback_obj) =
            resolve_action_types(goal_type, result_type, feedback_type)?;
        let goal_b = goal_obj.bind(py);
        let result_b = result_obj.bind(py);
        let feedback_b = feedback_obj.bind(py);

        // __msgtype__ is still required (validates the class); __hash__ is optional.
        extract_type_info_from_class(goal_b)?;
        extract_type_info_from_class(result_b)?;
        extract_type_info_from_class(feedback_b)?;

        let goal_ti = try_extract_type_info(goal_b);
        let result_ti = try_extract_type_info(result_b);
        let feedback_ti = try_extract_type_info(feedback_b);

        let node = Arc::clone(&self.inner);
        let rt = get_tokio_rt();

        let client = py.allow_threads(|| {
            let _guard = rt.enter();
            let mut builder = node.create_action_client::<RawBytesAction>(&action_name);
            if let Some(ti) = goal_ti {
                builder = builder.with_goal_type_info(ti);
            }
            if let Some(ti) = result_ti {
                builder = builder.with_result_type_info(ti);
            }
            if let Some(ti) = feedback_ti {
                builder = builder.with_feedback_type_info(ti);
            }
            builder
                .build()
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })?;

        // The action server advertises a `<action>/_action/send_goal` service;
        // wait_for_server polls the graph for it.
        let send_goal_service = format!(
            "{}/_action/send_goal",
            self.qualify_service_name(&action_name)
        );

        Ok(PyZActionClient::new(
            client,
            goal_obj.clone_ref(py),
            result_obj.clone_ref(py),
            feedback_obj.clone_ref(py),
            Arc::clone(self.inner.graph()),
            send_goal_service,
        ))
    }

    /// Create an action server.
    ///
    /// `goal_type`, `result_type`, `feedback_type` must be msgspec classes with
    /// `__msgtype__` and `__hash__` attributes (from `hiroz_msgs_py`).
    ///
    /// Returns a `ZActionServer` for receiving and executing goals.
    #[pyo3(signature = (action_name, goal_type, result_type=None, feedback_type=None))]
    fn create_action_server(
        &self,
        py: Python,
        action_name: String,
        goal_type: &Bound<'_, PyAny>,
        result_type: Option<&Bound<'_, PyAny>>,
        feedback_type: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyZActionServer> {
        let (goal_obj, result_obj, feedback_obj) =
            resolve_action_types(goal_type, result_type, feedback_type)?;
        let goal_b = goal_obj.bind(py);
        let result_b = result_obj.bind(py);
        let feedback_b = feedback_obj.bind(py);

        // __msgtype__ is still required (validates the class); __hash__ is optional.
        extract_type_info_from_class(goal_b)?;
        extract_type_info_from_class(result_b)?;
        extract_type_info_from_class(feedback_b)?;

        let goal_ti = try_extract_type_info(goal_b);
        let result_ti = try_extract_type_info(result_b);
        let feedback_ti = try_extract_type_info(feedback_b);

        let node = Arc::clone(&self.inner);
        let rt = get_tokio_rt();

        let server = py.allow_threads(|| {
            let _guard = rt.enter();
            let mut builder = node.create_action_server::<RawBytesAction>(&action_name);
            if let Some(ti) = goal_ti {
                builder = builder.with_goal_type_info(ti);
            }
            if let Some(ti) = result_ti {
                builder = builder.with_result_type_info(ti);
            }
            if let Some(ti) = feedback_ti {
                builder = builder.with_feedback_type_info(ti);
            }
            builder
                .build()
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        })?;

        Ok(PyZActionServer::new(
            server,
            goal_obj.clone_ref(py),
            result_obj.clone_ref(py),
            feedback_obj.clone_ref(py),
        ))
    }

    /// Destroy a callback-based subscriber early, undeclaring its Zenoh subscription.
    ///
    /// Matches rclpy's `Node.destroy_subscription()`. Has no effect on queue-based
    /// subscribers (those are owned by the caller and dropped when they go out of scope).
    fn destroy_subscriber(&mut self, sub: &PyZSubscriber) -> PyResult<()> {
        let Some(id) = sub.owned_id else {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "destroy_subscriber only applies to callback-based subscribers",
            ));
        };
        if let Some(pos) = self.owned_subs.iter().position(|(sid, _)| *sid == id) {
            self.owned_subs.swap_remove(pos);
        }
        Ok(())
    }

    /// Create a service server.
    ///
    /// `srv_type` may be a service grouping class (rclpy-style) or the bare
    /// Request class (back-compat).
    ///
    /// If `callback` is provided, the server runs in callback mode: a background
    /// thread receives each request, invokes `callback(request)`, and sends the
    /// returned value as the response. The caller never calls `take_request` /
    /// `send_response`. If `callback` is None (default), the server is in pull
    /// mode and the caller drives it via `take_request` / `send_response`.
    #[pyo3(signature = (service, srv_type, callback=None))]
    fn create_server(
        &self,
        service: String,
        srv_type: &Bound<'_, PyAny>,
        callback: Option<PyObject>,
    ) -> PyResult<PyZServer> {
        let (srv_type_str, type_info) = extract_service_type_info(srv_type)?;

        let server_builder = self
            .inner
            .create_service_impl::<RawBytesService>(&service, Some(type_info));
        let zserver = server_builder.build().map_err(|e| e.into_pyerr())?;
        let wrapper = GenericServerWrapper::new(zserver);

        match callback {
            Some(cb) => Ok(PyZServer::new_with_callback(
                Arc::new(wrapper),
                srv_type_str,
                cb,
            )),
            None => Ok(PyZServer::new(Box::new(wrapper), srv_type_str)),
        }
    }
}

impl PyZNode {
    /// Qualify a service name against the node's namespace/name so the result
    /// matches the entries the discovery graph stores. Absolute names pass through.
    fn qualify_service_name(&self, service: &str) -> String {
        hiroz::topic_name::qualify_topic_name(service, self.inner.namespace(), self.inner.name())
            .unwrap_or_else(|_| service.to_string())
    }
}
