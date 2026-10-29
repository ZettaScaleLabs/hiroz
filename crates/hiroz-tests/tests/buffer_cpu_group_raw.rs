//! A raw subscriber joins rmw_zenoh's CPU buffer group by default, and `BufferGroup::Off` keeps it out.
//!
//! A raw subscriber has no message schema, so `BufferGroup::Auto` joins it as it joins a typed
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
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::{
    buffer_rmw::{self, interfaces, put_until, router_allowing_only, tokens},
    *,
};
use hiroz::{
    BufferGroup, Builder,
    context::{ZContext, ZContextBuilder},
    ffi::subscriber::RawSubscriber,
    node::ZNode,
    qos::{QosDurability, QosProfile},
};
use hiroz_protocol::{KeyExprFormat, KeyExprFormatterAdapter, RmwZenohFormatter};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

const RECV: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(200);

const TYPE_NAME: &str = "std_msgs::msg::dds_::UInt8MultiArray_";
const TYPE_HASH: &str = "RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385";

/// The CDR bytes of a `UInt8MultiArray` with the data `[1, 2, 3]`.
const PAYLOAD: [u8; 19] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3];

/// The same message with the data `[1, 2, 9]`. A put on the group key carries this payload, so a
/// callback can tell a group-key delivery from a base-key one.
const GROUP_PAYLOAD: [u8; 19] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1, 2, 9];

/// The tokens of `topic` that are visible now.
fn tokens_of(session: &zenoh::Session, topic: &str) -> Vec<String> {
    let needle = format!("/%{topic}/");
    tokens(session)
        .into_iter()
        .filter(|k| k.contains(&needle))
        .collect()
}

/// The tokens of `topic`, once at least one is visible, or none after the deadline.
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

fn raw_put(session: &zenoh::Session, key: &str) {
    use zenoh::Wait;
    session.put(key, PAYLOAD.to_vec()).wait().expect("put");
}

/// What a raw callback received, split by `GROUP_PAYLOAD` against any other payload.
struct Tally {
    group: AtomicUsize,
    base: AtomicUsize,
}

impl Tally {
    /// Samples that carried `GROUP_PAYLOAD`.
    fn group(&self) -> usize {
        self.group.load(Ordering::Relaxed)
    }

    /// Samples that carried any other payload.
    fn base(&self) -> usize {
        self.base.load(Ordering::Relaxed)
    }

    fn total(&self) -> usize {
        self.group() + self.base()
    }
}

/// A raw callback that counts what it receives.
fn counter() -> (Arc<Tally>, impl Fn(&[u8]) + Send + Sync + 'static) {
    let count = Arc::new(Tally {
        group: AtomicUsize::new(0),
        base: AtomicUsize::new(0),
    });
    let c = count.clone();
    (count, move |payload: &[u8]| {
        if payload == GROUP_PAYLOAD {
            c.group.fetch_add(1, Ordering::Relaxed);
        } else {
            c.base.fetch_add(1, Ordering::Relaxed);
        }
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
        let raw = buffer_rmw::client_session(router.endpoint());
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
fn subscribe_default(node: &ZNode, topic: &str) -> (Arc<Tally>, RawSubscriber) {
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
) -> (Arc<Tally>, RawSubscriber) {
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

/// A put on the group key and a put on the base key each reach `count`. The two keys carry
/// different payloads and `count` tallies them apart, so a delayed group-key delivery cannot pass
/// the base-key phase.
fn assert_delivers_on_both_keys(
    name: &str,
    raw: &zenoh::Session,
    topic: &str,
    count: &Tally,
    who: &str,
) {
    let before = count.group();
    let key = group_key(topic);
    assert!(
        put_until(raw, &key, &GROUP_PAYLOAD, RECV, || count.group() > before),
        "{name}: nothing reached {who} from {key}"
    );
    let before = count.base();
    let key = base_key(topic);
    assert!(
        put_until(raw, &key, &PAYLOAD, RECV, || count.base() > before),
        "{name}: nothing reached {who} from {key}"
    );
}

/// After a teardown, the token of `topic` is gone and puts on both keys no longer reach `count`.
/// The caller proves before the teardown that the token is visible and that the callback fires.
fn assert_torn_down(name: &str, raw: &zenoh::Session, topic: &str, count: &Tally) {
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
    let before = count.total();
    for key in [group_key(topic), base_key(topic)] {
        for _ in 0..3 {
            raw_put(raw, &key);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    std::thread::sleep(QUIET);
    let after = count.total();
    assert!(
        after == before,
        "{name}: the callback ran {} more time(s) after the teardown",
        after - before
    );
}

/// A raw subscriber built by `create_raw_subscriber` or by `create_raw_subscriber_with_qos` joins:
/// it announces `backends:cpu:`, and one callback serves the base key and the group key.
#[test]
fn raw_subscriber_default_joins_and_announces_backends() {
    const NAME: &str = "raw_subscriber_default_joins_and_announces_backends";
    let f = Fixture::new("raw_default");
    let (count, _sub) = subscribe_default(&f.node, "raw_default_topic");
    let (qos_count, callback) = counter();
    let _qos_sub = f
        .node
        .create_raw_subscriber_with_qos("/raw_qos_topic", TYPE_NAME, TYPE_HASH, callback, None)
        .expect("raw subscriber with qos");

    assert_joins(
        NAME,
        &f.raw,
        "raw_default_topic",
        "the default raw subscriber",
    );
    assert_joins(NAME, &f.raw, "raw_qos_topic", "the raw subscriber with QoS");
    assert_delivers_on_both_keys(
        NAME,
        &f.raw,
        "raw_default_topic",
        &count,
        "the default callback",
    );
    assert_delivers_on_both_keys(
        NAME,
        &f.raw,
        "raw_qos_topic",
        &qos_count,
        "the QoS callback",
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
    subject_count: &Tally,
) {
    assert_joins(
        name,
        raw,
        control_topic,
        "the Cpu control (the checks below count for nothing without it)",
    );
    assert!(
        put_until(raw, &base_key(subject_topic), &PAYLOAD, RECV, || {
            subject_count.total() > 0
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
    let before = subject_count.total();
    for _ in 0..5 {
        raw_put(raw, &group_key(subject_topic));
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(QUIET);
    let got = subject_count.total() - before;
    assert!(
        got == 0,
        "{name}: the subject received {got} sample(s) on its group key, so it joined the group"
    );
}

/// The three ways a raw subscriber stays out of the group: `Off`, a transient-local QoS and a key
/// expression format other than `rmw_zenoh`'s. Each subject goes through `assert_stays_out`. The
/// control is a default raw subscriber on the `rmw_zenoh` format, which joins.
#[test]
fn raw_subscribers_that_stay_out_announce_no_backends_and_skip_the_group_key() {
    const NAME: &str = "raw_subscribers_that_stay_out_announce_no_backends_and_skip_the_group_key";
    let f = Fixture::new("raw_out");
    let (_c, _control) = subscribe_default(&f.node, "raw_out_control");
    let (_custom_ctx, custom_node) = f.custom_format_node("raw_out_custom");
    let subjects = [
        (
            "raw_out_off",
            subscribe(&f.node, "raw_out_off", BufferGroup::Off, None),
        ),
        (
            "raw_out_tl",
            subscribe(&f.node, "raw_out_tl", BufferGroup::Auto, transient_local()),
        ),
        (
            "raw_out_fmt",
            subscribe(&custom_node, "raw_out_fmt", BufferGroup::Auto, None),
        ),
    ];
    for (topic, (count, _sub)) in &subjects {
        assert_stays_out(
            &format!("{NAME}: {topic}"),
            &f.raw,
            "raw_out_control",
            topic,
            count,
        );
    }
}

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

/// A raw join logs one `debug` event that carries the base key and the group key, and a raw
/// subscriber set to `Off` logs none. The test selects the events by the marker `CPU buffer group`.
/// Controls in the same test: the joined subscriber announces the group in its token, so a missing
/// event is a missing log and not a missing join, and the `Off` subscriber announces none.
#[test]
fn raw_join_logs_both_keys_at_debug_level() {
    const NAME: &str = "raw_join_logs_both_keys_at_debug_level";
    let out = Captured::default();
    // Only the hiroz target is enabled: a debug span of another crate would cross threads and
    // break the thread-local subscriber.
    let _guard = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer({
                    let out = out.clone();
                    move || out.clone()
                })
                .with_ansi(false),
        )
        .with(Targets::new().with_target("hiroz", LevelFilter::DEBUG))
        .set_default();
    let f = Fixture::new("raw_log");
    let (_joined_count, _joined) = subscribe_default(&f.node, "raw_log_joined");
    let (_off_count, _off) = subscribe(&f.node, "raw_log_off", BufferGroup::Off, None);

    assert_joins(NAME, &f.raw, "raw_log_joined", "the default raw subscriber");
    assert!(
        wait_tokens(&f.raw, "raw_log_off")
            .iter()
            .all(|t| !t.contains("backends:")),
        "{NAME}: control: the Off subscriber announces a group"
    );
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("CPU buffer group"))
        .collect();
    let naming = |topic: &str| {
        let (base, group) = (base_key(topic), group_key(topic));
        lines
            .iter()
            .filter(|l| l.contains(&base) && l.contains(&group))
            .count()
    };
    assert_eq!(naming("raw_log_joined"), 1, "{NAME}: events: {text}");
    assert_eq!(naming("raw_log_off"), 0, "{NAME}: events: {text}");
    assert_eq!(lines.len(), 1, "{NAME}: one event per join: {text}");
}

// ---------------------------------------------------------------------------------------------
// Router access control
// ---------------------------------------------------------------------------------------------

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
        let received = count.total();
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

/// When the token of a default raw subscriber becomes visible, its group subscription is already
/// declared. A publisher that reads the token then finds a subscription on the group key. The
/// shared check runs inside the callback that reports the token, in 30 trials.
#[test]
fn raw_group_subscription_is_declared_before_the_token_is_visible() {
    common::buffer_rmw::assert_group_declared_before_token(
        "raw_group_subscription_is_declared_before_the_token_is_visible",
        group_key,
        |node, topic| {
            node.create_raw_subscriber(&format!("/{topic}"), TYPE_NAME, TYPE_HASH, |_: &[u8]| {})
                .expect("raw subscriber")
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Repeated create and destroy
// ---------------------------------------------------------------------------------------------

/// Every cycle of create and teardown on one topic declares one token, removes it, and leaves no
/// callback on the group key or the base key. Even cycles drop the subscriber and odd cycles call
/// `undeclare`. A token that accumulates, or a callback that outlives its subscriber, shows after a
/// few cycles.
#[test]
fn raw_create_and_teardown_cycles_leave_no_token_and_no_callback() {
    const NAME: &str = "raw_create_and_teardown_cycles_leave_no_token_and_no_callback";
    const CYCLES: usize = 5;
    const TOPIC: &str = "raw_cycle_topic";
    let f = Fixture::new("raw_cycle");

    for cycle in 0..CYCLES {
        let name = format!("{NAME}: cycle {cycle}");
        let (count, sub) = subscribe_default(&f.node, TOPIC);
        // Positive controls: the token is visible and the callback fires before the teardown.
        assert_joins(&name, &f.raw, TOPIC, "the raw subscriber");
        assert_delivers_on_both_keys(&name, &f.raw, TOPIC, &count, "the callback");

        if cycle % 2 == 0 {
            drop(sub);
        } else {
            sub.undeclare().expect("teardown");
        }
        assert_torn_down(&name, &f.raw, TOPIC, &count);
    }
}

// ---------------------------------------------------------------------------------------------
// The C ABI: `hiroz_subscriber_create_with_group`
// ---------------------------------------------------------------------------------------------

/// The C callback of the ABI tests. `user_data` is the address of a leaked `Tally`. It tallies a
/// payload as `counter` does, so a test can tell a group-key delivery from a base-key one.
extern "C" fn c_count(user_data: usize, data: *const u8, len: usize) {
    let tally = unsafe { &*(user_data as *const Tally) };
    let payload = unsafe { std::slice::from_raw_parts(data, len) };
    if payload == GROUP_PAYLOAD {
        tally.group.fetch_add(1, Ordering::Relaxed);
    } else {
        tally.base.fetch_add(1, Ordering::Relaxed);
    }
}

/// A tally that outlives the test, and the `user_data` that names it. The C callback can run until
/// its subscriber is destroyed, so the tally is never freed.
fn c_tally() -> (&'static Tally, usize) {
    let tally: &'static Tally = Box::leak(Box::new(Tally {
        group: AtomicUsize::new(0),
        base: AtomicUsize::new(0),
    }));
    (tally, tally as *const Tally as usize)
}

/// A context and a node made through the C ABI, as a C caller makes them.
struct CFixture {
    ctx: *mut hiroz::ffi::context::CContext,
    node: *mut hiroz::ffi::node::CNode,
}

impl CFixture {
    fn new(endpoint: &str, name: &str) -> Self {
        use std::ffi::CString;
        let endpoint = CString::new(endpoint).unwrap();
        let endpoints = [endpoint.as_ptr()];
        let mode = CString::new("client").unwrap();
        let config = hiroz::ffi::context::CContextConfig {
            domain_id: 0,
            config_file: std::ptr::null(),
            connect_endpoints: endpoints.as_ptr(),
            connect_endpoints_count: 1,
            mode: mode.as_ptr(),
            disable_multicast_scouting: true,
            connect_to_local_zenohd: false,
            json_config: std::ptr::null(),
            remap_rules: std::ptr::null(),
            remap_rules_count: 0,
            enable_logging: false,
            namespace: std::ptr::null(),
        };
        let name = CString::new(name).unwrap();
        unsafe {
            let ctx = hiroz::ffi::context::hiroz_context_create_with_config(&config);
            assert!(!ctx.is_null(), "the C context was not created");
            let node = hiroz::ffi::node::hiroz_node_create(ctx, name.as_ptr(), std::ptr::null());
            assert!(!node.is_null(), "the C node was not created");
            Self { ctx, node }
        }
    }

    /// `hiroz_subscriber_create_with_group` on `/topic`. Null on failure.
    fn with_group(
        &self,
        topic: &str,
        user_data: usize,
        group: i32,
    ) -> *mut hiroz::ffi::subscriber::CSubscriber {
        use std::ffi::CString;
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz::ffi::subscriber::hiroz_subscriber_create_with_group(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                c_count,
                user_data,
                std::ptr::null(),
                group,
            )
        }
    }

    /// `hiroz_subscriber_create` on `/topic`: no group choice. Null on failure.
    fn plain(&self, topic: &str, user_data: usize) -> *mut hiroz::ffi::subscriber::CSubscriber {
        use std::ffi::CString;
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz::ffi::subscriber::hiroz_subscriber_create(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                c_count,
                user_data,
            )
        }
    }

    /// `hiroz_subscriber_create_with_qos` on `/topic` with the default QoS. Null on failure.
    fn with_qos(&self, topic: &str, user_data: usize) -> *mut hiroz::ffi::subscriber::CSubscriber {
        use std::ffi::CString;
        let topic = CString::new(format!("/{topic}")).unwrap();
        let type_name = CString::new(TYPE_NAME).unwrap();
        let type_hash = CString::new(TYPE_HASH).unwrap();
        unsafe {
            hiroz::ffi::subscriber::hiroz_subscriber_create_with_qos(
                self.node,
                topic.as_ptr(),
                type_name.as_ptr(),
                type_hash.as_ptr(),
                c_count,
                user_data,
                std::ptr::null(),
            )
        }
    }
}

impl Drop for CFixture {
    fn drop(&mut self) {
        unsafe {
            hiroz::ffi::node::hiroz_node_destroy(self.node);
            hiroz::ffi::context::hiroz_context_destroy(self.ctx);
        }
    }
}

fn c_destroy(sub: *mut hiroz::ffi::subscriber::CSubscriber) {
    unsafe { hiroz::ffi::subscriber::hiroz_subscriber_destroy(sub) };
}

/// Groups `1` (`Cpu`) and `2` (`Auto`) join: the token announces `backends:cpu:` and one callback
/// serves the base key and the group key. Group `0` on the same node is the control that the token
/// query tells joined and not joined apart.
#[test]
fn c_abi_groups_cpu_and_auto_join() {
    const NAME: &str = "c_abi_groups_cpu_and_auto_join";
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_abi_join");
    let (_off_tally, off_data) = c_tally();
    let (cpu_tally, cpu_data) = c_tally();
    let (auto_tally, auto_data) = c_tally();
    let off = c.with_group("c_abi_join_off", off_data, 0);
    let cpu = c.with_group("c_abi_join_cpu", cpu_data, 1);
    let auto = c.with_group("c_abi_join_auto", auto_data, 2);
    assert!(
        !off.is_null() && !cpu.is_null() && !auto.is_null(),
        "{NAME}: a creator returned null"
    );

    assert_joins(
        NAME,
        &raw,
        "c_abi_join_cpu",
        "the C subscriber with group 1",
    );
    assert_joins(
        NAME,
        &raw,
        "c_abi_join_auto",
        "the C subscriber with group 2",
    );
    let off_tokens = tokens_of(&raw, "c_abi_join_off");
    assert!(
        off_tokens.iter().all(|t| !t.contains("backends:")),
        "{NAME}: the group 0 control announced backends: {off_tokens:?}"
    );
    assert_delivers_on_both_keys(
        NAME,
        &raw,
        "c_abi_join_cpu",
        cpu_tally,
        "the group 1 callback",
    );
    assert_delivers_on_both_keys(
        NAME,
        &raw,
        "c_abi_join_auto",
        auto_tally,
        "the group 2 callback",
    );

    for sub in [off, cpu, auto] {
        c_destroy(sub);
    }
}

/// Group `0` stays out: no group key, no `backends:` field. A C subscriber with group `1` on the
/// same node is the control.
#[test]
fn c_abi_group_off_stays_out() {
    const NAME: &str = "c_abi_group_off_stays_out";
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_abi_off");
    let (_control_tally, control_data) = c_tally();
    let (tally, data) = c_tally();
    let control = c.with_group("c_abi_off_control", control_data, 1);
    let subject = c.with_group("c_abi_off_subject", data, 0);
    assert!(
        !control.is_null() && !subject.is_null(),
        "{NAME}: a creator returned null"
    );

    assert_stays_out(NAME, &raw, "c_abi_off_control", "c_abi_off_subject", tally);

    c_destroy(subject);
    c_destroy(control);
}

/// `hiroz_subscriber_create` and `hiroz_subscriber_create_with_qos` make no group choice. They
/// follow the node default, which is `Auto` when nothing sets it, so they join. Group `0` on the
/// same node is the control.
#[test]
fn c_abi_creators_without_a_group_join_by_default() {
    const NAME: &str = "c_abi_creators_without_a_group_join_by_default";
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_abi_def");
    let (_off_tally, off_data) = c_tally();
    let (plain_tally, plain_data) = c_tally();
    let (qos_tally, qos_data) = c_tally();
    let off = c.with_group("c_abi_def_off", off_data, 0);
    let plain = c.plain("c_abi_def_plain", plain_data);
    let qos = c.with_qos("c_abi_def_qos", qos_data);
    assert!(
        !off.is_null() && !plain.is_null() && !qos.is_null(),
        "{NAME}: a creator returned null"
    );

    assert_joins(NAME, &raw, "c_abi_def_plain", "hiroz_subscriber_create");
    assert_joins(
        NAME,
        &raw,
        "c_abi_def_qos",
        "hiroz_subscriber_create_with_qos",
    );
    let off_tokens = tokens_of(&raw, "c_abi_def_off");
    assert!(
        off_tokens.iter().all(|t| !t.contains("backends:")),
        "{NAME}: the group 0 control announced backends: {off_tokens:?}"
    );
    assert_delivers_on_both_keys(
        NAME,
        &raw,
        "c_abi_def_plain",
        plain_tally,
        "the plain callback",
    );
    assert_delivers_on_both_keys(NAME, &raw, "c_abi_def_qos", qos_tally, "the QoS callback");

    for sub in [qos, plain, off] {
        c_destroy(sub);
    }
}

/// A group other than `0`, `1` and `2` is an error: the function returns null and declares
/// nothing. Group `1` on the same node is the control that the node and the arguments are valid.
#[test]
fn c_abi_unknown_group_returns_null_and_declares_nothing() {
    const NAME: &str = "c_abi_unknown_group_returns_null_and_declares_nothing";
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_abi_bad");
    let (_tally, data) = c_tally();

    let control = c.with_group("c_abi_bad_control", data, 1);
    assert!(
        !control.is_null(),
        "{NAME}: the control with group 1 returned null, so a null below proves nothing"
    );
    let bad = [3, -1, 99, i32::MIN];
    for (i, group) in bad.into_iter().enumerate() {
        let sub = c.with_group(&format!("c_abi_bad_{i}"), data, group);
        let accepted = !sub.is_null();
        if accepted {
            c_destroy(sub);
        }
        assert!(!accepted, "{NAME}: group {group} was accepted");
    }
    // Let a token that a rejected call wrongly declared become visible.
    std::thread::sleep(QUIET);
    for i in 0..bad.len() {
        let left = tokens_of(&raw, &format!("c_abi_bad_{i}"));
        assert!(
            left.is_empty(),
            "{NAME}: a rejected group left tokens behind: {left:?}"
        );
    }

    c_destroy(control);
}

/// Every cycle of create and destroy on one topic declares one token, removes it, and leaves no
/// callback on the group key or the base key.
#[test]
fn c_abi_create_and_destroy_cycles_leave_no_token_and_no_callback() {
    const NAME: &str = "c_abi_create_and_destroy_cycles_leave_no_token_and_no_callback";
    const CYCLES: usize = 5;
    const TOPIC: &str = "c_abi_cycle_topic";
    let router = TestRouter::new();
    let raw = buffer_rmw::client_session(router.endpoint());
    let c = CFixture::new(router.endpoint(), "c_abi_cycle");

    for cycle in 0..CYCLES {
        let name = format!("{NAME}: cycle {cycle}");
        let (tally, data) = c_tally();
        let sub = c.with_group(TOPIC, data, 2);
        assert!(!sub.is_null(), "{name}: the creator returned null");
        // Positive controls: the token is visible and the callback fires before the destroy.
        assert_joins(&name, &raw, TOPIC, "the C subscriber");
        assert_delivers_on_both_keys(&name, &raw, TOPIC, tally, "the C callback");

        c_destroy(sub);
        assert_torn_down(&name, &raw, TOPIC, tally);
    }
}
