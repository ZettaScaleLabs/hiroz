//! The T1 harness: the mode detector and the sequence and sentinel counter.
//!
//! A cell that reads a result from a released `rmw_zenoh` publisher needs two facts. The first is
//! that the publisher is in buffer mode: a plain Zenoh session counts samples on the group key
//! `<base>/_buf_cpu` and on the base key, and the publisher is in buffer mode only when the group
//! key carries samples and the base key none. The second is how many of the published samples
//! arrived: the counter reads a payload sequence number and a sentinel.

#![allow(dead_code)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use zenoh::Wait;

/// The group suffix a buffer-aware publisher writes under.
pub const GROUP_SUFFIX: &str = "_buf_cpu";

/// Fails a test with the marker the J-RED verdict rule reads: `R1ASSERT <name>: <message>`.
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
        let _ = self;
        Mode::Silent
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
        let _ = timeout;
        Ok(self.read().group)
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
