//! A raw subscriber stays out of rmw_zenoh's CPU buffer group unless the caller opts in.
//!
//! `BufferGroup::Auto` deliberately behaves as `Off` for a raw subscriber, so only
//! `create_raw_subscriber_with_group(.., BufferGroup::Cpu)` joins. These tests play the publisher
//! with a plain Zenoh session, so they need no ROS installation. They check what a publisher
//! reads: the liveliness token, and which keys reach the callback. Every assertion that filters
//! tokens filters on the text `backends:`, so it holds whether a default raw subscriber declares
//! no token or one plain token.
//!
//! Raw subscribers need hiroz's `ffi` feature, which the other suites do not, so the file has
//! its own feature, `raw-sub-tests`.

#![cfg(target_os = "linux")]
#![cfg(feature = "raw-sub-tests")]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::*;
use hiroz::{
    Builder,
    context::ZContextBuilder,
    qos::{QosDurability, QosProfile},
};
use hiroz_msgs::std_msgs::UInt8MultiArray;
use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter, RmwZenohFormatter};

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);

const TYPE_NAME: &str = "std_msgs::msg::dds_::UInt8MultiArray_";
const TYPE_HASH: &str = "RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";

/// The CDR bytes of a `UInt8MultiArray` with the data `[1, 2, 3]`.
const PAYLOAD: [u8; 19] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3];

fn raw_session(endpoint: &str) -> zenoh::Session {
    use zenoh::Wait;
    let mut config = zenoh::Config::default();
    config.insert_json5("mode", "\"client\"").unwrap();
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .unwrap();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    zenoh::open(config).wait().expect("raw session")
}

/// Every liveliness token currently declared. An error reply fails the test.
fn tokens(session: &zenoh::Session) -> Vec<String> {
    use zenoh::Wait;
    let replies = session
        .liveliness()
        .get("@ros2_lv/**")
        .timeout(Duration::from_secs(2))
        .wait()
        .expect("liveliness get");
    let mut keys = Vec::new();
    while let Ok(reply) = replies.recv() {
        match reply.result() {
            Ok(sample) => keys.push(sample.key_expr().to_string()),
            Err(e) => panic!("liveliness query returned an error: {e:?}"),
        }
    }
    keys
}

/// The tokens of `topic` that are visible now.
fn tokens_of(session: &zenoh::Session, topic: &str) -> Vec<String> {
    let needle = format!("/%{topic}/");
    tokens(session)
        .into_iter()
        .filter(|k| k.contains(&needle))
        .collect()
}

/// The one token for `topic`, once it is visible. A token that never appears fails the test.
fn token_for(session: &zenoh::Session, topic: &str) -> String {
    let deadline = Instant::now() + RECV;
    loop {
        let hits = tokens_of(session, topic);
        match hits.len() {
            1 => return hits[0].clone(),
            0 if Instant::now() < deadline => std::thread::sleep(POLL),
            n => panic!("{n} tokens for {topic}: {hits:?}"),
        }
    }
}

/// The tokens of `topic`, once at least one is visible, or none after the deadline. Unlike
/// `token_for` it does not panic on an empty set, so a test can name the missing token itself.
fn wait_tokens(session: &zenoh::Session, topic: &str) -> Vec<String> {
    let deadline = Instant::now() + RECV;
    loop {
        let hits = tokens_of(session, topic);
        if !hits.is_empty() || Instant::now() >= deadline {
            return hits;
        }
        std::thread::sleep(POLL);
    }
}

/// The key a publisher writes `topic` to. The hash segment is `TYPE_HASH` on Jazzy and later and
/// `TypeHashNotSupported` on Humble, as the distro this binary is built for writes it.
fn base_key(topic: &str) -> String {
    let hash = hiroz::TypeHash::from_rihs_string(TYPE_HASH)
        .expect("a valid RIHS string")
        .to_rihs_string();
    format!("0/{topic}/{TYPE_NAME}/{hash}")
}

fn group_key(topic: &str) -> String {
    format!("{}/_buf_cpu", base_key(topic))
}

/// Put `payload` on `key` until `done` is true or `limit` passes.
fn put_until(
    session: &zenoh::Session,
    key: &str,
    payload: &[u8],
    limit: Duration,
    mut done: impl FnMut() -> bool,
) -> bool {
    use zenoh::Wait;
    let end = Instant::now() + limit;
    while Instant::now() < end {
        session.put(key, payload.to_vec()).wait().expect("put");
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn raw_put(session: &zenoh::Session, key: &str) {
    use zenoh::Wait;
    session.put(key, PAYLOAD.to_vec()).wait().expect("put");
}

/// A raw callback that counts what it receives.
fn counter() -> (Arc<AtomicUsize>, impl Fn(&[u8]) + Send + Sync + 'static) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    (count, move |_: &[u8]| {
        c.fetch_add(1, Ordering::Relaxed);
    })
}

/// A raw subscriber built without the opt-in announces no `backends:` field, and it
/// is alive. The typed twin shows the token query works.
#[test]
fn raw_subscriber_without_opt_in_announces_no_backends() {
    const NAME: &str = "raw_subscriber_without_opt_in_announces_no_backends";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_default").build().expect("node");

    let _twin = node
        .create_sub::<UInt8MultiArray>("/raw_default_twin")
        .with_buffer_group(hiroz::BufferGroup::Auto)
        .build()
        .expect("typed twin");
    let (count, callback) = counter();
    let _sub = node
        .create_raw_subscriber("/raw_default_topic", TYPE_NAME, TYPE_HASH, callback)
        .expect("raw subscriber");

    // Control 1: the typed twin yields exactly one token, and it joins.
    let twin = token_for(&raw, "raw_default_twin");
    assert!(
        twin.ends_with("/backends:cpu:"),
        "{NAME}: the typed twin did not join, so the token query proves nothing: {twin}"
    );
    // Control 2: the raw subscriber is alive.
    assert!(
        put_until(&raw, &base_key("raw_default_topic"), &PAYLOAD, RECV, || {
            count.load(Ordering::Relaxed) > 0
        }),
        "{NAME}: a put on the base key did not reach the raw callback"
    );

    let raw_tokens = tokens_of(&raw, "raw_default_topic");
    let backends: Vec<&String> = raw_tokens
        .iter()
        .filter(|t| t.contains("backends:"))
        .collect();
    assert!(
        backends.is_empty(),
        "{NAME}: a default raw subscriber announced backends: {backends:?}"
    );
    assert!(
        raw_tokens.len() <= 1,
        "{NAME}: a default raw subscriber declared more than one token: {raw_tokens:?}"
    );
}

/// A raw subscriber built with the opt-in joins, and one callback serves the base key
/// and the group key, once per sample.
#[test]
fn raw_subscriber_with_the_opt_in_joins_and_one_callback_serves_both_keys() {
    const NAME: &str = "raw_subscriber_with_the_opt_in_joins_and_one_callback_serves_both_keys";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_optin").build().expect("node");

    let _twin = node
        .create_sub::<UInt8MultiArray>("/raw_optin_twin")
        .with_buffer_group(hiroz::BufferGroup::Auto)
        .build()
        .expect("typed twin");
    let (count, callback) = counter();
    let _sub = node
        .create_raw_subscriber_with_group(
            "/raw_optin_topic",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("raw subscriber");

    // Control: the typed twin joins in the same test.
    let twin = token_for(&raw, "raw_optin_twin");
    assert!(
        twin.ends_with("/backends:cpu:"),
        "{NAME}: the typed twin did not join: {twin}"
    );
    let tokens = wait_tokens(&raw, "raw_optin_topic");
    assert!(
        tokens.len() == 1 && tokens[0].ends_with("/backends:cpu:"),
        "{NAME}: the opted-in raw subscriber did not announce backends:cpu: {tokens:?}"
    );

    // One sample on each key reaches the one callback, once.
    for key in [group_key("raw_optin_topic"), base_key("raw_optin_topic")] {
        let before = count.load(Ordering::Relaxed);
        assert!(
            put_until(&raw, &key, &PAYLOAD, RECV, || count.load(Ordering::Relaxed)
                > before),
            "{NAME}: nothing reached the callback from {key}"
        );
    }
    // No duplicate delivery: a single put on each key adds exactly one call.
    std::thread::sleep(QUIET);
    for key in [group_key("raw_optin_topic"), base_key("raw_optin_topic")] {
        let before = count.load(Ordering::Relaxed);
        raw_put(&raw, &key);
        std::thread::sleep(QUIET);
        let got = count.load(Ordering::Relaxed) - before;
        assert!(
            got == 1,
            "{NAME}: one put on {key} produced {got} callback calls"
        );
    }
}

/// With a default and an opted-in raw subscriber side by side, only the opted-in one
/// announces `backends:cpu:`.
#[test]
fn only_the_opted_in_raw_subscriber_announces_backends() {
    const NAME: &str = "only_the_opted_in_raw_subscriber_announces_backends";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_side").build().expect("node");

    let (_c1, default_cb) = counter();
    let (_c2, optin_cb) = counter();
    let _default = node
        .create_raw_subscriber("/raw_side_default", TYPE_NAME, TYPE_HASH, default_cb)
        .expect("default raw");
    let _optin = node
        .create_raw_subscriber_with_group(
            "/raw_side_optin",
            TYPE_NAME,
            TYPE_HASH,
            optin_cb,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in raw");

    let optin_tokens = wait_tokens(&raw, "raw_side_optin");
    assert!(
        optin_tokens.len() == 1 && optin_tokens[0].ends_with("/backends:cpu:"),
        "{NAME}: the opted-in raw subscriber did not announce backends:cpu: {optin_tokens:?}"
    );
    let default_tokens = tokens_of(&raw, "raw_side_default");
    let announcing: Vec<&String> = default_tokens
        .iter()
        .filter(|t| t.contains("backends:"))
        .collect();
    assert!(
        announcing.is_empty(),
        "{NAME}: the default raw subscriber announced backends: {announcing:?}"
    );
}

/// The feedback subscriber of a raw action client stays out of the group. An
/// opted-in raw subscriber on the same node is the control.
#[test]
fn raw_action_client_feedback_subscriber_stays_out() {
    const NAME: &str = "raw_action_client_feedback_subscriber_stays_out";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_action").build().expect("node");
    let _client = node
        .create_raw_action_client(
            "/raw_fib",
            "example_interfaces/action/Fibonacci",
            "",
            "",
            "",
            "",
            "",
            "",
        )
        .expect("raw action client");
    let (_c, callback) = counter();
    let _control = node
        .create_raw_subscriber_with_group(
            "/raw_action_control",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in raw subscriber");

    let control = wait_tokens(&raw, "raw_action_control");
    assert!(
        control.len() == 1 && control[0].ends_with("/backends:cpu:"),
        "{NAME}: the opted-in control did not join, so the filter below counts for nothing: {control:?}"
    );
    let feedback: Vec<String> = tokens(&raw)
        .into_iter()
        .filter(|t| t.contains("feedback") && t.contains("backends:"))
        .collect();
    assert!(
        feedback.is_empty(),
        "{NAME}: the feedback subscriber announced backends: {feedback:?}"
    );
}

/// Dropping an opted-in raw subscriber stops its callback on the group key. The
/// callback fires before the drop.
#[test]
fn dropping_an_opted_in_raw_subscriber_stops_group_key_callbacks() {
    const NAME: &str = "dropping_an_opted_in_raw_subscriber_stops_group_key_callbacks";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_drop").build().expect("node");
    let (count, callback) = counter();
    let sub = node
        .create_raw_subscriber_with_group(
            "/raw_drop_topic",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in raw subscriber");

    // Positive control: the callback fires on the group key before the drop.
    assert!(
        put_until(&raw, &group_key("raw_drop_topic"), &PAYLOAD, RECV, || {
            count.load(Ordering::Relaxed) > 0
        }),
        "{NAME}: the opted-in callback did not fire on the group key before the drop"
    );

    drop(sub);
    // Wait for the token undeclaration to become visible to the router query.
    let deadline = Instant::now() + RECV;
    while !tokens_of(&raw, "raw_drop_topic").is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
    let left = tokens_of(&raw, "raw_drop_topic");
    assert!(
        left.is_empty(),
        "{NAME}: the token of the dropped raw subscriber is still declared after {RECV:?}: {left:?}"
    );
    std::thread::sleep(POLL);
    let before = count.load(Ordering::Relaxed);
    for _ in 0..5 {
        raw_put(&raw, &group_key("raw_drop_topic"));
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(QUIET);
    let after = count.load(Ordering::Relaxed);
    assert!(
        after == before,
        "{NAME}: the callback ran {} more time(s) on the group key after the drop",
        after - before
    );
}

/// What a raw subscriber that stays out must show, next to an opted-in `Cpu` raw subscriber that
/// joins. The control is the positive: it proves the token query and the group key work in this
/// test, so an empty answer for the subject is not a dead setup.
///
/// - the control announces `backends:cpu:`;
/// - the subject is alive: a put on its base key reaches its callback;
/// - the subject announces no `backends:` field;
/// - a put on the group key of the subject does not reach its callback.
fn assert_stays_out(
    name: &str,
    raw: &zenoh::Session,
    control_topic: &str,
    subject_topic: &str,
    subject_count: &AtomicUsize,
) {
    let control = wait_tokens(raw, control_topic);
    assert!(
        control.len() == 1 && control[0].ends_with("/backends:cpu:"),
        "{name}: the opted-in control did not join, so the checks below count for nothing: {control:?}"
    );
    assert!(
        put_until(raw, &base_key(subject_topic), &PAYLOAD, RECV, || {
            subject_count.load(Ordering::Relaxed) > 0
        }),
        "{name}: a put on the base key did not reach the subject, so it is not alive"
    );
    // Puts on the base key may still be in flight: let them land before counting.
    std::thread::sleep(QUIET);
    let subject_tokens = tokens_of(raw, subject_topic);
    let announcing: Vec<&String> = subject_tokens
        .iter()
        .filter(|t| t.contains("backends:"))
        .collect();
    assert!(
        announcing.is_empty(),
        "{name}: the subject announced backends: {announcing:?}"
    );
    let before = subject_count.load(Ordering::Relaxed);
    for _ in 0..5 {
        raw_put(raw, &group_key(subject_topic));
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(QUIET);
    let got = subject_count.load(Ordering::Relaxed) - before;
    assert!(
        got == 0,
        "{name}: the subject received {got} sample(s) on its group key, so it joined the group"
    );
}

/// `Auto` has no message definition to inspect for a raw subscriber, so it stays out.
#[test]
fn raw_subscriber_with_group_auto_stays_out() {
    const NAME: &str = "raw_subscriber_with_group_auto_stays_out";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_auto").build().expect("node");
    let (_c, control_cb) = counter();
    let _control = node
        .create_raw_subscriber_with_group(
            "/raw_auto_control",
            TYPE_NAME,
            TYPE_HASH,
            control_cb,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in control");
    let (count, callback) = counter();
    let _subject = node
        .create_raw_subscriber_with_group(
            "/raw_auto_subject",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Auto,
        )
        .expect("raw subscriber with Auto");
    assert_stays_out(NAME, &raw, "raw_auto_control", "raw_auto_subject", &count);
}

/// `Off` stays out, the same as the default.
#[test]
fn raw_subscriber_with_group_off_stays_out() {
    const NAME: &str = "raw_subscriber_with_group_off_stays_out";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_off").build().expect("node");
    let (_c, control_cb) = counter();
    let _control = node
        .create_raw_subscriber_with_group(
            "/raw_off_control",
            TYPE_NAME,
            TYPE_HASH,
            control_cb,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in control");
    let (count, callback) = counter();
    let _subject = node
        .create_raw_subscriber_with_group(
            "/raw_off_subject",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Off,
        )
        .expect("raw subscriber with Off");
    assert_stays_out(NAME, &raw, "raw_off_control", "raw_off_subject", &count);
}

/// `rmw_zenoh` serves transient-local history on the base key only, so a transient-local raw
/// subscriber stays out even with `Cpu`.
#[test]
fn raw_transient_local_subscriber_stays_out_with_cpu() {
    const NAME: &str = "raw_transient_local_subscriber_stays_out_with_cpu";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_tl").build().expect("node");
    let (_c, control_cb) = counter();
    let _control = node
        .create_raw_subscriber_with_group(
            "/raw_tl_control",
            TYPE_NAME,
            TYPE_HASH,
            control_cb,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in control");
    let (count, callback) = counter();
    let _subject = node
        .create_raw_subscriber_with_group(
            "/raw_tl_subject",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            Some(QosProfile {
                durability: QosDurability::TransientLocal,
                ..Default::default()
            }),
            hiroz::BufferGroup::Cpu,
        )
        .expect("transient-local raw subscriber");
    assert_stays_out(NAME, &raw, "raw_tl_control", "raw_tl_subject", &count);
}

/// Only the `rmw_zenoh` key format has the group, so a raw subscriber on a custom format stays
/// out even with `Cpu`. The control is on the `rmw_zenoh` format.
#[test]
fn raw_custom_key_format_subscriber_stays_out_with_cpu() {
    const NAME: &str = "raw_custom_key_format_subscriber_stays_out_with_cpu";
    let router = TestRouter::new();
    let raw = raw_session(router.endpoint());
    let ctx = create_hiroz_context_with_router(&router).expect("ctx");
    let node = ctx.create_node("raw_fmt").build().expect("node");
    let (_c, control_cb) = counter();
    let _control = node
        .create_raw_subscriber_with_group(
            "/raw_fmt_control",
            TYPE_NAME,
            TYPE_HASH,
            control_cb,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("opted-in control");
    let custom_ctx = ZContextBuilder::default()
        .keyexpr_format(KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<
            RmwZenohFormatter,
        >::new())))
        .disable_multicast_scouting()
        .with_connect_endpoints([router.endpoint()])
        .with_mode("client")
        .build()
        .expect("custom ctx");
    let custom_node = custom_ctx.create_node("raw_fmt2").build().expect("node");
    let (count, callback) = counter();
    let _subject = custom_node
        .create_raw_subscriber_with_group(
            "/raw_fmt_subject",
            TYPE_NAME,
            TYPE_HASH,
            callback,
            None,
            hiroz::BufferGroup::Cpu,
        )
        .expect("raw subscriber on a custom format");
    assert_stays_out(NAME, &raw, "raw_fmt_control", "raw_fmt_subject", &count);
}
