// Dynamic plugin loading via the C ABI exported by `cdylib` plugin crates.
//
// Search order for `libhu_<name>.so`:
//   1. Directories in `HU_PLUGIN_PATH` (colon-separated)
//   2. `~/.local/share/hu/plugins/`
//   3. `/usr/lib/hu/plugins/`
//
// Returns `Some(exit_code)` if the `.so` was found and dispatched, `None` to
// fall through to the subprocess path.

use std::{
    ffi::{CString, c_char, c_int},
    path::PathBuf,
};

type FnPluginMain = unsafe extern "C" fn(argc: c_int, argv: *const *const c_char) -> c_int;

fn plugin_so_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![];
    if let Ok(p) = std::env::var("HU_PLUGIN_PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/hu/plugins"));
    }
    dirs.push(PathBuf::from("/usr/lib/hu/plugins"));
    dirs
}

/// Try to find and dlopen `libhu_<name>.so`, call `hu_plugin_main`, and return
/// the exit code. Returns `None` if no `.so` was found in any search directory.
pub fn try_dispatch_so(plugin_name: &str, args: &[String]) -> Option<i32> {
    let lib_name = format!("libhu_{}.so", plugin_name.replace('-', "_"));

    for dir in plugin_so_dirs() {
        let lib_path = dir.join(&lib_name);
        if !lib_path.exists() {
            continue;
        }

        // SAFETY: The plugin must export `hu_plugin_main` with the C ABI defined
        // in `hiroz-bridge/src/lib.rs`. We block until it returns.
        let result = unsafe { libloading::Library::new(&lib_path) }.and_then(|lib| {
            let f: libloading::Symbol<FnPluginMain> = unsafe { lib.get(b"hu_plugin_main\0") }?;
            let c_strings: Vec<CString> = args
                .iter()
                .map(|s| CString::new(s.as_str()).unwrap_or_default())
                .collect();
            let c_ptrs: Vec<*const c_char> = c_strings.iter().map(|s| s.as_ptr()).collect();
            let code = unsafe { f(c_ptrs.len() as c_int, c_ptrs.as_ptr()) };
            // Keep lib alive until f returns (it is a blocking call).
            drop(lib);
            Ok(code as i32)
        });

        match result {
            Ok(code) => return Some(code),
            Err(e) => {
                tracing::warn!(
                    "Failed to load plugin {} from {}: {e}",
                    plugin_name,
                    lib_path.display()
                );
            }
        }
    }
    None
}
