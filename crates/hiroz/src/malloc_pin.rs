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
//! - The raise of the trim threshold from 64 MiB to 256 MiB speeds up a 32 MiB typed receive
//!   on the main thread. A receive on a spawned thread, and receives of 8 MiB or less, match the
//!   64 MiB threshold. Figures, conditions and what they do not cover are in
//!   [`crate::malloc_pin::TRIM_THRESHOLD`].
//! - Payloads of 64 MiB and more were not measured. glibc serves a request at or above the
//!   mmap threshold with `mmap` unless `M_MMAP_MAX` disables it, and the size of a request need
//!   not equal the size of the message.
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
    /// `trim_rc` is 0, which means the call was not attempted. The call then leaves the earlier
    /// settings of both thresholds unchanged. A trim threshold of 256 MiB without the mmap
    /// threshold is a combination that was not measured.
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
/// two 32 MiB buffers. With a threshold of 64 MiB the receiver spent about 30 ms of system time
/// per message at 32 MiB on the main thread, against about 6 ms with 256 MiB (medians of n=5,
/// ranges 29.7-30.5 and 5.8-6.5 ms, same conditions as below). That is consistent with glibc
/// trimming the heap on every message, so the receive pays page faults again. No trace of `brk`
/// or `madvise` calls confirmed this.
///
/// Measured effect of the raise from 64 to 256 MiB. Every figure is a median over n=5 fresh
/// receiver processes per arm and case, each with its own median p50 over 60 messages.
/// The build is `--release`, on one 16-core AMD host with glibc 2.42, one binary for all
/// arms. The receiver is pinned to cores. The path is a typed `UInt8MultiArray` on direct
/// loopback TCP with `recv_timeout`, at 10 Hz. Arms are no pin, the previous 64/64 pin, and this
/// 64/256 pin.
///
/// - 32 MiB, main thread: this pin 18.8 ms, no pin 36.1 ms, the 64/64 pin 42.7 ms. The three
///   ranges are disjoint, so the 64/64 pin was slower than no pin there.
/// - 32 MiB, spawned thread: this pin 24.0 ms and the 64/64 pin 24.1 ms overlap. Both are below
///   no pin at 35.9 ms, with disjoint ranges. The raise adds nothing there.
/// - 8 MiB and 1 MiB: this pin and the 64/64 pin overlap. On the main thread both are below no
///   pin, with disjoint ranges. On a spawned thread all three arms overlap.
/// - Resident memory at 32 MiB (VmRSS read in the receiver after each message): about 40 MiB
///   more than no pin, 143.6 against 103.7 MiB, for either pin. At 8 MiB the pins cost about
///   8 MiB more on a spawned thread and nothing on the main thread. The 256 MiB value adds no
///   resident memory over the 64 MiB value. The no-pin value is bimodal, so compare the
///   ordering and not the point value. Peak RSS is equal across arms.
///
/// The main arena keeps up to this many free bytes at its heap top, so a larger value can cost
/// resident memory after a burst of large messages. These measurements showed no such cost
/// above the 64 MiB value.
///
/// These figures do not cover any executor, any RMW, other message types, SHM or router
/// transports, other hosts or glibc versions, message sizes between 1, 8 and 32 MiB, or sizes
/// above 32 MiB. Arena assignment depends on `MALLOC_ARENA_MAX` and on earlier frees, so other
/// workloads can differ. Operators can keep their own values with `MALLOC_TRIM_THRESHOLD_` or
/// `MALLOC_MMAP_THRESHOLD_`.
pub const TRIM_THRESHOLD: i32 = 256 << 20;

/// Pin the glibc allocator thresholds for large message payloads.
///
/// Both calls are needed together. `mallopt()` on either parameter disables glibc's automatic
/// adjustment of both (see `mallopt(3)`). Pinning `M_TRIM_THRESHOLD` alone leaves
/// `M_MMAP_THRESHOLD` at its 128 KiB default, so every payload-sized buffer goes through
/// `mmap`. glibc's own dynamic ceiling for
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
