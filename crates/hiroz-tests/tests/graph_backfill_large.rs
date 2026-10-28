//! `Graph::new` must return when its session already holds more live liveliness
//! tokens than zenoh's default reply channel can buffer.
//!
//! `Graph::new` replays every live token into the backfill query's reply channel
//! before the code that drains it starts. The default channel holds 256 replies, so
//! a session that already holds more than 256 matching tokens blocks inside
//! `Graph::new`.
//!
//! The tokens are declared on the session that then builds the `Graph`. A second
//! session behind a router does not reproduce the hang: the replay then runs on a
//! session thread and a drain loop empties the channel, so nothing blocks.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use hiroz::{context::KeyExprFormat, graph::Graph};
use zenoh::{Wait, handlers::FifoChannel};

const DOMAIN_ID: usize = 0;
/// Above the 256-slot default reply channel, with headroom.
const LIVE_TOKENS: usize = 300;
const BUILD_DEADLINE: Duration = Duration::from_secs(30);

#[test]
fn graph_new_returns_when_more_than_256_tokens_are_live() {
    let mut config = zenoh::Config::default();
    // Offline: no other session can contribute tokens or replies.
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .expect("disable multicast scouting");
    config
        .insert_json5("scouting/gossip/enabled", "false")
        .expect("disable gossip");
    let session = zenoh::open(config).wait().expect("open session");

    let pattern = format!("@ros2_lv/{DOMAIN_ID}/**");
    let tokens: Vec<_> = (0..LIVE_TOKENS)
        .map(|i| {
            session
                .liveliness()
                .declare_token(format!("@ros2_lv/{DOMAIN_ID}/tok_{i}"))
                .wait()
                .expect("declare token")
        })
        .collect();

    // Positive control: with an oversized handler the same query returns every
    // token, so the count is really above the default channel's 256 slots. Without
    // this, a pass would not show that the fix was needed.
    let control = session
        .liveliness()
        .get(&pattern)
        .with(FifoChannel::new(65536))
        .timeout(Duration::from_secs(3))
        .wait()
        .expect("control liveliness query");
    let live = control.iter().count();
    assert!(
        live >= LIVE_TOKENS,
        "control failed: the query sees only {live} of {LIVE_TOKENS} tokens, so \
         `Graph::new` would not exceed the 256-slot channel and this test could not fail"
    );

    // Build off-thread so a blocked `Graph::new` fails the test instead of hanging it.
    // The thread, session and tokens are leaked once the outcome is known: dropping
    // them would wait on the very lock a blocked build holds.
    let (tx, rx) = mpsc::channel();
    let build_session = session.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        let graph = Graph::new(&build_session, DOMAIN_ID, KeyExprFormat::default());
        let _ = tx.send((start.elapsed(), graph.is_ok()));
        std::mem::forget(graph);
    });
    let outcome = rx.recv_timeout(BUILD_DEADLINE);
    std::mem::forget(tokens);
    std::mem::forget(session);

    let (elapsed, ok) = outcome.unwrap_or_else(|_| {
        panic!(
            "Graph::new still blocked after {BUILD_DEADLINE:?} with {live} live tokens; \
             the backfill reply channel is too small"
        )
    });
    println!("Graph::new returned in {elapsed:?} against {live} live tokens");
    assert!(ok, "Graph::new returned an error");
}
