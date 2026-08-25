//! Pin every hiroz zenoh override against `rmw_zenoh_cpp`'s own configuration.
//!
//! `crates/hiroz/src/config.rs` says it generates "rmw_zenoh_cpp compatible
//! configs". This test holds it to that.
//!
//! # This is not an equality check
//!
//! Matching upstream is not the same as being correct. hiroz's session
//! `listen/endpoints` matched `rmw_zenoh_cpp` exactly. Two nodes on two hosts
//! still delivered nothing to each other. Upstream's loopback locator depends
//! on zenoh 1.8 router relaying, and zenoh 1.9 removed it. hiroz runs 1.9.
//!
//! So this test asserts something weaker and more useful: every difference
//! appears in `DIVERGENCES` with a reason, and every listed reason still
//! describes a real difference. A divergence becomes a written decision
//! instead of a silent edit.
//!
//! # What it reads
//!
//! It reads the vendored copies under `tests/data/rmw_zenoh_cpp/`. Where
//! `AMENT_PREFIX_PATH` names an installed `rmw_zenoh_cpp`, it reads that
//! instead. The four ROS interop legs take the second path, so they also
//! catch the vendored copies going stale.
//!
//! The vendored copies are byte-identical across all five `rmw_zenoh` branches
//! at `e95c62d`. One copy therefore serves every distro.
//!
//! # What it does not cover
//!
//! It compares only the keys hiroz overrides. A key that `rmw_zenoh_cpp` sets
//! and hiroz leaves at zenoh's default is invisible here. One such key exists:
//! `rmw_zenoh_cpp` sets `queries_default_timeout` to 10 minutes on the router,
//! and hiroz keeps zenoh's 10 seconds there. Covering that direction needs a
//! second allow-list, because most of upstream's remaining settings are moot
//! for hiroz — multicast keys under disabled multicast, and keys zenoh 1.9
//! deprecated.

use hiroz::config::{ConfigOverride, router_overrides, session_overrides};
use serde_json::Value;

const VENDORED_SESSION: &str =
    include_str!("data/rmw_zenoh_cpp/DEFAULT_RMW_ZENOH_SESSION_CONFIG.json5");
const VENDORED_ROUTER: &str =
    include_str!("data/rmw_zenoh_cpp/DEFAULT_RMW_ZENOH_ROUTER_CONFIG.json5");

/// Which of the two configurations an override belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Router,
    Session,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Router => "router",
            Role::Session => "session",
        }
    }
}

/// Every place hiroz deliberately differs from `rmw_zenoh_cpp`, and why.
///
/// A difference not listed here fails the test. A difference listed here that
/// is no longer a difference *also* fails it — a stale entry is how an
/// allow-list quietly turns into a blanket exemption.
///
/// Empty means hiroz currently matches upstream on every override. Adding an
/// entry is how you record a deliberate departure; the reason is the point of
/// it, so write the mechanism, not "intentional".
const DIVERGENCES: &[(Role, &str, &str)] = &[];

/// Parse a json5 configuration into a `serde_json::Value`.
fn parse(text: &str, what: &str) -> Value {
    json5::from_str(text).unwrap_or_else(|e| panic!("failed to parse {what} as json5: {e}"))
}

/// Where the reference configuration came from, so a run is never ambiguous
/// about which bytes it checked.
struct Reference {
    router: Value,
    session: Value,
    source: String,
    /// True when the files came from an installed `rmw_zenoh_cpp`.
    installed: bool,
}

/// Every prefix that might hold an installed `rmw_zenoh_cpp`.
///
/// `AMENT_PREFIX_PATH` rather than `/opt/ros/$ROS_DISTRO`, because the latter
/// is Debian packaging's layout and this workspace also builds ROS from Nix,
/// where no `/opt/ros` exists. Both set `AMENT_PREFIX_PATH`.
fn ament_prefixes() -> Vec<std::path::PathBuf> {
    match std::env::var("AMENT_PREFIX_PATH") {
        Ok(v) => v
            .split(':')
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .collect(),
        // Fall back to the Debian layout only when ament said nothing.
        Err(_) => std::env::var("ROS_DISTRO")
            .map(|d| vec![std::path::PathBuf::from(format!("/opt/ros/{d}"))])
            .unwrap_or_default(),
    }
}

/// True when `rmw_zenoh_cpp` is installed at all, whether or not its
/// configuration directory turned out to be where this test looks.
fn rmw_zenoh_cpp_is_installed() -> bool {
    ament_prefixes()
        .iter()
        .any(|p| p.join("share/rmw_zenoh_cpp").is_dir())
}

fn installed_config_dir() -> Option<std::path::PathBuf> {
    ament_prefixes()
        .into_iter()
        .map(|p| p.join("share/rmw_zenoh_cpp/config"))
        .find(|d| d.is_dir())
}

fn reference() -> Reference {
    match installed_config_dir() {
        Some(dir) => {
            let read = |name: &str| {
                let p = dir.join(name);
                std::fs::read_to_string(&p)
                    .unwrap_or_else(|e| panic!("failed to read {}: {e}", p.display()))
            };
            let router_text = read("DEFAULT_RMW_ZENOH_ROUTER_CONFIG.json5");
            let session_text = read("DEFAULT_RMW_ZENOH_SESSION_CONFIG.json5");
            Reference {
                router: parse(&router_text, "installed router config"),
                session: parse(&session_text, "installed session config"),
                source: dir.display().to_string(),
                installed: true,
            }
        }
        None => Reference {
            router: parse(VENDORED_ROUTER, "vendored router config"),
            session: parse(VENDORED_SESSION, "vendored session config"),
            source: "tests/data/rmw_zenoh_cpp (vendored)".to_string(),
            installed: false,
        },
    }
}

/// Follow a hiroz override key (`a/b/c`) into a parsed configuration.
fn lookup<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('/').try_fold(root, |node, part| node.get(part))
}

fn divergence_reason(role: Role, key: &str) -> Option<&'static str> {
    DIVERGENCES
        .iter()
        .find(|(r, k, _)| *r == role && *k == key)
        .map(|(_, _, reason)| *reason)
        // A blank reason exempts the key while explaining nothing. The reason
        // is the whole value of an entry, so an entry without one does not
        // count as an entry.
        .filter(|reason| !reason.trim().is_empty())
}

/// One override compared against upstream.
enum Verdict {
    Same,
    /// Differs, and `DIVERGENCES` explains why.
    DivergesAsDocumented,
    /// Differs with nothing to explain it.
    UndocumentedDrift {
        ours: Value,
        theirs: Value,
    },
    /// hiroz sets a key upstream's configuration does not contain.
    AbsentUpstream {
        ours: Value,
    },
}

fn compare(role: Role, over: &ConfigOverride, upstream: &Value) -> Verdict {
    match lookup(upstream, over.key) {
        None => Verdict::AbsentUpstream {
            ours: over.value.clone(),
        },
        Some(theirs) if *theirs == over.value => Verdict::Same,
        Some(_) if divergence_reason(role, over.key).is_some() => Verdict::DivergesAsDocumented,
        Some(theirs) => Verdict::UndocumentedDrift {
            ours: over.value.clone(),
            theirs: theirs.clone(),
        },
    }
}

#[test]
fn every_override_matches_rmw_zenoh_cpp_or_is_a_documented_divergence() {
    let reference = reference();
    println!("reference configuration: {}", reference.source);

    let mut problems: Vec<String> = Vec::new();
    let mut compared = 0usize;
    let mut matched = 0usize;
    // Divergences observed, so a stale DIVERGENCES entry can be detected.
    let mut observed: Vec<(Role, String)> = Vec::new();

    for (role, overrides, upstream) in [
        (Role::Router, router_overrides(), &reference.router),
        (Role::Session, session_overrides(), &reference.session),
    ] {
        for over in &overrides {
            compared += 1;
            match compare(role, over, upstream) {
                Verdict::Same => matched += 1,
                Verdict::DivergesAsDocumented => observed.push((role, over.key.to_string())),
                Verdict::UndocumentedDrift { ours, theirs } => {
                    observed.push((role, over.key.to_string()));
                    problems.push(format!(
                        "{} config: `{}` is {} in hiroz and {} in rmw_zenoh_cpp.\n    \
                         If that is deliberate, add it to DIVERGENCES in this file with the \
                         reason. If it is not, change crates/hiroz/src/config.rs.",
                        role.as_str(),
                        over.key,
                        ours,
                        theirs,
                    ));
                }
                Verdict::AbsentUpstream { ours } => problems.push(format!(
                    "{} config: hiroz sets `{}` to {} but rmw_zenoh_cpp's configuration has \
                     no such key. Either upstream dropped it, or the key is misspelt.",
                    role.as_str(),
                    over.key,
                    ours,
                )),
            }
        }
    }

    // A DIVERGENCES entry that no longer describes a real difference is worse
    // than no entry: it reads as a considered decision while exempting a key
    // that now matches, and it would go on exempting it after a future edit.
    for (role, key, reason) in DIVERGENCES {
        if reason.trim().is_empty() {
            problems.push(format!(
                "{} config: DIVERGENCES lists `{}` with a blank reason. The reason is what \
                 makes the entry a decision rather than an exemption. Write it.",
                role.as_str(),
                key,
            ));
            continue;
        }
        if !observed.iter().any(|(r, k)| r == role && k == key) {
            problems.push(format!(
                "{} config: DIVERGENCES lists `{}`, but hiroz and rmw_zenoh_cpp now agree on \
                 it. Remove the entry.",
                role.as_str(),
                key,
            ));
        }
    }

    println!(
        "compared {compared} overrides: {matched} identical, {} documented divergence(s)",
        observed.len()
    );

    // Guard against the comparison doing nothing. Zero comparisons and zero
    // failures look alike, so an empty router_overrides()/session_overrides()
    // would pass. The count is 31 today. The bound stays loose so that a
    // legitimate removal does not require an edit here.
    assert!(
        compared >= 25,
        "expected at least 25 overrides to compare, got {compared} -- \
         router_overrides()/session_overrides() returned far less than they should"
    );

    assert!(
        problems.is_empty(),
        "hiroz's zenoh configuration has drifted from rmw_zenoh_cpp's.\n\
         Reference: {}\n\n{}",
        reference.source,
        problems.join("\n\n"),
    );
}

/// Where `rmw_zenoh_cpp` is installed, the alignment test must read its
/// configuration and not the vendored copy.
///
/// Checking the vendored copy against upstream is the only reason to run the
/// alignment test on a ROS leg. Three things can break that path: a wrong
/// path, a renamed upstream directory, and a packaging change. Each one makes
/// the alignment test fall back to the vendored copy and still pass. This test
/// turns that silent fallback into a red result.
#[test]
fn an_installed_rmw_zenoh_cpp_is_preferred_over_the_vendored_copy() {
    if !rmw_zenoh_cpp_is_installed() {
        println!(
            "rmw_zenoh_cpp is not installed (AMENT_PREFIX_PATH={:?}): the alignment test \
             reads the vendored copy, which is correct here",
            std::env::var("AMENT_PREFIX_PATH").unwrap_or_default(),
        );
        return;
    }

    let dir = installed_config_dir().unwrap_or_else(|| {
        panic!(
            "rmw_zenoh_cpp is installed under one of {:?}, but no `share/rmw_zenoh_cpp/config` \
             directory was found in any of them. Upstream may have moved or renamed it. Until \
             this is fixed the alignment test silently checks the vendored copy against \
             itself.",
            ament_prefixes(),
        )
    });

    assert!(
        reference().installed,
        "the installed configuration exists at {} but the alignment test did not select it",
        dir.display(),
    );
}
