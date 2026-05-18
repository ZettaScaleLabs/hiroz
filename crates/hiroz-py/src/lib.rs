#![allow(unsafe_op_in_unsafe_fn, clippy::useless_conversion, private_interfaces)]

use pyo3::prelude::*;

mod action;
mod context;
mod error;
mod graph;
mod node;
mod payload_view;
mod pubsub;
mod qos;
mod raw_bytes;
mod service;
mod traits;
mod utils;

use action::{
    PyGoalStatus, PyZActionClient, PyZActionServer, PyZClientGoalHandle, PyZServerExecutingHandle,
    PyZServerGoalRequest,
};
use context::*;
use node::*;
use payload_view::ZPayloadView;
use pubsub::{PyZPublisher, PyZSubscriber};
use qos::PyQosProfile;
use service::*;

/// Get list of all registered message types
#[pyfunction]
fn list_registered_types() -> Vec<String> {
    hiroz_msgs::list_registered_types()
}

/// Python bindings for hiroz: Native Rust ROS 2 implementation using Zenoh
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Register custom exceptions
    m.add("HirozError", m.py().get_type_bound::<error::HirozError>())?;
    m.add(
        "TimeoutError",
        m.py().get_type_bound::<error::TimeoutError>(),
    )?;
    m.add(
        "SerializationError",
        m.py().get_type_bound::<error::SerializationError>(),
    )?;
    m.add(
        "TypeMismatchError",
        m.py().get_type_bound::<error::TypeMismatchError>(),
    )?;

    // Register functions
    m.add_function(wrap_pyfunction!(list_registered_types, m)?)?;

    // Register classes
    m.add_class::<PyZContextBuilder>()?;
    m.add_class::<PyZContext>()?;
    m.add_class::<PyZNodeBuilder>()?;
    m.add_class::<PyZNode>()?;
    m.add_class::<PyZPublisher>()?;
    m.add_class::<PyZSubscriber>()?;
    m.add_class::<PyZClient>()?;
    m.add_class::<PyZServer>()?;
    m.add_class::<ZPayloadView>()?;
    m.add_class::<hiroz::zbuf_view::ZBufView>()?;
    m.add_class::<PyQosProfile>()?;
    m.add_class::<PyGoalStatus>()?;
    m.add_class::<PyZActionClient>()?;
    m.add_class::<PyZClientGoalHandle>()?;
    m.add_class::<PyZActionServer>()?;
    m.add_class::<PyZServerGoalRequest>()?;
    m.add_class::<PyZServerExecutingHandle>()?;

    // QoS presets
    m.add(
        "QOS_DEFAULT",
        qos::qos_to_pydict(m.py(), &qos::QOS_DEFAULT)?,
    )?;
    m.add(
        "QOS_SENSOR_DATA",
        qos::qos_to_pydict(m.py(), &qos::QOS_SENSOR_DATA)?,
    )?;
    m.add(
        "QOS_PARAMETERS",
        qos::qos_to_pydict(m.py(), &qos::QOS_PARAMETERS)?,
    )?;
    m.add(
        "QOS_SERVICES",
        qos::qos_to_pydict(m.py(), &qos::QOS_SERVICES)?,
    )?;

    // Register the hiroz_msgs submodule with message registry
    let hiroz_msgs_module = PyModule::new_bound(m.py(), "hiroz_msgs")?;
    hiroz_msgs::hiroz_msgs(m.py(), &hiroz_msgs_module)?;
    m.add_submodule(&hiroz_msgs_module)?;

    // Register the submodule in sys.modules so it can be accessed from Rust
    let sys = m.py().import_bound("sys")?;
    let sys_modules = sys.getattr("modules")?;
    // Register under both paths: the submodule path and the legacy top-level path.
    // Internal Rust code looks up "hiroz_py.hiroz_msgs"; keep that working after the rename.
    sys_modules.set_item("hiroz_py._native.hiroz_msgs", &hiroz_msgs_module)?;
    sys_modules.set_item("hiroz_py.hiroz_msgs", &hiroz_msgs_module)?;

    Ok(())
}
