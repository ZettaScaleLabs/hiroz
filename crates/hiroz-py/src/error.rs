#![allow(unexpected_cfgs)]

use pyo3::prelude::*;

// Custom exception types
pyo3::create_exception!(hiroz_py, HirozError, pyo3::exceptions::PyException);
pyo3::create_exception!(hiroz_py, TimeoutError, HirozError);
pyo3::create_exception!(hiroz_py, SerializationError, HirozError);
pyo3::create_exception!(hiroz_py, TypeMismatchError, HirozError);

/// Returns true if the error looks like a timeout (best-effort string sniff).
///
/// The core does not yet expose a dedicated `Timeout` error variant, so we
/// classify on the rendered message. Centralized here so every call site
/// (service `call`, action `send_goal`, …) classifies identically.
pub(crate) fn is_timeout_error(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("timeout") || s.contains("timed out")
}

/// Map a core error to the right Python exception.
///
/// Timeout-shaped errors become `hiroz_py.TimeoutError`; everything else
/// becomes `hiroz_py.HirozError`. Use this for blocking calls that raise on
/// failure (e.g. `ZClient.call`). Methods whose documented contract is to
/// return `None` on timeout should keep doing so rather than calling this.
pub(crate) fn map_call_error(e: anyhow::Error) -> PyErr {
    if is_timeout_error(&e) {
        TimeoutError::new_err(format!("{:#}", e))
    } else {
        HirozError::new_err(format!("{:#}", e))
    }
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
