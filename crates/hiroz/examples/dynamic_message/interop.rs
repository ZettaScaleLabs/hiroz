//! Example of interoperability between static and dynamic messages.
//!
//! This example demonstrates:
//! - Converting static ROS messages to dynamic messages via CDR
//! - Converting dynamic messages back to static messages
//! - Both produce identical CDR bytes, enabling seamless interop

use hiroz::{
    dynamic::{DynamicMessage, FieldType, MessageSchema},
    msg::ZMessage,
};
use hiroz_msgs::{
    geometry_msgs::{Point, Twist, Vector3},
    std_msgs::String as StdString,
};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Static/Dynamic Message Interop Example ===\n");

    // Create schemas that match the static message types
    let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    let string_schema = MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()?;

    let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3_schema.clone()))
        .field("angular", FieldType::Message(vector3_schema))
        .build()?;

    // Example 1: Static → Dynamic (Point)
    println!("--- Example 1: Static Point → Dynamic ---");
    let static_point = Point {
        x: 1.0,
        y: 2.0,
        z: 3.0,
    };
    println!(
        "Static Point: x={}, y={}, z={}",
        static_point.x, static_point.y, static_point.z
    );

    // Serialize static to CDR, then deserialize as dynamic
    let cdr_bytes = static_point.serialize();
    let dynamic_point = DynamicMessage::from_cdr(&cdr_bytes, &point_schema)?;
    println!(
        "Dynamic Point: x={}, y={}, z={}\n",
        dynamic_point.get::<f64>("x")?,
        dynamic_point.get::<f64>("y")?,
        dynamic_point.get::<f64>("z")?
    );

    // Example 2: Dynamic → Static (Point)
    println!("--- Example 2: Dynamic Point → Static ---");
    let mut dynamic_point = DynamicMessage::new(&point_schema);
    dynamic_point.set("x", 10.0f64)?;
    dynamic_point.set("y", 20.0f64)?;
    dynamic_point.set("z", 30.0f64)?;
    println!(
        "Dynamic Point: x={}, y={}, z={}",
        dynamic_point.get::<f64>("x")?,
        dynamic_point.get::<f64>("y")?,
        dynamic_point.get::<f64>("z")?
    );

    // Serialize dynamic to CDR, then deserialize as static
    let cdr_bytes = dynamic_point.to_cdr()?;
    let static_point = Point::deserialize(&cdr_bytes)?;
    println!(
        "Static Point: x={}, y={}, z={}\n",
        static_point.x, static_point.y, static_point.z
    );

    // Example 3: String message interop
    println!("--- Example 3: String Message Interop ---");
    let static_string = StdString {
        data: "Hello from static message!".to_string(),
    };
    println!("Static String: \"{}\"", static_string.data);

    let cdr_bytes = static_string.serialize();
    let dynamic_string = DynamicMessage::from_cdr(&cdr_bytes, &string_schema)?;
    println!(
        "Dynamic String: \"{}\"\n",
        dynamic_string.get::<String>("data")?
    );

    // Example 4: Nested message (Twist) interop
    println!("--- Example 4: Nested Twist Message Interop ---");
    let static_twist = Twist {
        linear: Vector3 {
            x: 0.5,
            y: 0.0,
            z: 0.0,
        },
        angular: Vector3 {
            x: 0.0,
            y: 0.0,
            z: 0.3,
        },
    };
    println!("Static Twist:");
    println!(
        "  linear:  ({}, {}, {})",
        static_twist.linear.x, static_twist.linear.y, static_twist.linear.z
    );
    println!(
        "  angular: ({}, {}, {})",
        static_twist.angular.x, static_twist.angular.y, static_twist.angular.z
    );

    let cdr_bytes = static_twist.serialize();
    let dynamic_twist = DynamicMessage::from_cdr(&cdr_bytes, &twist_schema)?;
    println!("Dynamic Twist:");
    println!(
        "  linear:  ({}, {}, {})",
        dynamic_twist.get::<f64>("linear.x")?,
        dynamic_twist.get::<f64>("linear.y")?,
        dynamic_twist.get::<f64>("linear.z")?
    );
    println!(
        "  angular: ({}, {}, {})\n",
        dynamic_twist.get::<f64>("angular.x")?,
        dynamic_twist.get::<f64>("angular.y")?,
        dynamic_twist.get::<f64>("angular.z")?
    );

    // Example 5: Verify CDR bytes are identical
    println!("--- Example 5: CDR Byte Verification ---");
    let static_msg = Point {
        x: 1.5,
        y: 2.5,
        z: 3.5,
    };
    let mut dynamic_msg = DynamicMessage::new(&point_schema);
    dynamic_msg.set("x", 1.5f64)?;
    dynamic_msg.set("y", 2.5f64)?;
    dynamic_msg.set("z", 3.5f64)?;

    let static_cdr = static_msg.serialize();
    let dynamic_cdr = dynamic_msg.to_cdr()?;

    println!("Static CDR:  {} bytes", static_cdr.len());
    println!("Dynamic CDR: {} bytes", dynamic_cdr.len());
    println!(
        "Bytes identical: {}\n",
        if static_cdr == dynamic_cdr {
            "YES"
        } else {
            "NO"
        }
    );

    // Example 6: Full roundtrip
    println!("--- Example 6: Full Roundtrip ---");
    let original = Point {
        x: 1.5,
        y: 2.5,
        z: 3.5,
    };
    println!(
        "Original static: ({}, {}, {})",
        original.x, original.y, original.z
    );

    // Static → CDR → Dynamic → CDR → Static
    let cdr1 = original.serialize();
    let dynamic = DynamicMessage::from_cdr(&cdr1, &point_schema)?;
    let cdr2 = dynamic.to_cdr()?;
    let recovered = Point::deserialize(&cdr2)?;

    println!(
        "Recovered static: ({}, {}, {})",
        recovered.x, recovered.y, recovered.z
    );
    println!(
        "Roundtrip successful: {}",
        if original.x == recovered.x && original.y == recovered.y && original.z == recovered.z {
            "YES"
        } else {
            "NO"
        }
    );

    println!("\n=== Example Complete ===");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}
