pub mod limits;
pub mod status;

#[cfg(feature = "cross-distro")]
pub mod distro;

#[cfg(feature = "cross-dds")]
pub mod dds;

mod cli;

/// Run the bridge from a slice of argv strings (argv[0] is the program name).
/// Returns 0 on success, non-zero on failure.
pub fn run_argv(argv: &[String]) -> i32 {
    cli::run_argv(argv)
}
