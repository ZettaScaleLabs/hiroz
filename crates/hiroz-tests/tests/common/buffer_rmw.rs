//! Helpers for the tests that read from a released, buffer-aware `rmw_zenoh_cpp`.

#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use zenoh::Wait;

/// The suffix of the key a buffer-aware publisher writes buffer messages to.
pub const GROUP_SUFFIX: &str = "_buf_cpu";

/// Setting this variable (to anything non-empty) makes a missing buffer-aware rmw fail the
/// test instead of skipping it.
pub const REQUIRE_ENV: &str = "HIROZ_REQUIRE_BUFFER_RMW";

/// True when the `rmw_zenoh_cpp` the test will load has buffer support, which shows as the
/// group suffix in its library. Otherwise the test prints `Skipping <name>: <reason>` and the
/// caller returns, or the test fails when `HIROZ_REQUIRE_BUFFER_RMW` is set.
pub fn buffer_aware_rmw_or_skip(name: &str) -> bool {
    let dirs = |var: &str, sub: &str| -> Vec<PathBuf> {
        std::env::var_os(var)
            .map(|v| std::env::split_paths(&v).map(|p| p.join(sub)).collect())
            .unwrap_or_default()
    };
    let mut search = dirs("LD_LIBRARY_PATH", "");
    search.extend(dirs("AMENT_PREFIX_PATH", "lib"));
    let reason = match search
        .iter()
        .map(|d| d.join("librmw_zenoh_cpp.so"))
        .find(|p| p.is_file())
    {
        None => "librmw_zenoh_cpp.so not found".to_owned(),
        Some(lib) => match std::fs::read(&lib) {
            Ok(bytes)
                if bytes
                    .windows(GROUP_SUFFIX.len())
                    .any(|w| w == GROUP_SUFFIX.as_bytes()) =>
            {
                return true;
            }
            Ok(_) => format!("{} has no buffer support", lib.display()),
            Err(e) => format!("cannot read {}: {e}", lib.display()),
        },
    };
    if std::env::var_os(REQUIRE_ENV).is_some_and(|v| !v.is_empty()) {
        panic!("{name}: no buffer-aware rmw ({reason}), and {REQUIRE_ENV} is set");
    }
    println!("Skipping {name}: no buffer-aware rmw ({reason})");
    false
}

/// A plain Zenoh client session on `endpoint`. It is no ROS entity, so no publisher matches it
/// as a ROS subscriber.
pub fn client_session(endpoint: &str) -> zenoh::Session {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    zenoh::open(config).wait().expect("client session")
}

/// Counts the samples on every key under one topic key, read by a plain session on `<base>/**`.
/// This is an observer and a control probe, not the code under test: a joined hiroz subscriber never
/// subscribes to `<base>/**`. A key is stored relative to the base key: `""` is the base key and
/// `"_buf_cpu"` the group key.
pub struct KeyCounts {
    counts: Arc<Mutex<BTreeMap<String, usize>>>,
    _sub: zenoh::pubsub::Subscriber<()>,
    _session: zenoh::Session,
}

impl KeyCounts {
    pub fn new(endpoint: &str, base_key: &str) -> Self {
        let session = client_session(endpoint);
        let counts = Arc::new(Mutex::new(BTreeMap::new()));
        let (seen, base) = (counts.clone(), base_key.to_owned());
        let sub = session
            .declare_subscriber(format!("{base_key}/**"))
            .callback(move |sample| {
                let key = sample.key_expr().as_str();
                let rest = key.strip_prefix(base.as_str()).unwrap_or(key);
                *seen
                    .lock()
                    .unwrap()
                    .entry(rest.trim_start_matches('/').to_owned())
                    .or_default() += 1;
            })
            .wait()
            .expect("key counter");
        Self {
            counts,
            _sub: sub,
            _session: session,
        }
    }

    pub fn get(&self, relative_key: &str) -> usize {
        self.counts
            .lock()
            .unwrap()
            .get(relative_key)
            .copied()
            .unwrap_or(0)
    }

    pub fn group(&self) -> usize {
        self.get(GROUP_SUFFIX)
    }

    pub fn base(&self) -> usize {
        self.get("")
    }

    /// Every counted key, relative to the base key.
    pub fn all(&self) -> Vec<(String, usize)> {
        self.counts
            .lock()
            .unwrap()
            .iter()
            .map(|(k, n)| (k.clone(), *n))
            .collect()
    }
}

/// Counts received messages by sequence number. Message `seq` carries it in the first 4 bytes
/// (little endian) of a payload of `size` bytes. A payload of another length counts for nothing.
pub struct SeqSet {
    size: usize,
    seen: std::collections::BTreeSet<u32>,
    pub duplicates: usize,
}

impl SeqSet {
    pub fn new(size: usize) -> Self {
        Self {
            size,
            seen: Default::default(),
            duplicates: 0,
        }
    }

    pub fn record(&mut self, data: &[u8]) {
        if data.len() != self.size {
            return;
        }
        if !self
            .seen
            .insert(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
        {
            self.duplicates += 1;
        }
    }

    /// How many of the sequence numbers `0..sent` arrived.
    pub fn received_of(&self, sent: u32) -> usize {
        self.seen.range(0..sent).count()
    }
}
