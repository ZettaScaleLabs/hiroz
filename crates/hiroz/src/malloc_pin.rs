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
//!   call of the host application. It detects only the glibc environment variables.
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
pub enum PinOutcome {
    /// glibc accepted both thresholds. The fields are the `mallopt` return codes.
    Pinned { mmap_rc: i32, trim_rc: i32 },
    /// glibc rejected a threshold. The fields are the `mallopt` return codes.
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
/// two 32 MiB buffers, so a threshold of 64 MiB trimmed the heap on every message and the
/// receive paid page faults again. The process keeps up to this many free bytes at the heap
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
/// `MALLOC_TRIM_THRESHOLD_` or `MALLOC_MMAP_THRESHOLD_`: an explicit deployment choice is never
/// overridden. The call changes the allocator of the whole process.
pub fn pin_large_message_thresholds() -> PinOutcome {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        const M_TRIM_THRESHOLD: core::ffi::c_int = -1;
        const M_MMAP_THRESHOLD: core::ffi::c_int = -3;
        unsafe extern "C" {
            fn mallopt(param: core::ffi::c_int, value: core::ffi::c_int) -> core::ffi::c_int;
        }
        if std::env::var_os("MALLOC_TRIM_THRESHOLD_").is_some()
            || std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_some()
        {
            return PinOutcome::OperatorSet;
        }
        let mmap_rc = unsafe { mallopt(M_MMAP_THRESHOLD, MMAP_THRESHOLD) };
        let trim_rc = unsafe { mallopt(M_TRIM_THRESHOLD, TRIM_THRESHOLD) };
        if mmap_rc == 0 || trim_rc == 0 {
            PinOutcome::Rejected { mmap_rc, trim_rc }
        } else {
            PinOutcome::Pinned { mmap_rc, trim_rc }
        }
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        PinOutcome::Unsupported
    }
}
