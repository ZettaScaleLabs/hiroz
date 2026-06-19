pub mod limits;

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

// ─── C ABI plugin interface ───────────────────────────────────────────────────
//
// These symbols are exported when the crate is compiled as a `cdylib` so that
// `hu` can dlopen `libhu_bridge.so` and dispatch without a subprocess.
//
// Stability contract: the three symbol names and their signatures are fixed.
// Breaking changes require a new major ABI version.

/// Plugin name (null-terminated UTF-8, static lifetime).
#[unsafe(no_mangle)]
pub extern "C" fn hu_plugin_name() -> *const std::ffi::c_char {
    b"bridge\0".as_ptr().cast()
}

/// Plugin version (null-terminated UTF-8, static lifetime).
#[unsafe(no_mangle)]
pub extern "C" fn hu_plugin_version() -> *const std::ffi::c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Plugin entry point. `argv[0]` should be the plugin name (e.g. `"hu-bridge"`);
/// remaining elements are the subcommand and flags.
///
/// # Safety
/// `argv` must be a valid C array of `argc` null-terminated UTF-8 strings.
/// The function is synchronous: it blocks until the bridge exits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hu_plugin_main(
    argc: std::ffi::c_int,
    argv: *const *const std::ffi::c_char,
) -> std::ffi::c_int {
    let args: Vec<String> = (0..argc as usize)
        .map(|i| {
            unsafe { std::ffi::CStr::from_ptr(*argv.add(i)) }
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    cli::run_argv(&args)
}
