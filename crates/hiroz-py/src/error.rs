#![allow(unexpected_cfgs)]

use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
use pyo3::types::{PyDict, PyTuple, PyType};

// Custom exception types.
//
// `HirozError` derives from `RuntimeError` rather than `Exception` so that
// rclpy code ported to hiroz-py keeps working: the blocking call paths used to
// raise a bare `RuntimeError`, and `except RuntimeError:` is what an rclpy user
// writes today.
pyo3::create_exception!(hiroz_py, HirozError, pyo3::exceptions::PyRuntimeError);
pyo3::create_exception!(hiroz_py, SerializationError, HirozError);
pyo3::create_exception!(hiroz_py, TypeMismatchError, HirozError);

/// `hiroz_py.TimeoutError`, built at module init.
///
/// It needs *two* bases — `HirozError` so `except hiroz_py.HirozError:` catches
/// timeouts, and `builtins.TimeoutError` because that is what rclpy's
/// `Client.call` actually raises, so a ported `except TimeoutError:` keeps
/// catching. `create_exception!` only accepts a single base, so the type is
/// constructed with `type(name, bases, dict)` instead.
///
/// Inheriting `builtins.TimeoutError` also makes these instances `OSError`s,
/// since that is its base. That matches rclpy's behaviour exactly.
static TIMEOUT_ERROR: GILOnceCell<Py<PyType>> = GILOnceCell::new();

/// Build the `TimeoutError` type. Called once from module init.
pub(crate) fn init_timeout_error(py: Python<'_>) -> PyResult<Py<PyType>> {
    let bases = PyTuple::new_bound(
        py,
        [
            py.get_type_bound::<HirozError>().into_any(),
            py.get_type_bound::<pyo3::exceptions::PyTimeoutError>()
                .into_any(),
        ],
    );
    let dict = PyDict::new_bound(py);
    dict.set_item("__module__", "hiroz_py")?;
    dict.set_item(
        "__doc__",
        "Raised when a hiroz-py operation exceeds its timeout.\n\n\
         Subclasses both hiroz_py.HirozError and the builtin TimeoutError.",
    )?;

    let cls = py
        .get_type_bound::<PyType>()
        .call1(("TimeoutError", bases, dict))?
        .downcast_into::<PyType>()?;

    let cls: Py<PyType> = cls.unbind();
    TIMEOUT_ERROR.set(py, cls.clone_ref(py)).ok();
    Ok(cls)
}

/// Construct a `hiroz_py.TimeoutError` carrying `msg`.
pub(crate) fn timeout_err(msg: String) -> PyErr {
    Python::with_gil(|py| match TIMEOUT_ERROR.get(py) {
        Some(cls) => match cls.bind(py).call1((msg.clone(),)) {
            Ok(instance) => PyErr::from_value_bound(instance),
            // Falling back keeps the error visible rather than masking the
            // original failure behind a construction error.
            Err(e) => e,
        },
        None => HirozError::new_err(msg),
    })
}

/// Render an error and its full source chain as `outer: inner: root`.
///
/// Matches anyhow's `{:#}` output, which a bare `Box<dyn Error>` does not give us.
fn format_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        msg.push_str(&format!(": {e}"));
        source = e.source();
    }
    msg
}

fn classify(is_timeout: bool, msg: String) -> PyErr {
    if is_timeout {
        timeout_err(msg)
    } else {
        HirozError::new_err(msg)
    }
}

/// Map an `anyhow` error to the right Python exception.
///
/// Timeout-shaped errors become `hiroz_py.TimeoutError`; everything else
/// becomes `hiroz_py.HirozError`. Use this for blocking calls that raise on
/// failure (e.g. `ZClient.call`). Methods whose documented contract is to
/// return `None` on timeout should keep doing so rather than calling this.
///
/// Classification goes through the core's structured detector, which walks the
/// whole source chain — do not string-match on the message.
pub(crate) fn map_call_error(e: anyhow::Error) -> PyErr {
    // Deref rather than boxing: `Box<dyn Error>::from(anyhow::Error)` wraps the
    // value so `is_timeout`'s downcast no longer sees the real error and every
    // timeout is misreported as a plain HirozError.
    classify(hiroz::error::is_timeout(&*e), format!("{e:#}"))
}

/// Same mapping for the action paths, which yield `zenoh::Error`
/// (`Box<dyn Error + Send + Sync>`) rather than `anyhow::Error`.
pub(crate) fn map_zenoh_error(e: zenoh::Error) -> PyErr {
    classify(hiroz::error::is_timeout(&*e), format_chain(&*e))
}

/// Trait for converting Rust errors to Python exceptions
pub(crate) trait IntoPyErr {
    fn into_pyerr(self) -> PyErr;
}

impl IntoPyErr for anyhow::Error {
    fn into_pyerr(self) -> PyErr {
        HirozError::new_err(format!("{:#}", self))
    }
}

impl IntoPyErr for zenoh::Error {
    fn into_pyerr(self) -> PyErr {
        HirozError::new_err(format!("Zenoh error: {}", self))
    }
}

impl<T> IntoPyErr for Result<T, anyhow::Error> {
    fn into_pyerr(self) -> PyErr {
        match self {
            Ok(_) => panic!("Tried to convert Ok to error"),
            Err(e) => e.into_pyerr(),
        }
    }
}
