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
    fn abi_version_is_pinned() {
        // Update this literal in the same change as any bump of `ABI_VERSION`.
        assert_eq!(hiroz_abi_version(), 1);
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn abi_version_is_never_zero() {
        // Zero is reserved and never a real version.
        assert_ne!(hiroz_abi_version(), 0);
    }
}
