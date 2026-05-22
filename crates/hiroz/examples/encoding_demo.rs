use std::time::Duration;

use hiroz::Builder;
/// Demonstrates runtime encoding selection for hiroz publishers.
///
/// This example shows how to:
/// 1. Create publishers with different encoding formats (CDR, Protobuf)
/// 2. Publish messages with encoding metadata
/// 3. Have subscribers detect encoding automatically
///
/// Run with: cargo run --example encoding_demo
use hiroz::encoding::Encoding;
use hiroz_msgs::std_msgs::String as RosString;

fn main() -> zenoh::Result<()> {
    println!("=== hiroz Dynamic Encoding Demo ===\n");

    // Create context and node
    let ctx = hiroz::context::ZContextBuilder::default().build()?;
    let node = ctx.create_node("encoding_demo").build()?;

    println!("1. Creating publishers with different encodings...\n");

    // Publisher with CDR encoding (default)
    let cdr_pub = node
        .create_pub::<RosString>("/topic/cdr")
        .with_encoding(Encoding::cdr())
        .build()?;
    println!("   ✓ CDR publisher ready: /topic/cdr");

    // Publisher with Protobuf encoding (if feature enabled)
    #[cfg(feature = "protobuf")]
    let proto_pub = {
        let pub_ = node
            .create_pub::<RosString>("/topic/protobuf")
            .with_encoding(Encoding::protobuf().with_schema("std_msgs/msg/String"))
            .build()?;
        println!("   ✓ Protobuf publisher ready: /topic/protobuf (schema: std_msgs/msg/String)");
        pub_
    };
    #[cfg(not(feature = "protobuf"))]
    println!("   ℹ Protobuf support not enabled (use --features protobuf)");

    println!("\n2. Publishing messages...\n");

    // Publish with CDR
    let msg = RosString {
        data: "Hello from CDR!".to_string(),
    };
    cdr_pub.publish(&msg)?;
    println!("   → Published CDR message: '{}'", msg.data);

    // Publish with Protobuf
    #[cfg(feature = "protobuf")]
    {
        let msg = RosString {
            data: "Hello from Protobuf!".to_string(),
        };
        proto_pub.publish(&msg)?;
        println!("   → Published Protobuf message: '{}'", msg.data);
    }

    println!("\n3. Encoding metadata is automatically transmitted with each message");
    println!("   Subscribers can detect the encoding via sample.encoding()");
    println!("   Format: mime_type[; schema=schema_name]");
    println!("     - CDR:      application/octet-stream");
    println!("     - Protobuf: application/protobuf; schema=std_msgs/msg/String");

    println!("\n4. Performance benefits:");
    println!("   ✓ Zenoh encoding is cached (computed once during build())");
    println!("   ✓ Zero overhead per publish() call");
    println!("   ✓ No string formatting on hot path");

    println!("\n=== Demo complete ===");
    println!("Press Ctrl+C to exit");

    // Keep alive
    std::thread::sleep(Duration::from_secs(60));

    Ok(())
}
