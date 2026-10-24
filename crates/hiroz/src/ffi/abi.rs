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
