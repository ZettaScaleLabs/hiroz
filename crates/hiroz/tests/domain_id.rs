//! `ROS_DOMAIN_ID` resolution, exercised only through `ZContextBuilder`'s
//! public API -- no reach into `context.rs`'s private `DomainId` type.
//!
//! Each `tests/*.rs` file compiles as its own process, so mutating the
//! process-global `ROS_DOMAIN_ID` here cannot race with the rest of the
//! crate's tests in other files or in the `--lib` binary. `#[serial]` only
//! has to guard against races between the handful of tests in *this* file.

use hiroz::{Builder, context::ZContextBuilder};
use serial_test::serial;

/// Restores the previous `ROS_DOMAIN_ID` (or its absence) on drop, so one
/// test's env mutation can't leak into the next.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: serialized by #[serial] -- no other thread reads/writes
        // process env vars while a guard is live.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: see above.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: see above.
        unsafe {
            match &self.previous {
                Some(val) => std::env::set_var(self.key, val),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
#[serial]
fn unset_env_defaults_to_domain_zero() {
    let _guard = EnvVarGuard::unset("ROS_DOMAIN_ID");
    let ctx = ZContextBuilder::default()
        .build()
        .expect("default domain must build");
    assert_eq!(ctx.domain_id(), 0);
}

#[test]
#[serial]
fn valid_env_is_used() {
    let _guard = EnvVarGuard::set("ROS_DOMAIN_ID", "42");
    let ctx = ZContextBuilder::default()
        .build()
        .expect("a valid ROS_DOMAIN_ID must build");
    assert_eq!(ctx.domain_id(), 42);
}

/// The behavior Copilot's review asked for: an invalid `ROS_DOMAIN_ID`
/// aborts `build()` with an error naming the bad value, matching
/// `rcl_init`, rather than silently producing a `ZContext` on domain 0.
#[test]
#[serial]
fn invalid_env_is_rejected_by_build() {
    let _guard = EnvVarGuard::set("ROS_DOMAIN_ID", "not-a-number");
    let err = ZContextBuilder::default()
        .build()
        .expect_err("an invalid ROS_DOMAIN_ID must not silently build");
    assert!(err.to_string().contains("not-a-number"));
}

#[test]
#[serial]
fn with_domain_id_overrides_an_invalid_env_value() {
    let _guard = EnvVarGuard::set("ROS_DOMAIN_ID", "garbage");
    let ctx = ZContextBuilder::default()
        .with_domain_id(7)
        .build()
        .expect("explicit with_domain_id must override a bad env value");
    assert_eq!(ctx.domain_id(), 7);
}
