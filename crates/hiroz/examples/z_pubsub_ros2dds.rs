//! ros2dds pub/sub example
//!
//! This example demonstrates hiroz pub/sub using the ros2dds backend,
//! which is compatible with zenoh-plugin-ros2dds for interoperability
//! with standard ROS 2 nodes.
//!
//! # Usage
//!
//! ## Direct hiroz to hiroz communication
//!
//! ```bash
//! # Terminal 1: Run listener
//! cargo run --example z_pubsub_ros2dds --features ros2dds -- --role listener
//!
//! # Terminal 2: Run talker
//! cargo run --example z_pubsub_ros2dds --features ros2dds -- --role talker
//! ```
//!
//! ## Interop with ROS 2 via zenoh-bridge-ros2dds
//!
//! ```bash
//! # Terminal 1: Run zenoh-bridge-ros2dds
//! zenoh-bridge-ros2dds
//!
//! # Terminal 2: Run ROS 2 talker
//! ros2 run demo_nodes_cpp talker
//!
//! # Terminal 3: Run hiroz listener (ros2dds backend)
//! cargo run --example z_pubsub_ros2dds --features ros2dds -- --role listener --topic chatter
//! ```

use std::time::Duration;

use clap::Parser;
use hiroz::{
    TypeHash,
    entity::{NodeEntity, TypeInfo},
};
use hiroz_cdr::{CdrSerializer, LittleEndian};
use hiroz_protocol::entity::{EndpointEntity, EndpointKind};
use serde::{Deserialize, Serialize};
use zenoh::{Wait, key_expr::KeyExpr};

/// CDR encapsulation header for little-endian encoding
const CDR_HEADER_LE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

/// Simple string message compatible with std_msgs/msg/String
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RosString {
    data: String,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "Hello from ros2dds backend!")]
    data: String,
    #[arg(short, long, default_value = "chatter")]
    topic: String,
    #[arg(short, long, default_value = "1.0")]
    period: f64,
    #[arg(short, long, default_value = "listener")]
    role: String,
    #[arg(short, long, default_value = "peer")]
    mode: String,
    #[arg(short, long)]
    endpoint: Option<String>,
}

#[tokio::main]
async fn main() -> hiroz::Result<()> {
    zenoh::init_log_from_env_or("info");
    let args = Args::parse();

    // Build Zenoh config
    let mut config = zenoh::Config::default();
    if args.mode == "client" {
        config
            .set_mode(Some(zenoh::config::WhatAmI::Client))
            .unwrap();
    }
    if let Some(ref endpoint) = args.endpoint {
        config
            .connect
            .endpoints
            .set(vec![endpoint.parse().unwrap()])
            .unwrap();
    }

    // Open Zenoh session
    let session = zenoh::open(config).wait()?;
    let zid = session.zid();
    println!("=== ros2dds Backend Demo ===");
    println!("Zenoh ID: {zid}");

    // Create entity for key expression generation
    let node = NodeEntity::new(
        0, // domain_id
        zid,
        1, // node_id
        "ros2dds_demo".to_string(),
        "/".to_string(),
        "/".to_string(), // enclave
    );

    // Topic name - ros2dds uses simple format without leading slash in KE
    let topic = if args.topic.starts_with('/') {
        args.topic[1..].to_string()
    } else {
        args.topic.clone()
    };

    // Type info for std_msgs/msg/String
    let type_info = TypeInfo::new("std_msgs/msg/String", TypeHash::zero());

    // QoS profile - use Reliable/Volatile for standard compatibility
    let qos = hiroz_protocol::qos::QosProfile {
        reliability: hiroz_protocol::qos::QosReliability::Reliable,
        durability: hiroz_protocol::qos::QosDurability::Volatile,
        history: hiroz_protocol::qos::QosHistory::KeepLast(10),
    };

    let entity_kind = match args.role.as_str() {
        "talker" => EndpointKind::Publisher,
        _ => EndpointKind::Subscription,
    };

    let entity = EndpointEntity {
        id: 1,
        node: Some(node),
        kind: entity_kind,
        topic: topic.clone(),
        type_info: Some(type_info),
        qos,
    };

    // Generate key expressions using ros2dds format
    let format = hiroz_protocol::KeyExprFormat::Ros2Dds;
    let topic_ke = format.topic_key_expr(&entity)?;
    let liveliness_ke = format.liveliness_key_expr(&entity, &zid)?;

    println!("Role:           {}", args.role);
    println!("Topic:          {}", topic);
    println!("Topic KE:       {}", topic_ke);
    println!("Liveliness KE:  {}", liveliness_ke.as_str());
    println!();

    match args.role.as_str() {
        "talker" => run_talker(&session, &topic_ke, &liveliness_ke, &args).await,
        "listener" => run_listener(&session, &topic_ke, &liveliness_ke).await,
        _ => {
            println!("Unknown role: {}. Use 'talker' or 'listener'.", args.role);
            Ok(())
        }
    }
}

async fn run_talker(
    session: &zenoh::Session,
    topic_ke: &KeyExpr<'static>,
    liveliness_ke: &KeyExpr<'static>,
    args: &Args,
) -> hiroz::Result<()> {
    println!("Publishing messages...\n");

    // Declare liveliness token for discovery
    let _token = session.liveliness().declare_token(liveliness_ke).wait()?;

    // Create publisher
    let publisher = session.declare_publisher(topic_ke).wait()?;

    let period = Duration::from_secs_f64(args.period);
    let mut count = 0u64;

    loop {
        let msg = RosString {
            data: format!("{} - #{count}", args.data),
        };

        // Serialize to CDR format with header
        let mut cdr_bytes = Vec::with_capacity(64);
        cdr_bytes.extend_from_slice(&CDR_HEADER_LE);
        let mut serializer = CdrSerializer::<LittleEndian>::new(&mut cdr_bytes);
        serde::Serialize::serialize(&msg, &mut serializer).unwrap();

        println!("Publishing: {}", msg.data);
        publisher.put(&cdr_bytes).await?;

        count += 1;
        tokio::time::sleep(period).await;
    }
}

async fn run_listener(
    session: &zenoh::Session,
    topic_ke: &KeyExpr<'static>,
    liveliness_ke: &KeyExpr<'static>,
) -> hiroz::Result<()> {
    println!("Waiting for messages...\n");

    // Declare liveliness token for discovery
    let _token = session.liveliness().declare_token(liveliness_ke).wait()?;

    // Create subscriber
    let subscriber = session.declare_subscriber(topic_ke).wait()?;

    loop {
        let sample = subscriber.recv_async().await?;
        let payload = sample.payload().to_bytes();

        // Skip CDR header (4 bytes) and deserialize
        if payload.len() < 4 {
            println!("Payload too short: {} bytes", payload.len());
            continue;
        }
        match hiroz_cdr::from_bytes::<RosString, LittleEndian>(&payload[4..]) {
            Ok((msg, _bytes_consumed)) => println!("Received: {}", msg.data),
            Err(e) => println!("Failed to deserialize: {e}"),
        }
    }
}
