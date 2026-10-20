//! Helper process of `malloc_pin_receive`: one publisher or one typed receiver.
//!
//! The receiver loop runs on the process main thread, because by default glibc serves the main
//! thread from the main arena and other threads from arenas of their own. `MALLOC_ARENA_MAX`
//! changes that assignment. A libtest test body never runs on the main thread, so the
//! measurement lives in a separate binary.
//!
//! Usage:
//!   malloc_probe pub  <size_bytes> <hz> <count> [skip<n>|dup<n>]
//!   malloc_probe recv <arm> <count>
//!   malloc_probe pin
//! Arms: `none`, `lib` (the pin of the library), `raw:<mmap_mib>:<trim_mib>` (a probe value),
//! each optionally followed by `+spawn` (receive on a spawned thread) or `+delay<ms>`
//! (sleep before the receive time is read, a deliberately slowed arm).
//!
//! Every message carries its send time and a sequence id. The receiver exits with status 5 and
//! the marker `IDGATE` when an id after the first arrival is not the next one. The publisher
//! modifier `skip<n>` makes the id sequence jump over one id at message `n`, and `dup<n>` repeats
//! an id there, so a test can prove the check fires. `pin` calls the production pin alone and
//! prints `PIN outcome=<outcome>`.
//!
//! The probe measures Linux glibc behaviour. On another target, musl included, it compiles to a
//! stub that exits with status 2.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn main() {
    linux::run();
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn main() {
    eprintln!("malloc_probe supports Linux with glibc only");
    std::process::exit(2);
}

// The Linux body is a separate file. Its own cfg attribute leaves it empty on other targets.
#[path = "malloc_probe/linux.rs"]
mod linux;
