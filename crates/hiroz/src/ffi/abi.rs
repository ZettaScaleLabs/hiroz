//! ABI version of the C interface.

/// Version of the C ABI this library exports.
///
/// This integer counts breaking changes only. It changes when an existing C symbol, struct layout or error code value changes incompatibly.
/// An addition never changes it. A binding detects an added symbol by probing for it, with `dlsym` or at link time, and not by comparing numbers.
/// Zero is never a version: a library without `hiroz_abi_version` does not export the symbol.
const ABI_VERSION: u32 = 1;

/// Return the breaking-change counter of the C ABI this library exports.
///
/// The value changes only when an existing C symbol, struct layout or error code value changes incompatibly. It never changes for an addition. Probe for an added symbol instead of comparing numbers.
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
