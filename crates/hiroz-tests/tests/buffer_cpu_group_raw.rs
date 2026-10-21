//! A raw subscriber joins rmw_zenoh's CPU buffer group by default, and `BufferGroup::Off` keeps it out.
//!
//! A raw subscriber has no message definition, so `BufferGroup::Auto` joins it as it joins a typed
//! subscriber without one. A transient-local subscriber and a custom key expression format never
//! join. These tests play the publisher with a plain Zenoh session, so they need no ROS
//! installation. They check what a publisher reads: the liveliness token, and which keys reach the
//! callback. Every assertion that filters tokens filters on the text `backends:`.
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
    BufferGroup, Builder,
    context::{ZContext, ZContextBuilder},
    ffi::subscriber::RawSubscriber,
    node::ZNode,
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

/// A router, a plain Zenoh session on it and a hiroz node on it. The plain session plays the
/// publisher and reads the tokens.
struct Fixture {
    node: ZNode,
    _ctx: ZContext,
    raw: zenoh::Session,
    router: TestRouter,
}

impl Fixture {
    fn new(node_name: &str) -> Self {
        Self::on(TestRouter::new(), node_name)
    }

    fn on(router: TestRouter, node_name: &str) -> Self {
        let raw = raw_session(router.endpoint());
        let ctx = create_hiroz_context_with_router(&router).expect("ctx");
        let node = ctx.create_node(node_name).build().expect("node");
        Self {
            node,
            _ctx: ctx,
            raw,
            router,
        }
    }

    /// A node on the same router whose context uses a custom key expression format.
    fn custom_format_node(&self, node_name: &str) -> (ZContext, ZNode) {
        let ctx = ZContextBuilder::default()
            .keyexpr_format(KeyExprFormat::Custom(Arc::new(KeyExprFormatterAdapter::<
                RmwZenohFormatter,
            >::new())))
            .disable_multicast_scouting()
            .with_connect_endpoints([self.router.endpoint()])
            .with_mode("client")
            .build()
            .expect("custom ctx");
        let node = ctx.create_node(node_name).build().expect("node");
        (ctx, node)
    }
}

/// A raw subscriber on `topic` (no leading slash) built the default way, and its call count.
fn subscribe_default(node: &ZNode, topic: &str) -> (Arc<AtomicUsize>, RawSubscriber) {
    let (count, callback) = counter();
    let sub = node
        .create_raw_subscriber(&format!("/{topic}"), TYPE_NAME, TYPE_HASH, callback)
        .expect("default raw subscriber");
    (count, sub)
}

/// A raw subscriber on `topic` (no leading slash) with an explicit group setting, and its call
/// count.
fn subscribe(
    node: &ZNode,
    topic: &str,
    group: BufferGroup,
    qos: Option<QosProfile>,
) -> (Arc<AtomicUsize>, RawSubscriber) {
    let (count, callback) = counter();
    let sub = node
        .create_raw_subscriber_with_group(
            &format!("/{topic}"),
            TYPE_NAME,
            TYPE_HASH,
            callback,
            qos,
            group,
        )
        .expect("raw subscriber");
    (count, sub)
}

fn transient_local() -> Option<QosProfile> {
    Some(QosProfile {
        durability: QosDurability::TransientLocal,
        ..Default::default()
    })
}

/// `topic` has one token and it ends `backends:cpu:`.
fn assert_joins(name: &str, raw: &zenoh::Session, topic: &str, who: &str) {
    let tokens = wait_tokens(raw, topic);
    assert!(
        tokens.len() == 1 && tokens[0].ends_with("/backends:cpu:"),
        "{name}: {who} did not announce backends:cpu: {tokens:?}"
    );
}

/// A put on the group key and a put on the base key each reach `count`.
fn assert_delivers_on_both_keys(
    name: &str,
    raw: &zenoh::Session,
    topic: &str,
    count: &AtomicUsize,
    who: &str,
) {
    for key in [group_key(topic), base_key(topic)] {
        let before = count.load(Ordering::Relaxed);
        assert!(
            put_until(raw, &key, &PAYLOAD, RECV, || count.load(Ordering::Relaxed)
                > before),
            "{name}: nothing reached {who} from {key}"
        );
    }
}

/// After a teardown, the token of `topic` is gone and puts on both keys no longer reach `count`.
/// The caller proves before the teardown that the token is visible and that the callback fires.
fn assert_torn_down(name: &str, raw: &zenoh::Session, topic: &str, count: &AtomicUsize) {
    // The token undeclaration takes a moment to reach the router query.
    let deadline = Instant::now() + RECV;
    while !tokens_of(raw, topic).is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
    let left = tokens_of(raw, topic);
    assert!(
        left.is_empty(),
        "{name}: the token is still declared after {RECV:?}: {left:?}"
    );
    std::thread::sleep(POLL);
    let before = count.load(Ordering::Relaxed);
    for key in [group_key(topic), base_key(topic)] {
        for _ in 0..3 {
            raw_put(raw, &key);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    std::thread::sleep(QUIET);
    let after = count.load(Ordering::Relaxed);
    assert!(
        after == before,
        "{name}: the callback ran {} more time(s) after the teardown",
        after - before
    );
}

/// A raw subscriber built the default way joins: it announces `backends:cpu:`, one callback
/// serves the base key and the group key, and it is alive. The typed twin shows the token query
/// works.
#[test]
fn raw_subscriber_default_joins_and_announces_backends() {
    const NAME: &str = "raw_subscriber_default_joins_and_announces_backends";
    let f = Fixture::new("raw_default");
    let _twin = f
        .node
        .create_sub::<UInt8MultiArray>("/raw_default_twin")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .expect("typed twin");
    let (count, _sub) = subscribe_default(&f.node, "raw_default_topic");

    // Control: the typed twin yields exactly one token, and it joins.
    let twin = token_for(&f.raw, "raw_default_twin");
    assert!(
        twin.ends_with("/backends:cpu:"),
        "{NAME}: the typed twin did not join, so the token query proves nothing: {twin}"
    );
    assert_joins(
        NAME,
        &f.raw,
        "raw_default_topic",
        "the default raw subscriber",
    );
    assert_delivers_on_both_keys(
        NAME,
        &f.raw,
        "raw_default_topic",
        &count,
        "the default raw callback",
    );
}

/// A raw subscriber with an explicit `Cpu` setting joins, and one callback serves the base key and
/// the group key, once per sample.
#[test]
fn raw_subscriber_with_explicit_cpu_joins_and_one_callback_serves_both_keys() {
    const NAME: &str = "raw_subscriber_with_explicit_cpu_joins_and_one_callback_serves_both_keys";
    const TOPIC: &str = "raw_cpu_topic";
    let f = Fixture::new("raw_cpu");
    let _twin = f
        .node
        .create_sub::<UInt8MultiArray>("/raw_cpu_twin")
        .with_buffer_group(BufferGroup::Auto)
        .build()
        .expect("typed twin");
    let (count, _sub) = subscribe(&f.node, TOPIC, BufferGroup::Cpu, None);

    // Control: the typed twin joins in the same test.
    let twin = token_for(&f.raw, "raw_cpu_twin");
    assert!(
        twin.ends_with("/backends:cpu:"),
        "{NAME}: the typed twin did not join: {twin}"
    );
    assert_joins(NAME, &f.raw, TOPIC, "the Cpu raw subscriber");
    assert_delivers_on_both_keys(NAME, &f.raw, TOPIC, &count, "the callback");

    // No duplicate delivery: a single put on each key adds exactly one call.
    std::thread::sleep(QUIET);
    for key in [group_key(TOPIC), base_key(TOPIC)] {
        let before = count.load(Ordering::Relaxed);
        raw_put(&f.raw, &key);
        std::thread::sleep(QUIET);
        let got = count.load(Ordering::Relaxed) - before;
        assert!(
            got == 1,
            "{NAME}: one put on {key} produced {got} callback calls"
        );
    }
}

/// With a default and an `Off` raw subscriber side by side, only the default one announces
/// `backends:cpu:`.
#[test]
fn only_the_default_raw_subscriber_announces_backends() {
    const NAME: &str = "only_the_default_raw_subscriber_announces_backends";
    let f = Fixture::new("raw_side");
    let (_c1, _default) = subscribe_default(&f.node, "raw_side_default");
    let (_c2, _off) = subscribe(&f.node, "raw_side_off", BufferGroup::Off, None);

    assert_joins(
        NAME,
        &f.raw,
        "raw_side_default",
        "the default raw subscriber",
    );
    let off_tokens = tokens_of(&f.raw, "raw_side_off");
    let announcing: Vec<&String> = off_tokens
        .iter()
        .filter(|t| t.contains("backends:"))
        .collect();
    assert!(
        announcing.is_empty(),
        "{NAME}: the Off raw subscriber announced backends: {announcing:?}"
    );
}

/// The feedback subscriber of a raw action client stays out of the group. A `Cpu` raw subscriber
/// on the same node is the control.
#[test]
fn raw_action_client_feedback_subscriber_stays_out() {
    const NAME: &str = "raw_action_client_feedback_subscriber_stays_out";
    let f = Fixture::new("raw_action");
    let _client = f
        .node
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
    let (_c, _control) = subscribe(&f.node, "raw_action_control", BufferGroup::Cpu, None);

    assert_joins(
        NAME,
        &f.raw,
        "raw_action_control",
        "the Cpu control (the filter below counts for nothing without it)",
    );
    let feedback: Vec<String> = tokens(&f.raw)
        .into_iter()
        .filter(|t| t.contains("feedback") && t.contains("backends:"))
        .collect();
    assert!(
        feedback.is_empty(),
        "{NAME}: the feedback subscriber announced backends: {feedback:?}"
    );
}

/// Dropping a `Cpu` raw subscriber removes its token and stops its callbacks on both keys. The
/// callback fires before the drop.
#[test]
fn dropping_a_cpu_raw_subscriber_stops_group_key_callbacks() {
    const NAME: &str = "dropping_a_cpu_raw_subscriber_stops_group_key_callbacks";
    const TOPIC: &str = "raw_drop_topic";
    let f = Fixture::new("raw_drop");
    let (count, sub) = subscribe(&f.node, TOPIC, BufferGroup::Cpu, None);

    // Positive control: the callback fires on each key before the drop.
    assert_delivers_on_both_keys(NAME, &f.raw, TOPIC, &count, "the callback before the drop");

    drop(sub);
    assert_torn_down(NAME, &f.raw, TOPIC, &count);
}

/// Tearing down a joined raw subscriber by hand removes the whole subscription. The token is gone
/// and neither key reaches the callback, while the original binding is still in scope.
///
/// Controls in the same test: before the teardown the token is visible and a put on each key
/// reaches the callback, so a silent result afterwards is not a dead setup.
#[test]
fn explicit_teardown_removes_the_token_and_both_subscriptions() {
    const NAME: &str = "explicit_teardown_removes_the_token_and_both_subscriptions";
    const TOPIC: &str = "raw_undeclare_topic";
    let f = Fixture::new("raw_undeclare");
    let (count, sub) = subscribe(&f.node, TOPIC, BufferGroup::Cpu, None);

    assert_joins(NAME, &f.raw, TOPIC, "the subscriber before the teardown");
    assert_delivers_on_both_keys(
        NAME,
        &f.raw,
        TOPIC,
        &count,
        "the callback before the teardown",
    );

    sub.undeclare().expect("teardown");

    // `sub` moved into the teardown, and nothing else owns a handle: only the teardown can have
    // removed anything.
    assert_torn_down(NAME, &f.raw, TOPIC, &count);
}

/// What a raw subscriber that stays out must show, next to a `Cpu` raw subscriber that joins. The
/// control is the positive: it proves the token query and the group key work in this test, so an
/// empty answer for the subject is not a dead setup.
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
    assert_joins(
        name,
        raw,
        control_topic,
        "the Cpu control (the checks below count for nothing without it)",
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

/// `Auto` has no message definition to inspect for a raw subscriber, so it joins as a typed
/// subscriber without a definition does: the token announces `backends:cpu:`, and one callback
/// serves the base key and the group key. A `Cpu` subscriber is the control.
#[test]
fn raw_subscriber_with_group_auto_joins() {
    const NAME: &str = "raw_subscriber_with_group_auto_joins";
    let f = Fixture::new("raw_auto");
    let (_c, _control) = subscribe(&f.node, "raw_auto_control", BufferGroup::Cpu, None);
    let (count, _subject) = subscribe(&f.node, "raw_auto_subject", BufferGroup::Auto, None);

    assert_joins(
        NAME,
        &f.raw,
        "raw_auto_control",
        "the Cpu control (the checks below count for nothing without it)",
    );
    assert_joins(NAME, &f.raw, "raw_auto_subject", "the Auto raw subscriber");
    assert_delivers_on_both_keys(
        NAME,
        &f.raw,
        "raw_auto_subject",
        &count,
        "the Auto raw callback",
    );
}

/// `Off` stays out: it declares no group key and no `backends:` field.
#[test]
fn raw_subscriber_with_group_off_stays_out() {
    const NAME: &str = "raw_subscriber_with_group_off_stays_out";
    let f = Fixture::new("raw_off");
    let (_c, _control) = subscribe(&f.node, "raw_off_control", BufferGroup::Cpu, None);
    let (count, _subject) = subscribe(&f.node, "raw_off_subject", BufferGroup::Off, None);
    assert_stays_out(NAME, &f.raw, "raw_off_control", "raw_off_subject", &count);
}

/// A transient-local raw subscriber never joins, with `Cpu` or with `Auto`. The control is a
/// default raw subscriber on a volatile QoS, which joins.
fn assert_transient_local_stays_out(name: &str, group: BufferGroup) {
    let f = Fixture::new("raw_tl");
    let (_c, _control) = subscribe_default(&f.node, "raw_tl_control");
    let (count, _subject) = subscribe(&f.node, "raw_tl_subject", group, transient_local());
    assert_stays_out(name, &f.raw, "raw_tl_control", "raw_tl_subject", &count);
}

#[test]
fn raw_transient_local_subscriber_stays_out_with_cpu() {
    assert_transient_local_stays_out(
        "raw_transient_local_subscriber_stays_out_with_cpu",
        BufferGroup::Cpu,
    );
}

#[test]
fn raw_transient_local_subscriber_stays_out_with_auto() {
    assert_transient_local_stays_out(
        "raw_transient_local_subscriber_stays_out_with_auto",
        BufferGroup::Auto,
    );
}

/// Only the `rmw_zenoh` key format has the group, so a raw subscriber on a custom format stays
/// out with `Cpu` and with `Auto`. The control is a default raw subscriber on the `rmw_zenoh`
/// format, which joins.
fn assert_custom_format_stays_out(name: &str, group: BufferGroup) {
    let f = Fixture::new("raw_fmt");
    let (_c, _control) = subscribe_default(&f.node, "raw_fmt_control");
    let (_custom_ctx, custom_node) = f.custom_format_node("raw_fmt2");
    let (count, _subject) = subscribe(&custom_node, "raw_fmt_subject", group, None);
    assert_stays_out(name, &f.raw, "raw_fmt_control", "raw_fmt_subject", &count);
}

#[test]
fn raw_custom_key_format_subscriber_stays_out_with_cpu() {
    assert_custom_format_stays_out(
        "raw_custom_key_format_subscriber_stays_out_with_cpu",
        BufferGroup::Cpu,
    );
}

#[test]
fn raw_custom_key_format_subscriber_stays_out_with_auto() {
    assert_custom_format_stays_out(
        "raw_custom_key_format_subscriber_stays_out_with_auto",
        BufferGroup::Auto,
    );
}

// ---------------------------------------------------------------------------------------------
// Router access control
// ---------------------------------------------------------------------------------------------

/// The quoted name of every network interface. A rule whose subject names only `lo` would match
/// nothing.
fn interfaces() -> String {
    std::fs::read_dir("/sys/class/net")
        .expect("list interfaces")
        .map(|e| format!("\"{}\"", e.unwrap().file_name().to_string_lossy()))
        .collect::<Vec<_>>()
        .join(",")
}

/// A router that denies `key_expr` on every interface and allows everything else.
fn router_denying(key_expr: &str) -> TestRouter {
    let access_control = format!(
        r#"{{
            enabled: true,
            default_permission: "allow",
            rules: [{{
                id: "deny_rule", permission: "deny",
                messages: ["put", "declare_subscriber"], flows: ["egress", "ingress"],
                key_exprs: ["{key_expr}"],
            }}],
            subjects: [{{ id: "all", interfaces: [{}] }}],
            policies: [{{ rules: ["deny_rule"], subjects: ["all"] }}],
        }}"#,
        interfaces()
    );
    TestRouter::with_config(move |config| {
        config
            .insert_json5("access_control", &access_control)
            .unwrap();
    })
}

/// A router with default-deny access control that allows only `key_exprs`, on every interface.
/// Liveliness traffic is allowed, so a token still reaches a plain session. This is the allow-list
/// shape: the operator names the topic key and nothing else.
fn router_allowing_only(key_exprs: &[String]) -> TestRouter {
    let keys: Vec<String> = key_exprs.iter().map(|k| format!("\"{k}\"")).collect();
    let access_control = format!(
        r#"{{
            enabled: true,
            default_permission: "deny",
            rules: [{{
                id: "allow_rule", permission: "allow",
                messages: ["put", "declare_subscriber"], flows: ["egress", "ingress"],
                key_exprs: [{}],
            }}, {{
                id: "allow_tokens", permission: "allow",
                messages: ["liveliness_token", "liveliness_query", "declare_liveliness_subscriber"],
                flows: ["egress", "ingress"], key_exprs: ["@ros2_lv/**"],
            }}],
            subjects: [{{ id: "all", interfaces: [{}] }}],
            policies: [{{ rules: ["allow_rule", "allow_tokens"], subjects: ["all"] }}],
        }}"#,
        keys.join(","),
        interfaces()
    );
    TestRouter::with_config(move |config| {
        config
            .insert_json5("access_control", &access_control)
            .unwrap();
    })
}

/// One raw subscriber under a router rule: its topic, its group setting, the key a plain session
/// puts on, and whether the callback must see the samples.
struct AclCase {
    topic: &'static str,
    group: BufferGroup,
    put_group_key: bool,
    delivers: bool,
}

/// Build one raw subscriber per case, put on each case's key, and check what reached it. `build`
/// returns `Ok` under every rule: the starvation is silent.
fn run_acl_cases(name: &str, router: TestRouter, cases: &[AclCase]) {
    let f = Fixture::on(router, "raw_acl_node");
    let raw = &f.raw;
    let (counts, _subs): (Vec<_>, Vec<_>) = cases
        .iter()
        .map(|c| subscribe(&f.node, c.topic, c.group, None))
        .unzip();
    // The token is declared after the subscription, so a visible token ends the wait for it.
    for c in cases {
        let tokens = wait_tokens(raw, c.topic);
        assert_eq!(
            tokens.len() == 1 && tokens[0].ends_with("/backends:cpu:"),
            c.group != BufferGroup::Off,
            "{name}: token for {} ({:?}): {tokens:?}",
            c.topic,
            c.group
        );
    }
    for _ in 0..20 {
        for c in cases {
            let key = if c.put_group_key {
                group_key(c.topic)
            } else {
                base_key(c.topic)
            };
            raw_put(raw, &key);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(QUIET);
    for (c, count) in cases.iter().zip(&counts) {
        let received = count.load(Ordering::Relaxed);
        assert!(
            (received > 0) == c.delivers,
            "{name}: {} ({:?}, group key {}): received {received}, expected {}",
            c.topic,
            c.group,
            c.put_group_key,
            if c.delivers { "some" } else { "none" }
        );
    }
}

/// Router access control starves the group key of a joined raw subscriber silently. A default
/// raw subscriber declares the base key and `<topic key>/_buf_cpu`, so a default-deny router that
/// allows `<topic key>` and not the group key starves the group key only, and `build` still
/// returns `Ok`. The base key keeps delivering.
///
/// Controls in the same test: on the narrow rule, an `Off` subscriber and a default subscriber
/// both receive on the base key, so the puts reach the router and the rule lets the base key
/// through. On the rule that also allows the group key, the default subscriber receives on the
/// group key, which is the remedy the docs give. A wide subscription on `<topic key>/**` fails
/// the narrow rule on the base key too, so the base-key control fails for that shape.
#[test]
fn raw_access_control_starves_the_group_key_silently() {
    const NAME: &str = "raw_access_control_starves_the_group_key_silently";
    use BufferGroup::{Auto, Off};
    let case = |topic, group, put_group_key, delivers| AclCase {
        topic,
        group,
        put_group_key,
        delivers,
    };

    // Allow the topic key only. The group key of a joined subscriber is starved. Its base key and
    // the base key of an Off subscriber still deliver.
    let narrow = [
        case("raw_acl_off", Off, false, true),
        case("raw_acl_auto_base", Auto, false, true),
        case("raw_acl_auto_group", Auto, true, false),
    ];
    let keys: Vec<String> = narrow.iter().map(|c| base_key(c.topic)).collect();
    run_acl_cases(NAME, router_allowing_only(&keys), &narrow);

    // Allow the topic key and the group key. Nothing wider is needed.
    let both = [
        case("raw_aclb_auto_base", Auto, false, true),
        case("raw_aclb_auto_group", Auto, true, true),
    ];
    let keys: Vec<String> = both
        .iter()
        .flat_map(|c| [base_key(c.topic), group_key(c.topic)])
        .collect();
    run_acl_cases(NAME, router_allowing_only(&keys), &both);
}

/// A rule that denies only the group key starves the group key of a joined raw subscriber and
/// leaves its base key flowing. Without the rule, the same subscriber receives on the group key.
#[test]
fn raw_denied_group_key_leaves_the_base_key_flowing() {
    const NAME: &str = "raw_denied_group_key_leaves_the_base_key_flowing";
    use BufferGroup::Auto;
    let case = |topic, put_group_key, delivers| AclCase {
        topic,
        group: Auto,
        put_group_key,
        delivers,
    };

    // Control: an open router delivers on the group key.
    let open = [case("raw_ad_open", true, true)];
    run_acl_cases(NAME, TestRouter::new(), &open);

    let denied = [
        case("raw_ad_group", true, false),
        case("raw_ad_base", false, true),
    ];
    run_acl_cases(NAME, router_denying("0/*/*/*/_buf_cpu"), &denied);
}

// ---------------------------------------------------------------------------------------------
// Exact subscriptions
// ---------------------------------------------------------------------------------------------

/// Whether a plain publisher on `key` has a matching subscriber now.
fn matches(session: &zenoh::Session, key: &str) -> bool {
    use zenoh::Wait;
    let publisher = session
        .declare_publisher(key.to_owned())
        .wait()
        .expect("publisher");
    publisher
        .matching_status()
        .wait()
        .expect("status")
        .matching()
}

/// Poll until `matches(key)` equals `want` or `RECV` passes. It returns the last reading.
fn settle(session: &zenoh::Session, key: &str, want: bool) -> bool {
    let end = Instant::now() + RECV;
    loop {
        let now = matches(session, key);
        if now == want || Instant::now() >= end {
            return now;
        }
        std::thread::sleep(POLL);
    }
}

/// A default raw subscriber declares two exact subscriptions: the base key and the group key. It
/// declares nothing that matches `<base>/_buf/<gid>`, the key a publisher writes for a subscriber
/// outside the group. An `Off` raw subscriber declares the base key only.
///
/// The reading comes from the subscriptions a publisher matches, not from a callback, so a filter
/// in process cannot hide a wide subscription. Controls in the same test: the default subscriber
/// matches its group key, which shows the probe reports a declared subscription, and the `Off`
/// subscriber matches its base key.
#[test]
fn raw_subscriber_declares_two_exact_subscriptions_and_off_declares_one() {
    const NAME: &str = "raw_subscriber_declares_two_exact_subscriptions_and_off_declares_one";
    let f = Fixture::new("raw_exact");
    let raw = &f.raw;
    let (_c1, _default) = subscribe_default(&f.node, "raw_exact_default");
    let (_c2, _off) = subscribe(&f.node, "raw_exact_off", BufferGroup::Off, None);

    let sibling = |topic: &str| format!("{}/_buf/0123456789abcdef", base_key(topic));

    // Controls: both subscribers match their base key, and the default one matches its group key.
    assert!(
        settle(raw, &base_key("raw_exact_default"), true),
        "{NAME}: the default subscriber does not match its base key"
    );
    assert!(
        settle(raw, &group_key("raw_exact_default"), true),
        "{NAME}: the default subscriber does not match its group key"
    );
    assert!(
        settle(raw, &base_key("raw_exact_off"), true),
        "{NAME}: the Off subscriber does not match its base key"
    );

    // The default subscriber matches no sibling key. The Off subscriber matches no group key and
    // no sibling key. The group key control above was read after the same settle time.
    std::thread::sleep(QUIET);
    assert!(
        !matches(raw, &sibling("raw_exact_default")),
        "{NAME}: the default subscriber matches a sibling key, so it is not exact"
    );
    assert!(
        !matches(raw, &group_key("raw_exact_off")),
        "{NAME}: the Off subscriber matches the group key"
    );
    assert!(
        !matches(raw, &sibling("raw_exact_off")),
        "{NAME}: the Off subscriber matches a sibling key"
    );
}

// ---------------------------------------------------------------------------------------------
// Repeated create and destroy
// ---------------------------------------------------------------------------------------------

/// Every cycle of create and drop on one topic declares one token, removes it, and leaves no
/// callback on the group key or the base key. A token that accumulates, or a callback that
/// outlives its subscriber, shows after a few cycles.
#[test]
fn raw_create_and_drop_cycles_leave_no_token_and_no_callback() {
    const NAME: &str = "raw_create_and_drop_cycles_leave_no_token_and_no_callback";
    const CYCLES: usize = 5;
    const TOPIC: &str = "raw_cycle_topic";
    let f = Fixture::new("raw_cycle");

    for cycle in 0..CYCLES {
        let name = format!("{NAME}: cycle {cycle}");
        let (count, sub) = subscribe_default(&f.node, TOPIC);
        // Positive controls: the token is visible and the callback fires before the drop.
        assert_joins(&name, &f.raw, TOPIC, "the raw subscriber");
        assert_delivers_on_both_keys(&name, &f.raw, TOPIC, &count, "the callback");

        drop(sub);
        assert_torn_down(&name, &f.raw, TOPIC, &count);
    }
}
