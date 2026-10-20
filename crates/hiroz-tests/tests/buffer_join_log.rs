//! The join log decides once per group key and remembers a bounded number of keys.
//!
//! The decision state is a value of its own, so these tests drive it directly with a small cap.
//! The process-wide log is one global set, so a test that reads its output needs a process of its
//! own, and this file is that process.

#![cfg(feature = "ros-msgs")]

use hiroz::buffer_group::{JoinLog, JoinLogLevel, LOG_KEYS};

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
    assert_eq!(LOG_KEYS, 1024, "join_log_cap_is_the_documented_bound");
}
