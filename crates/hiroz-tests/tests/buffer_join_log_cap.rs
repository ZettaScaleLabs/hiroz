//! The join log remembers a bounded number of group keys.
//!
//! The set of logged keys is per process and capped, so this test needs a process of its own. A
//! test that shares its process with other joining tests fills the set and changes what they log.

#![cfg(target_os = "linux")]
#![cfg(feature = "ros-msgs")]

mod common;

use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use common::*;
use hiroz::{BufferGroup, Builder};
use hiroz_msgs::std_msgs::UInt8MultiArray;
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

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

/// The process remembers a bounded number of group keys for the one-line log. A joined subscriber
/// on a key past that bound logs at `debug` and not at `info`, so a process that joins many
/// topics keeps no unbounded state.
#[test]
fn join_log_remembers_a_bounded_number_of_keys() {
    const NAME: &str = "join_log_remembers_a_bounded_number_of_keys";
    const CAP: usize = 1024;
    const TOPICS: usize = CAP + 16;
    let out = Captured::default();
    // Only the `hiroz` targets are captured at debug level: debug spans of the transport would
    // cross threads that carry no subscriber.
    let _guard = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer({
                    let out = out.clone();
                    move || out.clone()
                })
                .with_ansi(false)
                .with_filter(Targets::new().with_target("hiroz", LevelFilter::DEBUG)),
        )
        .set_default();
    let router = TestRouter::new();
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("cap_node").build().expect("node");
    let subs: Vec<_> = (0..TOPICS)
        .map(|i| {
            node.create_sub::<UInt8MultiArray>(&format!("/cap_{i}"))
                .with_buffer_group(BufferGroup::Auto)
                .build()
                .unwrap()
        })
        .collect();

    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("CPU buffer group"))
        .collect();
    let info = lines.iter().filter(|l| l.contains(" INFO ")).count();
    let debug = lines.iter().filter(|l| l.contains(" DEBUG ")).count();
    // Control: every subscriber joined and each join logged once at some level, so a short count
    // is a missing log and not a missing join.
    assert_eq!(subs.len(), TOPICS, "{NAME}: control: subscribers built");
    assert_eq!(
        info + debug,
        TOPICS,
        "{NAME}: control: want one line per joined key, got {info} info and {debug} debug"
    );
    assert!(
        info > 0,
        "{NAME}: control: no line at info level, the log never fired"
    );
    assert!(
        info <= CAP,
        "{NAME}: {info} keys logged at info level, the bound is {CAP}"
    );
}
