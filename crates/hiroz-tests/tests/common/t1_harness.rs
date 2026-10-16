//! The T1 harness: the mode detector and the sequence and sentinel counter.
//!
//! A cell that reads a result from a released `rmw_zenoh` publisher needs two facts. The first is
//! that the publisher is in buffer mode: a plain Zenoh session counts samples on the group key
//! `<base>/_buf_cpu` and on the base key, and the publisher is in buffer mode only when the group
//! key carries samples and the base key none. The second is how many of the published samples
//! arrived: the counter reads a payload sequence number and a sentinel.

#![allow(dead_code)]

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use zenoh::Wait;

/// The group suffix a buffer-aware publisher writes under.
pub const GROUP_SUFFIX: &str = "_buf_cpu";

/// Fails a test with the marker that names the failing assertion: `R1ASSERT <name>: <message>`.
pub fn r1_fail(name: &str, message: &str) -> ! {
    panic!("R1ASSERT {name}: {message}");
}

/// What the plain session read on the two keys of one topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeReading {
    pub group: usize,
    pub base: usize,
}

/// The mode of a publisher, read from the two counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The group key carries samples and the base key none.
    Buffer { group: usize },
    /// The base key carries samples: the publisher is not in buffer mode.
    NotBuffer { base: usize },
    /// Neither key carried a sample.
    Silent,
}

impl ModeReading {
    pub fn mode(&self) -> Mode {
        if self.base > 0 {
            Mode::NotBuffer { base: self.base }
        } else if self.group > 0 {
            Mode::Buffer { group: self.group }
        } else {
            Mode::Silent
        }
    }
}

/// The detector cannot vouch for the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorError {
    NotInBufferMode { base: usize, group: usize },
    NoSamples,
}

impl std::fmt::Display for DetectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInBufferMode { base, group } => write!(
                f,
                "the publisher is not in buffer mode (base key {base}, group key {group}): the cell proves nothing"
            ),
            Self::NoSamples => write!(f, "no sample on either key: the cell proves nothing"),
        }
    }
}

/// A plain Zenoh client session with one counter on the group key and one on the base key.
pub struct ModeDetector {
    group: Arc<AtomicUsize>,
    base: Arc<AtomicUsize>,
    _group_sub: zenoh::pubsub::Subscriber<()>,
    _base_sub: zenoh::pubsub::Subscriber<()>,
    _session: zenoh::Session,
}

impl ModeDetector {
    /// Opens a client session on `endpoint` and counts samples on `base_key` and its group key.
    pub fn new(endpoint: &str, base_key: &str) -> Self {
        let mut config = zenoh::Config::default();
        config.insert_json5("mode", "\"client\"").unwrap();
        config
            .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
            .unwrap();
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        let session = zenoh::open(config).wait().expect("detector session");
        let (group_sub, group) = counter(&session, &format!("{base_key}/{GROUP_SUFFIX}"));
        let (base_sub, base) = counter(&session, base_key);
        Self {
            group,
            base,
            _group_sub: group_sub,
            _base_sub: base_sub,
            _session: session,
        }
    }

    pub fn read(&self) -> ModeReading {
        ModeReading {
            group: self.group.load(Ordering::SeqCst),
            base: self.base.load(Ordering::SeqCst),
        }
    }

    /// Prints `T1MODE <cell> GROUP=<g> BASE=<b>`, the line the cell runner turns into a result row.
    pub fn report_mode(&self, cell: &str) {
        let reading = self.read();
        println!("T1MODE {cell} GROUP={} BASE={}", reading.group, reading.base);
    }

    /// Prints the result line of a cell that counts received messages: `T1RED <cell> RECEIVED=<k>
    /// OF=<n> DETECTOR=group:<g>,base:<b>`. The cell runner appends `BUILD=<rc>`.
    pub fn report_cell(&self, cell: &str, received: usize, of: usize) {
        let reading = self.read();
        println!(
            "T1RED {cell} RECEIVED={received} OF={of} DETECTOR=group:{},base:{}",
            reading.group, reading.base
        );
    }

    /// Waits up to `timeout` for a sample on either key, then judges the mode.
    pub fn require_buffer_mode(&self, timeout: Duration) -> Result<usize, DetectorError> {
        let end = Instant::now() + timeout;
        while Instant::now() < end && self.read().mode() == Mode::Silent {
            std::thread::sleep(Duration::from_millis(20));
        }
        // A publisher that writes both keys must show on the base key, so wait for the second
        // key after the first sample.
        std::thread::sleep(Duration::from_millis(500));
        let reading = self.read();
        match reading.mode() {
            Mode::Buffer { group } => Ok(group),
            Mode::NotBuffer { base } => Err(DetectorError::NotInBufferMode {
                base,
                group: reading.group,
            }),
            Mode::Silent => Err(DetectorError::NoSamples),
        }
    }
}

fn counter(session: &zenoh::Session, key: &str) -> (zenoh::pubsub::Subscriber<()>, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let sub = session
        .declare_subscriber(key.to_owned())
        .callback(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .wait()
        .expect("counter");
    (sub, count)
}

/// `r1_fail` unless `left == right`.
pub fn r1_assert_eq<T: PartialEq + std::fmt::Debug>(name: &str, left: T, right: T, what: &str) {
    if left != right {
        r1_fail(name, &format!("{what}: {left:?} is not {right:?}"));
    }
}

/// The last byte of every payload a cell publishes.
pub const SENTINEL: u8 = 0xA5;

/// The `data` bytes of message number `seq`: the sequence number as 4 little-endian bytes, filler,
/// and the sentinel as the last byte. `len` is at least 5.
pub fn seq_payload(seq: u32, len: usize) -> Vec<u8> {
    assert!(len >= 5, "a payload holds 4 sequence bytes and a sentinel");
    let mut data = vec![0u8; len];
    data[..4].copy_from_slice(&seq.to_le_bytes());
    data[len - 1] = SENTINEL;
    data
}

/// Counts received messages by sequence number. A payload whose last byte is not the sentinel is
/// corrupt and counts for nothing: a truncated or shifted buffer must not pass as received.
#[derive(Debug, Default)]
pub struct SeqCounter {
    seen: std::collections::BTreeSet<u32>,
    duplicates: usize,
    corrupt: usize,
}

impl SeqCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one received `data` field. Returns whether it was a new valid message.
    pub fn record(&mut self, data: &[u8]) -> bool {
        if data.len() < 5 || data[data.len() - 1] != SENTINEL {
            self.corrupt += 1;
            return false;
        }
        let seq = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if self.seen.insert(seq) {
            true
        } else {
            self.duplicates += 1;
            false
        }
    }

    /// How many of the sequence numbers `0..sent` arrived at least once.
    pub fn received_of(&self, sent: u32) -> usize {
        self.seen.range(0..sent).count()
    }

    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    pub fn corrupt(&self) -> usize {
        self.corrupt
    }
}

/// The marker string a buffer-aware `rmw_zenoh_cpp` carries: the group suffix of its key expressions.
pub const BUFFER_MARKER: &str = "_buf_cpu";

/// The most bytes of a library the guard reads while it looks for the marker.
const SCAN_LIMIT: u64 = 64 * 1024 * 1024;

/// Why the guard cannot vouch for the `rmw_zenoh_cpp` of this environment.
#[derive(Debug, PartialEq, Eq)]
pub enum RmwGuardError {
    /// No `librmw_zenoh_cpp.so` on `LD_LIBRARY_PATH`, in an `AMENT_PREFIX_PATH` prefix or in the
    /// prefix `ros2 pkg prefix rmw_zenoh_cpp` names.
    NotFound,
    /// The library has no buffer marker.
    NotBufferAware { path: PathBuf, version: Option<String> },
    /// The library could not be read to the end inside the limit.
    Unreadable { path: PathBuf, why: String },
}

impl std::fmt::Display for RmwGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "could not find librmw_zenoh_cpp.so"),
            Self::NotBufferAware { path, version } => write!(
                f,
                "the rmw_zenoh_cpp in this environment is not buffer-aware ({}, {}): the library has no `_buf_cpu` marker",
                path.display(),
                version.as_deref().map_or("version not found".to_owned(), |v| format!("version {v}")),
            ),
            Self::Unreadable { path, why } => write!(
                f,
                "could not scan librmw_zenoh_cpp.so ({}): {why}",
                path.display()
            ),
        }
    }
}

/// True when `reader` holds `needle` in its first `limit` bytes. The read is chunked, so memory
/// stays bounded. An error is returned when the limit ends the scan before the end of the input.
pub fn stream_contains(
    mut reader: impl std::io::Read,
    needle: &[u8],
    limit: u64,
) -> std::io::Result<bool> {
    const CHUNK: usize = 1 << 20;
    let keep = needle.len().saturating_sub(1);
    let mut buf = vec![0u8; CHUNK + keep];
    let (mut carried, mut total) = (0usize, 0u64);
    loop {
        let n = reader.read(&mut buf[carried..carried + CHUNK])?;
        if n == 0 {
            return Ok(false);
        }
        total += n as u64;
        let filled = carried + n;
        if buf[..filled].windows(needle.len()).any(|w| w == needle) {
            return Ok(true);
        }
        if total >= limit {
            return Err(std::io::Error::other(format!("no marker in the first {limit} bytes")));
        }
        carried = keep.min(filled);
        buf.copy_within(filled - carried..filled, 0);
    }
}

/// The directories where the publisher may find its `librmw_zenoh_cpp.so`, in loader order.
fn rmw_library_dirs() -> Vec<PathBuf> {
    let split = |var: &str| -> Vec<PathBuf> {
        std::env::var_os(var)
            .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default()
    };
    let mut dirs = split("LD_LIBRARY_PATH");
    dirs.extend(split("AMENT_PREFIX_PATH").into_iter().map(|p| p.join("lib")));
    if let Ok(out) = Command::new("ros2").args(["pkg", "prefix", "rmw_zenoh_cpp"]).output()
        && out.status.success()
    {
        let prefix = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !prefix.is_empty() {
            dirs.push(PathBuf::from(prefix).join("lib"));
        }
    }
    dirs
}

/// The first `librmw_zenoh_cpp.so` in `dirs`.
pub fn find_rmw_library(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join("librmw_zenoh_cpp.so")).find(|p| p.is_file())
}

/// The version in the `package.xml` of the package that owns `lib`, when the install has one.
pub fn rmw_package_version(lib: &Path) -> Option<String> {
    let prefix = lib.canonicalize().ok()?.parent()?.parent()?.to_owned();
    let xml = std::fs::read_to_string(prefix.join("share/rmw_zenoh_cpp/package.xml")).ok()?;
    let rest = xml.split("<version>").nth(1)?;
    Some(rest.split("</version>").next()?.trim().to_owned())
}

/// Judges the library at `lib`.
pub fn check_rmw_library(lib: &Path) -> Result<(), RmwGuardError> {
    let unreadable = |e: std::io::Error| RmwGuardError::Unreadable {
        path: lib.to_owned(),
        why: e.to_string(),
    };
    let file = std::fs::File::open(lib).map_err(unreadable)?;
    if stream_contains(file, BUFFER_MARKER.as_bytes(), SCAN_LIMIT).map_err(unreadable)? {
        Ok(())
    } else {
        Err(RmwGuardError::NotBufferAware {
            path: lib.to_owned(),
            version: rmw_package_version(lib),
        })
    }
}

/// Judges the `rmw_zenoh_cpp` the publisher will load.
fn judge_rmw() -> Result<(), RmwGuardError> {
    match find_rmw_library(&rmw_library_dirs()) {
        Some(lib) => check_rmw_library(&lib),
        None => Err(RmwGuardError::NotFound),
    }
}

/// The environment variable that turns a missing buffer mode from a logged skip into a failure.
/// A CI runner that must exercise the buffer cells sets it: a buffer cell must run there, never skip.
pub const REQUIRE_BUFFER_RMW_ENV: &str = "T1_REQUIRE_BUFFER_RMW";

/// The gate of a buffer cell: true when the `rmw_zenoh_cpp` the publisher will load is
/// buffer-aware, so the cell may run. When it is not, the call prints
/// `Skipping <name>: no buffer-aware rmw (<reason>)` and returns false, and the cell returns. The
/// line is the explicit record that the cell did not run: a CI check can count `Skipping` lines, and a
/// pass with such a line is UNKNOWN for that cell, not a pass. When `T1_REQUIRE_BUFFER_RMW` is set
/// the call fails the test instead, so a runner that must exercise the buffer cells cannot skip.
/// Do not call it from a compat cell: a test that uses a non-buffer message type is valid on any
/// rmw.
pub fn buffer_aware_rmw_or_skip(name: &str) -> bool {
    match judge_rmw() {
        Ok(()) => true,
        Err(e) if std::env::var_os(REQUIRE_BUFFER_RMW_ENV).is_some() => r1_fail(name, &e.to_string()),
        Err(e) => {
            println!("Skipping {name}: no buffer-aware rmw ({e})");
            false
        }
    }
}
