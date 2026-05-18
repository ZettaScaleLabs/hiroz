//! ZCache talker — publishes a string message every 100 ms.
//!
//! ## Usage
//!
//! ```text
//! cargo run --example z_cache_talker
//! ```
//!
//! Run alongside [`z_cache_zenoh_stamp`] or [`z_cache_app_stamp`] to see the
//! cache fill up.

use std::time::Duration;

use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::std_msgs::String as RosString;

pub async fn run(ctx: hiroz::context::ZContext, topic: String, count: usize) -> Result<()> {
    let node = ctx.create_node("cache_talker").build()?;
    let publisher = node.create_pub::<RosString>(&topic).build()?;

    println!("[talker] publishing on '{}' every 100 ms", topic);

    let mut seq: u64 = 0;
    loop {
        let msg = RosString {
            data: format!("msg-{seq}"),
        };
        publisher.async_publish(&msg).await?;
        println!("[talker] sent: {}", msg.data);
        tokio::time::sleep(Duration::from_millis(100)).await;
        seq += 1;
        if count > 0 && seq as usize >= count {
            break;
        }
    }
    Ok(())
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    zenoh::init_log_from_env_or("error");
    let ctx = ZContextBuilder::default().build()?;
    run(ctx, "/cache_demo".into(), 0).await
}
