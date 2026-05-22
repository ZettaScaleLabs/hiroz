use std::{env, time::Duration};

use hiroz::Builder;
/// Demonstrates Protobuf encoding interoperability between hiroz and external Zenoh applications.
///
/// This example shows:
/// 1. Publishing ROS messages with Protobuf encoding
/// 2. Subscribing with encoding validation
/// 3. How external Zenoh apps can consume hiroz messages
///
/// Run publisher: cargo run --example protobuf_interop --features protobuf -- pub
/// Run subscriber: cargo run --example protobuf_interop --features protobuf -- sub
/// Run both: cargo run --example protobuf_interop --features protobuf
use hiroz::encoding::Encoding;
use hiroz_msgs::std_msgs::String as RosString;

fn main() -> zenoh::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("both");

    match mode {
        "pub" => run_publisher(),
        "sub" => run_subscriber(),
        "both" => run_both(),
        _ => {
            println!("Usage: protobuf_interop [pub|sub|both]");
            Ok(())
        }
    }
}

fn run_publisher() -> zenoh::Result<()> {
    println!("=== Protobuf Interop - Publisher ===\n");

    let ctx = hiroz::context::ZContextBuilder::default().build()?;
    let node = ctx.create_node("protobuf_publisher").build()?;

    // Create publisher with Protobuf encoding
    let pub_proto = node
        .create_pub::<RosString>("/interop/protobuf")
        .with_encoding(Encoding::protobuf().with_schema("std_msgs/msg/String"))
        .build()?;

    // Create publisher with CDR for comparison
    let pub_cdr = node
        .create_pub::<RosString>("/interop/cdr")
        .with_encoding(Encoding::cdr())
        .build()?;

    println!("Publishers ready:");
    println!("  - /interop/protobuf (Protobuf encoding)");
    println!("  - /interop/cdr (CDR encoding)");
    println!("\nPublishing messages every second...\n");

    let mut counter = 0;
    loop {
        counter += 1;

        // Publish Protobuf message
        let msg_proto = RosString {
            data: format!("Protobuf message #{}", counter),
        };
        pub_proto.publish(&msg_proto)?;
        println!(
            "[PROTO] Published: '{}' (encoding: application/protobuf)",
            msg_proto.data
        );

        // Publish CDR message for comparison
        let msg_cdr = RosString {
            data: format!("CDR message #{}", counter),
        };
        pub_cdr.publish(&msg_cdr)?;
        println!(
            "[CDR]   Published: '{}' (encoding: application/octet-stream)",
            msg_cdr.data
        );

        println!();
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn run_subscriber() -> zenoh::Result<()> {
    println!("=== Protobuf Interop - Subscriber ===\n");

    let ctx = hiroz::context::ZContextBuilder::default().build()?;
    let node = ctx.create_node("protobuf_subscriber").build()?;

    // Subscriber with Protobuf encoding validation
    let sub_proto = node
        .create_sub::<RosString>("/interop/protobuf")
        .with_encoding(Encoding::protobuf().with_schema("std_msgs/msg/String"))
        .build()?;

    // Subscriber with CDR encoding validation
    let sub_cdr = node
        .create_sub::<RosString>("/interop/cdr")
        .with_encoding(Encoding::cdr())
        .build()?;

    println!("Subscribers ready:");
    println!("  - /interop/protobuf (expects Protobuf)");
    println!("  - /interop/cdr (expects CDR)");
    println!("\nListening for messages...\n");

    loop {
        // Receive Protobuf messages
        if let Ok(msg) = sub_proto.recv() {
            println!("[PROTO] Received: '{}'", msg.data);
        }

        // Receive CDR messages
        if let Ok(msg) = sub_cdr.recv() {
            println!("[CDR]   Received: '{}'", msg.data);
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run_both() -> zenoh::Result<()> {
    println!("=== Protobuf Interop - Full Demo ===\n");
    println!("This demonstrates encoding interoperability:");
    println!("  1. Publishers send messages with explicit encoding");
    println!("  2. Subscribers validate received encoding");
    println!("  3. External Zenoh apps can read the encoding metadata\n");

    let ctx = hiroz::context::ZContextBuilder::default().build()?;
    let node = ctx.create_node("protobuf_demo").build()?;

    // Create publishers
    let pub_proto = node
        .create_pub::<RosString>("/demo/protobuf")
        .with_encoding(Encoding::protobuf().with_schema("std_msgs/msg/String"))
        .build()?;

    let pub_cdr = node
        .create_pub::<RosString>("/demo/cdr")
        .with_encoding(Encoding::cdr())
        .build()?;

    // Create subscribers with encoding validation
    let sub_proto = node
        .create_sub::<RosString>("/demo/protobuf")
        .with_encoding(Encoding::protobuf().with_schema("std_msgs/msg/String"))
        .build()?;

    let sub_cdr = node
        .create_sub::<RosString>("/demo/cdr")
        .with_encoding(Encoding::cdr())
        .build()?;

    println!("✓ Publishers and subscribers ready\n");
    println!("Publishing and receiving messages...\n");

    let mut counter = 0;
    loop {
        counter += 1;

        // Publish Protobuf
        let msg_proto = RosString {
            data: format!("Interop test #{}", counter),
        };
        pub_proto.publish(&msg_proto)?;

        // Publish CDR
        let msg_cdr = RosString {
            data: format!("CDR test #{}", counter),
        };
        pub_cdr.publish(&msg_cdr)?;

        std::thread::sleep(Duration::from_millis(200));

        // Receive and validate
        if let Ok(msg) = sub_proto.recv() {
            println!("✓ Protobuf: '{}'", msg.data);
        }

        if let Ok(msg) = sub_cdr.recv() {
            println!("✓ CDR:      '{}'", msg.data);
        }

        if counter >= 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(800));
    }

    println!("\n=== Demo Complete ===");
    println!("\nKey Takeaways:");
    println!("  • Encoding metadata is transmitted automatically");
    println!("  • Subscribers can validate encoding at runtime");
    println!("  • External Zenoh apps can detect format via sample.encoding()");
    println!("  • Zero overhead - encoding cached during build()");

    Ok(())
}
