//! Structured error types for hiroz core operations.
//!
//! The crate's public [`Result`](crate::Result) alias is Zenoh's
//! `Result<T, Box<dyn std::error::Error + Send + Sync>>`, which erases the
//! concrete error type. That makes it impossible for callers — and, more
//! importantly, the language bindings — to tell *why* an operation failed.
//!
//! Historically the service/action timeout path returned a stringly-typed
//! `zenoh::Error::from(format!("... timed out ..."))`, forcing every binding
//! to sniff the error message (`e.to_string().contains("timeout")`). That is
//! fragile: it breaks silently the moment the wording changes.
//!
//! This module gives that path a real, structured error type with a
//! [`Timeout`](Error::Timeout) variant. It is boxed into the crate's
//! `Result`, and callers recover the structure with [`is_timeout`] (which
//! walks the `source()` chain) or by downcasting the boxed error to [`Error`].

use std::time::Duration;

/// Errors produced by hiroz core operations that callers and language
/// bindings need to distinguish structurally.
///
/// Construct a boxed, `Result`-compatible timeout error with
/// [`Error::timeout`], and detect one with [`is_timeout`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An operation did not complete before its timeout elapsed.
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),

    /// A generic, message-carrying failure that has no dedicated variant.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Build a boxed [`Error::Timeout`] suitable for returning from any
    /// function whose error type is the crate's [`Result`](crate::Result)
    /// (i.e. `zenoh::Error`).
    ///
    /// ```
    /// use std::time::Duration;
    /// let err = hiroz::error::Error::timeout(Duration::from_secs(1));
    /// assert!(hiroz::error::is_timeout(&*err));
    /// ```
    pub fn timeout(elapsed: Duration) -> zenoh::Error {
        Box::new(Error::Timeout(elapsed))
    }
}

/// Returns `true` if `err` — or any error in its [`source`](std::error::Error::source)
/// chain — is a hiroz [`Error::Timeout`].
///
/// Bindings use this instead of matching on the error's `Display` string, so a
/// change to the timeout message wording can never silently break timeout
/// detection.
pub fn is_timeout(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if matches!(e.downcast_ref::<Error>(), Some(Error::Timeout(_))) {
            return true;
        }
        current = e.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_constructor_is_detected() {
        let err = Error::timeout(Duration::from_millis(250));
        assert!(is_timeout(&*err));
        assert!(err.to_string().contains("250ms"));
    }

    #[test]
    fn non_timeout_errors_are_not_flagged() {
        let other = Error::Other("some other failure".to_string());
        assert!(!is_timeout(&other));

        let zenoh_err: zenoh::Error = zenoh::Error::from("plain failure");
        assert!(!is_timeout(&*zenoh_err));
    }

    #[test]
    fn is_timeout_walks_the_source_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("wrapper")]
        struct Wrapper(#[source] Error);

        let wrapped = Wrapper(Error::Timeout(Duration::from_secs(1)));
        assert!(is_timeout(&wrapped));

        let wrapped_other = Wrapper(Error::Other("nope".to_string()));
        assert!(!is_timeout(&wrapped_other));
    }

    #[test]
    fn downcast_recovers_the_variant() {
        let err = Error::timeout(Duration::from_secs(3));
        match err.downcast_ref::<Error>() {
            Some(Error::Timeout(d)) => assert_eq!(*d, Duration::from_secs(3)),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }
}
