//! Helper process of `malloc_pin_receive`: one publisher or one typed receiver.
//!
//! The receiver loop must run on the process main thread, because glibc serves the main
//! thread from the main arena and every other thread from its own arena. A libtest test body
//! never runs on the main thread, so the measurement lives in a separate binary.
//!
//! Usage:
//!   malloc_probe pub  <size_bytes> <hz> <count>
//!   malloc_probe recv <arm> <count>
//! Arms: `none`, `lib` (the pin of the library), `raw:<mmap_mib>:<trim_mib>` (a probe value),
//! each optionally followed by `+spawn` (receive on a spawned thread) or `+delay<ms>`
//! (sleep before the receive time is read, a deliberately slowed arm).
//!
//! The probe measures Linux glibc behaviour. On another target, musl included, it compiles to a stub that exits
//! with status 2.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn main() {
    linux::run();
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn main() {
    eprintln!("malloc_probe supports Linux with glibc only");
    std::process::exit(2);
}

// The module file carries its own .
#[path = "malloc_probe/linux.rs"]
mod linux;
