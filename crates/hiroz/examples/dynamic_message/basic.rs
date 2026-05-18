//! Basic example of dynamic message handling.
//!
//! This example demonstrates:
//! - Creating message schemas at runtime
//! - Creating and manipulating dynamic messages
//! - Nested message support with dot notation
//! - Using the builder pattern for messages

use hiroz::dynamic::{DynamicMessage, FieldType, MessageSchema};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Dynamic Message Basic Example ===\n");

    // Create a simple Point schema
    let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    println!("Created schema: {}", point_schema.type_name);
    println!(
        "Fields: {:?}\n",
        point_schema.field_names().collect::<Vec<_>>()
    );

    // Create a message with default values
    let mut point = DynamicMessage::new(&point_schema);
    println!(
        "Default point: x={}, y={}, z={}",
        point.get::<f64>("x")?,
        point.get::<f64>("y")?,
        point.get::<f64>("z")?
    );

    // Set values
    point.set("x", 1.0f64)?;
    point.set("y", 2.0f64)?;
    point.set("z", 3.0f64)?;
    println!(
        "Modified point: x={}, y={}, z={}\n",
        point.get::<f64>("x")?,
        point.get::<f64>("y")?,
        point.get::<f64>("z")?
    );

    // Create a nested message schema (Twist with linear and angular Vector3)
    let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3_schema.clone()))
        .field("angular", FieldType::Message(vector3_schema))
        .build()?;

    println!("Created nested schema: {}", twist_schema.type_name);

    // Create a Twist message and use dot notation for nested access
    let mut twist = DynamicMessage::new(&twist_schema);
    twist.set("linear.x", 0.5f64)?;
    twist.set("linear.y", 0.0f64)?;
    twist.set("linear.z", 0.0f64)?;
    twist.set("angular.x", 0.0f64)?;
    twist.set("angular.y", 0.0f64)?;
    twist.set("angular.z", 0.3f64)?;

    println!("Twist message:");
    println!(
        "  linear: x={}, y={}, z={}",
        twist.get::<f64>("linear.x")?,
        twist.get::<f64>("linear.y")?,
        twist.get::<f64>("linear.z")?
    );
    println!(
        "  angular: x={}, y={}, z={}\n",
        twist.get::<f64>("angular.x")?,
        twist.get::<f64>("angular.y")?,
        twist.get::<f64>("angular.z")?
    );

    // Using the builder pattern
    let msg = DynamicMessage::builder(&point_schema)
        .set("x", 10.0f64)?
        .set("y", 20.0f64)?
        .set("z", 30.0f64)?
        .build();

    println!(
        "Built with builder: x={}, y={}, z={}",
        msg.get::<f64>("x")?,
        msg.get::<f64>("y")?,
        msg.get::<f64>("z")?
    );

    // Iterate over fields
    println!("\nIterating over point fields:");
    for (name, value) in msg.iter() {
        println!("  {} = {:?}", name, value);
    }

    println!("\n=== Example Complete ===");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}
