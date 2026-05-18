//! ZCache consumer — ExtractorStamp (application-level timestamp).
//!
//! The stamp extractor reads the message payload as `"msg-<seq>"` and maps it
//! to a logical [`SystemTime`].  In real sensor-fusion code you would read
//! `header.stamp` instead.
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
//! cargo run --example z_cache_app_stamp
//! ```

use std::time::{Duration, SystemTime};

use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

pub async fn run(
    ctx: hiroz::context::ZContext,
    topic: String,
    capacity: usize,
    count: usize,
) -> Result<()> {
    let node = ctx.create_node("cache_consumer_app").build()?;

    // Extractor reads the sequence number from the payload as seconds since
    // UNIX_EPOCH.  For real data: read header.stamp.
    let cache = node
        .create_cache::<RosString>(&topic, capacity)
        .with_stamp(|msg: &RosString| {
            let secs: u64 = msg
                .data
                .split('-')
                .next_back()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
        })
        .build()?;

    println!(
        "[cache/app] subscribed to '{}', capacity={} (app-level stamp)",
        topic, capacity,
    );

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut i = 0usize;
    loop {
        println!(
            "[cache/app] len={} | oldest={:?} | newest={:?}",
            cache.len(),
            cache.oldest_stamp(),
            cache.newest_stamp(),
        );

        // Find the message whose logical timestamp is closest to t=5s.
        let target = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let nearest = cache.get_nearest(target);
        println!(
            "[cache/app] nearest to t=5s: {}",
            nearest
                .as_ref()
                .map(|m| m.data.as_str())
                .unwrap_or("(none)"),
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
    run(ctx, "/cache_demo".into(), 20, 0).await
}
