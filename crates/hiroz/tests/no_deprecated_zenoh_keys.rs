//! The shipped configs must set no key the zenoh version in use has removed.
//!
//! # What this pins
//!
//! `routing.router.peers_failover_brokering` governed whether a router forwards
//! between two peers attached to it that cannot reach each other. zenoh 1.9
//! removed it. The runtime says so on every start of anything that sets it:
//!
//! ```text
//! WARN zenoh_config: `routing.router.peers_failover_brokering` is deprecated
//! and has no effect; please remove it from your configuration
//! ```
//!
//! hiroz set it in two places, and they disagreed with each other. The library
//! set `false`; `rmw-zenoh-rs`'s vendored session config set `true`. Neither
//! did anything.
//!
//! # Why a test rather than a one-off deletion
//!
//! A deprecated key is invisible. It compiles, it validates, it produces a
//! warning nobody reads, and it stays right until someone measures. Deleting
//! the two occurrences fixes today; this test is what notices the next one.
//!
//! # What this test cannot do
//!
//! It knows only the keys named in `REMOVED_KEYS`, so it catches a
//! reintroduction rather than a newly deprecated key. A zenoh upgrade that
//! retires something else passes this test in silence. Adding the new name here
//! is part of taking that upgrade.

use hiroz::config::{router_overrides, session_overrides};

/// Keys the zenoh version this crate builds against no longer honours.
///
/// Each entry is the key as `ConfigOverride` spells it, the version that
/// removed it, and what replaced it.
/// A key the zenoh version this crate builds against no longer honours.
///
/// `override_token` is what a `ConfigOverride` key path contains.
/// `file_tokens` are what a JSON5 config file contains, because a file spells
/// the same key as nested blocks rather than a path. The two surfaces need
/// different needles for the same key.
struct RemovedKey {
    override_token: &'static str,
    file_tokens: &'static [&'static str],
    removed_in: &'static str,
    replacement: &'static str,
}

const REMOVED_KEYS: &[RemovedKey] = &[
    RemovedKey {
        override_token: "peers_failover_brokering",
        file_tokens: &["peers_failover_brokering"],
        removed_in: "zenoh 1.9",
        replacement: "a router per host, or peer subregions",
    },
    // zenoh 1.9 removed `routing.peer` entirely; every peer is peer-to-peer
    // now. Its deserializer warns about `mode` and `linkstate` alike, so the
    // override side matches the parent and the file side matches the values,
    // which are what a JSON5 block actually contains.
    RemovedKey {
        override_token: "routing/peer",
        file_tokens: &["peer_to_peer", "linkstate"],
        removed_in: "zenoh 1.9",
        replacement: "nothing; peer-to-peer is the only behaviour",
    },
];

/// Neither shipped override list may name a key zenoh has removed.
///
/// This inspects what hiroz *sets*, not the `zenoh::Config` it produces. The
/// rendered config carries every key with its default, so it cannot tell an
/// override from a field that merely exists — an earlier version of this test
/// checked that and failed on a clean tree.
#[test]
fn the_shipped_overrides_name_no_removed_key() {
    for (label, overrides) in [
        ("session", session_overrides()),
        ("router", router_overrides()),
    ] {
        // An empty list would satisfy every assertion below while proving
        // nothing.
        assert!(
            !overrides.is_empty(),
            "the {label} override list is empty; this test would pass vacuously"
        );

        for removed in REMOVED_KEYS {
            let found = overrides
                .iter()
                .find(|o| o.key.contains(removed.override_token));
            assert!(
                found.is_none(),
                "the shipped {label} overrides set `{}`, which {} removed. \
                 It has no effect, and the runtime warns about it at every start. \
                 Use {} instead.",
                found.map(|o| o.key).unwrap_or(removed.override_token),
                removed.removed_in,
                removed.replacement
            );
        }
    }
}
/// The vendored `rmw-zenoh-rs` config files may not set one either.
///
/// Those are JSON5 shipped as files rather than built through `ConfigOverride`,
/// so the check above cannot see them. This is the path that carried
/// `peers_failover_brokering: true` — in a *session* config, where a key about
/// router forwarding has no meaning even on a runtime that honoured it.
#[test]
fn the_vendored_config_files_set_no_removed_key() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../rmw-zenoh-rs/config");
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json5"))
        .collect::<Vec<_>>();

    // An empty sweep would pass while checking nothing.
    assert!(
        !entries.is_empty(),
        "no .json5 config files found under {dir}; this test would pass vacuously"
    );

    for entry in entries {
        let path = entry.path();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

        for removed in REMOVED_KEYS {
            // Skip commented lines: the upstream files carry long `///` blocks
            // that legitimately name the key while documenting it.
            let live = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .find(|l| removed.file_tokens.iter().any(|t| l.contains(t)));

            assert!(
                live.is_none(),
                "{} sets `{}`, which {} removed. Use {} instead. Offending line: {}",
                path.display(),
                removed.override_token,
                removed.removed_in,
                removed.replacement,
                live.unwrap_or("").trim()
            );
        }
    }
}
