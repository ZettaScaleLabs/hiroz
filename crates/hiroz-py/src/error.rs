#![allow(unexpected_cfgs)]

use pyo3::prelude::*;

// Custom exception types
pyo3::create_exception!(hiroz_py, HirozError, pyo3::exceptions::PyException);
pyo3::create_exception!(hiroz_py, TimeoutError, HirozError);
pyo3::create_exception!(hiroz_py, SerializationError, HirozError);
pyo3::create_exception!(hiroz_py, TypeMismatchError, HirozError);

/// Render an error and its full source chain as `outer: inner: root`.
///
/// Matches anyhow's `{:#}` output, which we lose once the error is boxed.
fn format_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        msg.push_str(&format!(": {e}"));
        source = e.source();
    }
    msg
}

/// Map a core error to the right Python exception.
///
/// Timeout-shaped errors become `hiroz_py.TimeoutError`; everything else
/// becomes `hiroz_py.HirozError`. Use this for blocking calls that raise on
/// failure (e.g. `ZClient.call`, `send_goal`, `get_result`). Methods whose
/// documented contract is to return `None` on timeout should keep doing so
/// rather than calling this.
///
/// Generic over the error type because the service path yields `anyhow::Error`
/// while the action path yields `zenoh::Error`; both classify identically via
/// the core's structured detector, which walks the whole source chain — do not
/// string-match on the message.
pub(crate) fn map_call_error<E>(e: E) -> PyErr
where
    E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
{
    let err = e.into();
    let msg = format_chain(&*err);
    if hiroz::error::is_timeout(&*err) {
        TimeoutError::new_err(msg)
    } else {
        HirozError::new_err(msg)
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
