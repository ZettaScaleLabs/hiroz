//! ZCache consumer — ZenohStamp (zero-config, default).
//!
//! Queries a sliding `[now - window, now]` window every 500 ms using the
//! Zenoh transport timestamp, which requires no extractor.  Works for any
//! message type.
//!
//! ## Usage
//!
//! Terminal 1:
//! ```text
//! cargo run --example z_cache_talker
//! ```
//!
//! Terminal 2:
//! ```text
//! cargo run --example z_cache_zenoh_stamp
//! ```

use std::time::{Duration, SystemTime};

use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

pub async fn run(
    ctx: hiroz::context::ZContext,
    topic: String,
    capacity: usize,
    window_ms: u64,
    count: usize,
) -> Result<()> {
    let node = ctx.create_node("cache_consumer").build()?;

    // No extractor required — Zenoh transport timestamp is used automatically.
    let cache = node.create_cache::<RosString>(&topic, capacity).build()?;

    println!(
        "[cache/zenoh] subscribed to '{}', capacity={}, window={}ms",
        topic, capacity, window_ms
    );

    // Give the subscription time to connect before querying.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let window = Duration::from_millis(window_ms);
    let mut i = 0usize;
    loop {
        let now = SystemTime::now();
        let msgs = cache.get_interval(now - window, now);
        let newest = cache.get_before(now);

        println!(
            "[cache/zenoh] window=[now-{}ms, now]: {} messages | newest: {}",
            window_ms,
            msgs.len(),
            newest.as_ref().map(|m| m.data.as_str()).unwrap_or("(none)"),
        );

        i += 1;
        if count > 0 && i >= count {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(())
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    zenoh::init_log_from_env_or("error");
    let ctx = ZContextBuilder::default().build()?;
    run(ctx, "/cache_demo".into(), 20, 500, 0).await
}
