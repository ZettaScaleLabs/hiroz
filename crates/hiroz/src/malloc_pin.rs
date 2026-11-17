//! Allocator pin for large message payloads (glibc only).
//!
//! glibc trims the top of the main heap when the free top chunk reaches `M_TRIM_THRESHOLD`. It
//! serves a request at or above `M_MMAP_THRESHOLD` with a fresh `mmap`, but only when the bins
//! and the top chunk cannot serve it. A receiver that allocates a payload buffer per message can
//! then pay page faults on every message. Pinning both thresholds keeps the payload heap in the
//! process.
//!
//! This module is internal to the hiroz crates and carries no stability promise. It is `pub` only
//! because `rmw-zenoh-rs` and the test crate call it. Only `rmw_init` applies the pin, so a native
//! hiroz application gets no pin.

/// Result of one call to [`pin_large_message_thresholds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinOutcome {
    /// glibc accepted both thresholds. The fields are the `mallopt` return codes.
    Pinned { mmap_rc: i32, trim_rc: i32 },
    /// glibc rejected a threshold. The fields are the `mallopt` return codes.
    ///
    /// The mmap threshold is set first. When glibc rejects it, the trim call is skipped and
    /// `trim_rc` is 0, which means the call was not attempted.
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
const MMAP_THRESHOLD: i32 = 64 << 20;
/// Value of `M_TRIM_THRESHOLD`, in bytes.
///
/// glibc trims the top chunk of the main heap when its free size reaches this value. It is a
/// trigger and not a cap. A 32 MiB receive leaves two 32 MiB buffers free at the top, so a lower
/// value makes glibc trim after every message and the next receive pays page faults again. A
/// higher value lets the process keep more free heap after a burst of large messages.
///
/// Measured only for a 32 MiB typed receive on the main thread. Other sizes, threads, executors,
/// RMW paths, hosts and glibc versions were not measured.
const TRIM_THRESHOLD: i32 = 256 << 20;

/// Pin the glibc allocator thresholds for large message payloads.
///
/// Both calls are needed together. `mallopt()` on either parameter disables the automatic
/// adjustment of both thresholds (see `mallopt(3)`). Pinning `M_TRIM_THRESHOLD` alone leaves
/// `M_MMAP_THRESHOLD` at its 128 KiB default, so payload-sized requests that the heap cannot
/// serve go through `mmap`.
///
/// The function skips both calls when `MALLOC_TRIM_THRESHOLD_` or `MALLOC_MMAP_THRESHOLD_` is
/// set. `GLIBC_TUNABLES` is not detected, so `mallopt` overrides a value set there.
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

/// The decision of the pin, with the `mallopt` call injected so a test can drive the rejection
/// path. `mallopt` returns 1 on success and 0 on rejection.
#[cfg_attr(
    not(all(target_os = "linux", target_env = "gnu")),
    allow(dead_code)
)]
fn pin_with(mut mallopt: impl FnMut(i32, i32) -> i32) -> PinOutcome {
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

/// glibc parameter numbers of `mallopt`.
const M_TRIM_THRESHOLD: i32 = -1;
const M_MMAP_THRESHOLD: i32 = -3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_thresholds_set_mmap_first_then_trim() {
        let mut calls = vec![];
        let out = pin_with(|p, v| {
            calls.push((p, v));
            1
        });
        assert_eq!(
            out,
            PinOutcome::Pinned {
                mmap_rc: 1,
                trim_rc: 1
            }
        );
        assert_eq!(
            calls,
            vec![
                (M_MMAP_THRESHOLD, MMAP_THRESHOLD),
                (M_TRIM_THRESHOLD, TRIM_THRESHOLD)
            ]
        );
    }

    #[test]
    fn rejected_mmap_threshold_skips_the_trim_call() {
        let mut calls = vec![];
        let out = pin_with(|p, v| {
            calls.push((p, v));
            i32::from(p != M_MMAP_THRESHOLD)
        });
        assert_eq!(
            out,
            PinOutcome::Rejected {
                mmap_rc: 0,
                trim_rc: 0
            }
        );
        assert_eq!(calls, vec![(M_MMAP_THRESHOLD, MMAP_THRESHOLD)]);
    }

    #[test]
    fn rejected_trim_threshold_is_reported() {
        let out = pin_with(|p, _| i32::from(p != M_TRIM_THRESHOLD));
        assert_eq!(
            out,
            PinOutcome::Rejected {
                mmap_rc: 1,
                trim_rc: 0
            }
        );
    }

    #[test]
    fn rejected_mmap_threshold_is_described_without_a_trim_return_code() {
        let text = PinOutcome::Rejected {
            mmap_rc: 0,
            trim_rc: 0,
        }
        .to_string();
        assert!(text.contains("trim not attempted"), "{text}");
        assert!(!text.contains("trim=0"), "{text}");
    }

    #[test]
    fn rejected_trim_threshold_is_described_with_its_return_code() {
        // Positive control of the test above: a trim call that ran and failed is reported as such.
        let text = PinOutcome::Rejected {
            mmap_rc: 1,
            trim_rc: 0,
        }
        .to_string();
        assert!(text.contains("trim=0"), "{text}");
        assert!(!text.contains("not attempted"), "{text}");
    }
}
