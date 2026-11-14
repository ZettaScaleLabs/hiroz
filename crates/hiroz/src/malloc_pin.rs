//! Allocator pin for large message payloads (glibc only).
//!
//! glibc trims the top of the main heap when the free top chunk reaches `M_TRIM_THRESHOLD`. It
//! serves a request at or above `M_MMAP_THRESHOLD` with a fresh `mmap`, but only when the bins
//! and the top chunk cannot serve it. A receiver that allocates a payload buffer per message can
//! then pay page faults on every message. Pinning both thresholds keeps the payload heap in the
//! process.
//!
//! Scope:
//! - This module is internal to the hiroz crates and not part of the public API. It is hidden
//!   from the documentation and carries no stability promise. It is `pub` only because
//!   `rmw-zenoh-rs` and the test crate call it.
//! - Only `rmw_init` in `rmw-zenoh-rs` applies the pin. `hiroz` does not call it, so a native
//!   hiroz application gets no pin.
//! - The pin changes the allocator of the whole process and overrides any earlier `mallopt`
//!   call of the host application. It detects only the environment variables
//!   `MALLOC_TRIM_THRESHOLD_` and `MALLOC_MMAP_THRESHOLD_`, not `GLIBC_TUNABLES`.
//! - The trim threshold of 256 MiB speeds up a 32 MiB typed receive on the main thread. A receive
//!   on a spawned thread, and receives of 1 MiB and 8 MiB, match a threshold of 64 MiB.
//! - Payloads of 64 MiB and more were not measured. The size of a request need not equal the
//!   size of the message.
//! - Nothing in CI checks the timing. The gate is opt-in and needs a quiet host.
//! - A source check guards the `rmw_init` call, because `rmw-zenoh-rs` needs ROS headers to
//!   build. It fails when the call text is removed. It does not fail when the call is present
//!   but unreachable.

/// Result of one call to [`pin_large_message_thresholds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinOutcome {
    /// glibc accepted both thresholds. The fields are the `mallopt` return codes.
    Pinned { mmap_rc: i32, trim_rc: i32 },
    /// glibc rejected a threshold. The fields are the `mallopt` return codes.
    ///
    /// The mmap threshold is set first. When glibc rejects it, the trim call is skipped and
    /// `trim_rc` is 0, which means the call was not attempted. The call then leaves the earlier
    /// settings of both thresholds unchanged. A trim threshold of 256 MiB without the mmap
    /// threshold is a combination that was not measured.
    Rejected { mmap_rc: i32, trim_rc: i32 },
    /// The operator set a glibc environment variable. The pin changes nothing.
    OperatorSet,
    /// The target is not Linux with glibc. The pin changes nothing.
    Unsupported,
}

/// A one-line description for logs. A rejected mmap threshold reports that the trim call did
/// not run, because its return code of 0 would read as a second rejection.
impl core::fmt::Display for PinOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pinned { mmap_rc, trim_rc } => {
                write!(f, "mallopt rc: mmap={mmap_rc}, trim={trim_rc}")
            }
            Self::Rejected { mmap_rc: 0, .. } => {
                f.write_str("mallopt rc: mmap=0, trim not attempted")
            }
            Self::Rejected { mmap_rc, trim_rc } => {
                write!(f, "mallopt rc: mmap={mmap_rc}, trim={trim_rc}")
            }
            Self::OperatorSet => f.write_str("operator set"),
            Self::Unsupported => f.write_str("unsupported"),
        }
    }
}

/// Value of `M_MMAP_THRESHOLD`, in bytes.
pub const MMAP_THRESHOLD: i32 = 64 << 20;
/// Value of `M_TRIM_THRESHOLD`, in bytes.
///
/// glibc trims the top chunk of the main heap when its free size reaches this value. It is a
/// trigger and not a cap: free chunks below the top are not bound by it, and `M_TOP_PAD` adds to
/// the top. A 32 MiB receive leaves two 32 MiB buffers free at the top. A lower value makes
/// glibc trim after every message, so the next receive pays page faults again. A higher value
/// lets the process keep more free heap after a burst of large messages.
///
/// Measured at 32 MiB on the main thread. The raise from 64 MiB does not change the latency of
/// a receive on a spawned thread. Other message sizes, executors, RMW paths, hosts and glibc
/// versions were not measured. Operators can keep their own values with
/// `MALLOC_TRIM_THRESHOLD_` or `MALLOC_MMAP_THRESHOLD_`.
pub const TRIM_THRESHOLD: i32 = 256 << 20;

/// Pin the glibc allocator thresholds for large message payloads.
///
/// Both calls are needed together. `mallopt()` on either parameter disables the automatic
/// adjustment of both thresholds (see `mallopt(3)`). Pinning `M_TRIM_THRESHOLD` alone leaves
/// `M_MMAP_THRESHOLD` at its 128 KiB default, so payload-sized requests that the heap cannot
/// serve go through `mmap`. The dynamic ceiling of `M_MMAP_THRESHOLD` is about 32 MiB, so the
/// 64 MiB value is a deliberate override.
///
/// The function skips both calls when `MALLOC_TRIM_THRESHOLD_` or `MALLOC_MMAP_THRESHOLD_` is
/// set, because it keeps those two explicit deployment choices. `GLIBC_TUNABLES` is not
/// detected, so `mallopt` overrides a value set there.
///
/// glibc documents `mallopt` as not safe against concurrent allocation. Call this function at
/// startup, before the process starts other threads that allocate.
pub fn pin_large_message_thresholds() -> PinOutcome {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn mallopt(param: core::ffi::c_int, value: core::ffi::c_int) -> core::ffi::c_int;
        }
        if std::env::var_os("MALLOC_TRIM_THRESHOLD_").is_some()
            || std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_some()
        {
            return PinOutcome::OperatorSet;
        }
        pin_with(|param, value| unsafe { mallopt(param, value) })
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        PinOutcome::Unsupported
    }
}

/// The decision of the pin, with the `mallopt` call injected. A test uses it to drive the
/// rejection path without a glibc that rejects. Production code calls
/// [`pin_large_message_thresholds`] instead.
///
/// `mallopt` takes the glibc parameter number and the value, and returns 1 on success and 0 on
/// rejection. The function sets `M_MMAP_THRESHOLD` first and returns
/// [`PinOutcome::Rejected`] with `trim_rc` 0, without a trim call, when that call is rejected.
#[doc(hidden)]
pub fn pin_with(mut mallopt: impl FnMut(i32, i32) -> i32) -> PinOutcome {
    const M_TRIM_THRESHOLD: i32 = -1;
    const M_MMAP_THRESHOLD: i32 = -3;
    let mmap_rc = mallopt(M_MMAP_THRESHOLD, MMAP_THRESHOLD);
    if mmap_rc == 0 {
        return PinOutcome::Rejected {
            mmap_rc,
            trim_rc: 0,
        };
    }
    let trim_rc = mallopt(M_TRIM_THRESHOLD, TRIM_THRESHOLD);
    if trim_rc == 0 {
        PinOutcome::Rejected { mmap_rc, trim_rc }
    } else {
        PinOutcome::Pinned { mmap_rc, trim_rc }
    }
}
