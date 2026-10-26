//! ABI version of the C interface.

/// Version of the C ABI this library exports.
///
/// Bump this integer whenever a change removes or alters an exported function, struct layout or error code.
/// Adding a function does not change it.
/// This function never returns 0. A library that predates it does not export the symbol.
const ABI_VERSION: u32 = 1;

/// Return the version of the C ABI this library exports. A binding compares it with the version it was built against.
#[unsafe(no_mangle)]
pub extern "C" fn hiroz_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_matches_constant() {
        assert_eq!(hiroz_abi_version(), ABI_VERSION);
    }

    #[test]
    fn abi_version_starts_at_one() {
        // Zero means "library predates hiroz_abi_version()", so a real version is never 0.
        assert!(hiroz_abi_version() >= 1);
    }
}
