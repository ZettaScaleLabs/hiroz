//! `BufferGroup` is `#[non_exhaustive]`: a crate outside hiroz must match it with a wildcard arm.
//!
//! This crate is separate from hiroz, so it sees the enum the way a user does. The match below
//! carries the wildcard arm that `#[non_exhaustive]` requires. A match with no wildcard arm
//! cannot be a test, because it fails to compile once the attribute is present. A scratch crate
//! checks that side by hand.

#[allow(dead_code)]
mod common;

use hiroz::BufferGroup;

const NAME: &str = "buffer_group_matches_with_a_wildcard_arm";

/// The name of a value. The wildcard arm is required outside hiroz once the enum is
/// `#[non_exhaustive]`. It is unreachable before the attribute, so the lint is allowed here.
#[allow(unreachable_patterns)]
fn name_of(group: BufferGroup) -> &'static str {
    match group {
        BufferGroup::Auto => "auto",
        BufferGroup::Cpu => "cpu",
        BufferGroup::Off => "off",
        _ => "other",
    }
}

#[test]
fn buffer_group_matches_with_a_wildcard_arm() {
    for (group, want) in [
        (BufferGroup::Auto, "auto"),
        (BufferGroup::Cpu, "cpu"),
        (BufferGroup::Off, "off"),
    ] {
        let got = name_of(group);
        r1_assert!(
            got == want,
            NAME,
            "{group:?} mapped to {got:?}, want {want:?}"
        );
    }
}
