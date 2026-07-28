#![allow(unexpected_cfgs)]

use pyo3::prelude::*;

// Custom exception types
pyo3::create_exception!(hiroz_py, HirozError, pyo3::exceptions::PyException);
pyo3::create_exception!(hiroz_py, TimeoutError, HirozError);
pyo3::create_exception!(hiroz_py, SerializationError, HirozError);
pyo3::create_exception!(hiroz_py, TypeMismatchError, HirozError);

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
        TimeoutError::new_err(msg)
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
