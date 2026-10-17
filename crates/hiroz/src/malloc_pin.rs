//! Allocator pin for large message payloads (glibc only).
//!
//! glibc returns the heap top to the kernel when it grows past `M_TRIM_THRESHOLD`, and serves
//! a request above `M_MMAP_THRESHOLD` with a fresh `mmap`. A receiver that allocates a payload
//! buffer per message then pays page faults on every message. Pinning both thresholds keeps
//! the payload heap in the process.
//!
//! Scope:
//! - Only `rmw_init` in `rmw-zenoh-rs` applies the pin. `hiroz` does not call it, so an
//!   application that uses `hiroz` directly gets no pin and must call
//!   [`crate::malloc_pin::pin_large_message_thresholds`] itself.
//! - The pin changes the allocator of the whole process. It overrides any earlier `mallopt`
//!   call of the host application. It detects only the two glibc environment variables `MALLOC_TRIM_THRESHOLD_` and
//!   `MALLOC_MMAP_THRESHOLD_`, not `GLIBC_TUNABLES`.
//! - A message of 64 MiB or more is always served by `mmap` and sees no benefit. At 64 MiB
//!   the receive latency was the same with no pin, with the previous 64/64 pin and with this
//!   pin.
//! - The timing gate for the pin (`malloc_pin_receive.rs` in `hiroz-tests`) is opt-in. CI does
//!   not run it, because it holds only on a quiet host (load average below 0.6). The command is
//!   in the module doc of that test.
//! - The `rmw_init` call is guarded only by a source check
//!   (`malloc_pin_wiring.rs` in `hiroz-tests`), because `rmw-zenoh-rs` needs ROS headers and
//!   cannot build without them. The check fails when the call text is removed. It does not
//!   fail when the call is present but unreachable.

/// Result of one call to [`pin_large_message_thresholds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinOutcome {
    /// glibc accepted both thresholds. The fields are the `mallopt` return codes.
    Pinned { mmap_rc: i32, trim_rc: i32 },
    /// glibc rejected a threshold. The fields are the `mallopt` return codes.
    ///
    /// The mmap threshold is set first. When glibc rejects it, the trim call is skipped and
    /// `trim_rc` is 0, so both thresholds keep their defaults. A trim threshold of 256 MiB with
    /// the default mmap threshold would measure worse than no pin.
    Rejected { mmap_rc: i32, trim_rc: i32 },
    /// The operator set a glibc environment variable. The pin changes nothing.
    OperatorSet,
    /// The target is not Linux with glibc. The pin changes nothing.
    Unsupported,
}

/// Value of `M_MMAP_THRESHOLD`, in bytes.
pub const MMAP_THRESHOLD: i32 = 64 << 20;
/// Value of `M_TRIM_THRESHOLD`, in bytes.
///
/// It must stay above the free heap top that a receive leaves behind. A 32 MiB payload leaves
/// two 32 MiB buffers. The measured system time per message with a threshold of 64 MiB is
/// consistent with glibc trimming the heap on every message, so the receive pays page faults
/// again. No trace of `brk` or `madvise` calls confirmed this. The process keeps up to this many free bytes at the heap
/// top of each arena, so a larger value costs resident memory after a burst of large messages.
///
/// Measured cost: a typed `UInt8MultiArray` of 32 MiB at 10 Hz, received on the main thread
/// (glibc 2.42, a quiet 16-core AMD host, n=5 per arm). Steady RSS was 163.4-163.7 MiB with the pin and
/// 91.7-99.5 MiB without it. The peak RSS did not change. Only the main-thread arena was
/// measured. Every other arena (callback threads, `async_recv`) can retain up to this
/// threshold, and that is not measured.
pub const TRIM_THRESHOLD: i32 = 256 << 20;

/// Pin the glibc allocator thresholds for large message payloads.
///
/// Both calls are needed together. `mallopt()` on either parameter disables glibc's automatic
/// adjustment of both (see `mallopt(3)`). Pinning `M_TRIM_THRESHOLD` alone leaves
/// `M_MMAP_THRESHOLD` at its 128 KiB default, so every payload-sized buffer goes through
/// `mmap`, which measures worse than no pin. glibc's own dynamic ceiling for
/// `M_MMAP_THRESHOLD` is about 32 MiB, so the 64 MiB value is a deliberate override that glibc
/// would never reach on its own. Skips both calls when the operator has set
/// `MALLOC_TRIM_THRESHOLD_` or `MALLOC_MMAP_THRESHOLD_`: those two explicit deployment choices
/// are never overridden. `GLIBC_TUNABLES` is not detected, so `mallopt` overrides a value set
/// there. The call changes the allocator of the whole process.
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

/// The decision of the pin, with the `mallopt` call injected. It exists so a test can drive the
/// rejection path without a glibc that rejects. Applications call
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
