//! The join log decides once per group key and remembers a bounded number of keys.
//!
//! The decision state is a value of its own, so these tests drive it directly with a small cap.
//! The process-wide log is one global set, so a test that reads its output needs a process of its
//! own, and this file is that process.

#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use common::*;
use hiroz::{
    BufferGroup, Builder,
    buffer_group::{JoinLog, JoinLogLevel, Plan},
};
use hiroz_msgs::std_msgs::UInt8MultiArray;
use hiroz_protocol::KeyExprFormat;
use tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt};
use zenoh::Wait;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);
impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn session(endpoint: &str) -> zenoh::Session {
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    zenoh::open(config).wait().expect("session")
}

/// A key logs at `info` level once, a repeat logs nothing, and a new key past the cap logs at
/// `debug` level and is not stored. A key stored before the cap stays known after it.
#[test]
fn join_log_decides_each_join_once_and_stays_bounded() {
    const NAME: &str = "join_log_decides_each_join_once_and_stays_bounded";
    const CAP: usize = 3;
    let mut log = JoinLog::new(CAP);
    assert!(log.is_empty(), "{NAME}: a new log holds a key");

    let steps = [
        ("a", JoinLogLevel::Info, 1),
        ("a", JoinLogLevel::Skip, 1),
        ("b", JoinLogLevel::Info, 2),
        ("c", JoinLogLevel::Info, 3),
        ("c", JoinLogLevel::Skip, 3),
        ("d", JoinLogLevel::Debug, 3),
        ("d", JoinLogLevel::Debug, 3),
        ("e", JoinLogLevel::Debug, 3),
        ("a", JoinLogLevel::Skip, 3),
        ("b", JoinLogLevel::Skip, 3),
    ];
    for (key, want, size) in steps {
        let got = log.record(key);
        assert_eq!(got, want, "{NAME}: key {key}");
        assert_eq!(log.len(), size, "{NAME}: size after key {key}");
        assert!(
            log.len() <= CAP,
            "{NAME}: size {} is over the cap",
            log.len()
        );
    }
}

/// A flood of distinct keys never grows the set past the cap, and exactly `cap` of them log at
/// `info` level.
#[test]
fn join_log_stores_no_more_than_its_cap_under_many_keys() {
    const NAME: &str = "join_log_stores_no_more_than_its_cap_under_many_keys";
    const CAP: usize = 8;
    let mut log = JoinLog::new(CAP);
    let mut info = 0;
    let mut debug = 0;
    for i in 0..CAP * 4 {
        match log.record(&format!("key_{i}")) {
            JoinLogLevel::Info => info += 1,
            JoinLogLevel::Debug => debug += 1,
            JoinLogLevel::Skip => panic!("{NAME}: key_{i} is new and was skipped"),
        }
        assert!(log.len() <= CAP, "{NAME}: size {} after key_{i}", log.len());
    }
    assert_eq!(info, CAP, "{NAME}: keys logged at info level");
    assert_eq!(debug, CAP * 3, "{NAME}: keys logged at debug level");
}

/// The process-wide log holds the number of keys the user guide states.
#[test]
fn join_log_cap_is_the_documented_bound() {
    assert_eq!(
        JoinLog::process_cap(),
        1024,
        "join_log_cap_is_the_documented_bound"
    );
}

/// A token declaration that fails logs no join, and the next successful join of the same key
/// still takes the one `info` line. The failing session is closed, so the declaration is the only
/// step of the join that fails.
#[test]
fn a_failed_token_declaration_logs_no_join() {
    const NAME: &str = "a_failed_token_declaration_logs_no_join";
    let out = Captured::default();
    let _guard = tracing_subscriber::fmt()
        .with_writer({
            let out = out.clone();
            move || out.clone()
        })
        .with_ansi(false)
        .with_max_level(LevelFilter::INFO)
        .finish()
        .set_default();
    let lines = || {
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .filter(|l| l.contains("CPU buffer group"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("wiring_node").build().expect("node");
    // Off declares no group, so building it logs nothing. Its entity names the topic to join.
    let probe = node
        .create_sub::<UInt8MultiArray>("/wiring_probe")
        .with_buffer_group(BufferGroup::Off)
        .build()
        .expect("probe subscriber");
    let live = session(router.endpoint());
    let dead = session(router.endpoint());
    dead.close().wait().expect("close");
    let plan = |s: &zenoh::Session| {
        Plan::new(&KeyExprFormat::RmwZenoh, &probe.entity, &s.zid(), true).expect("plan")
    };

    assert!(
        plan(&dead).declare(&dead).is_err(),
        "{NAME}: control: a token declaration on a closed session succeeded"
    );
    assert!(
        lines().is_empty(),
        "{NAME}: a failed declaration logged a join: {:?}",
        lines()
    );
    let _token = plan(&live)
        .declare(&live)
        .expect("declare on the open session");
    let after = lines();
    assert_eq!(
        after.len(),
        1,
        "{NAME}: want one line for the successful join: {after:?}"
    );
    assert!(
        after[0].contains(" INFO ")
            && after[0].contains("0/wiring_probe/")
            && after[0].contains("/**, which includes"),
        "{NAME}: the line is not an info line for the key: {after:?}"
    );
}
