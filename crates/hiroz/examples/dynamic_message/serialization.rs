//! Example of CDR serialization with dynamic messages.
//!
//! This example demonstrates:
//! - Serializing dynamic messages to CDR format
//! - Deserializing CDR data back to dynamic messages
//! - Working with arrays and sequences
//! - Round-trip serialization verification

use hiroz::dynamic::{DynamicMessage, DynamicValue, FieldType, MessageSchema};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Dynamic Message Serialization Example ===\n");

    // Example 1: Simple message serialization
    println!("--- Example 1: Point message round-trip ---");
    let point_schema = MessageSchema::builder("geometry_msgs/msg/Point")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    let mut point = DynamicMessage::new(&point_schema);
    point.set("x", 1.5f64)?;
    point.set("y", 2.5f64)?;
    point.set("z", 3.5f64)?;

    // Serialize to CDR
    let cdr_bytes = point.to_cdr()?;
    println!(
        "Original: x={}, y={}, z={}",
        point.get::<f64>("x")?,
        point.get::<f64>("y")?,
        point.get::<f64>("z")?
    );
    println!("CDR bytes: {} bytes", cdr_bytes.len());
    println!(
        "CDR header: {:02x} {:02x} {:02x} {:02x}",
        cdr_bytes[0], cdr_bytes[1], cdr_bytes[2], cdr_bytes[3]
    );

    // Deserialize from CDR
    let decoded = DynamicMessage::from_cdr(&cdr_bytes, &point_schema)?;
    println!(
        "Decoded:  x={}, y={}, z={}\n",
        decoded.get::<f64>("x")?,
        decoded.get::<f64>("y")?,
        decoded.get::<f64>("z")?
    );

    // Example 2: String message
    println!("--- Example 2: String message round-trip ---");
    let string_schema = MessageSchema::builder("std_msgs/msg/String")
        .field("data", FieldType::String)
        .build()?;

    let mut string_msg = DynamicMessage::new(&string_schema);
    string_msg.set("data", "Hello, hiroz Dynamic Messages!")?;

    let cdr_bytes = string_msg.to_cdr()?;
    println!("Original: \"{}\"", string_msg.get::<String>("data")?);
    println!("CDR bytes: {} bytes", cdr_bytes.len());

    let decoded = DynamicMessage::from_cdr(&cdr_bytes, &string_schema)?;
    println!("Decoded:  \"{}\"\n", decoded.get::<String>("data")?);

    // Example 3: Array/Sequence message
    println!("--- Example 3: Sequence message round-trip ---");
    let array_schema = MessageSchema::builder("test_msgs/msg/FloatArray")
        .field("data", FieldType::Sequence(Box::new(FieldType::Float32)))
        .build()?;

    let mut array_msg = DynamicMessage::new(&array_schema);
    array_msg.set("data", vec![1.0f32, 2.0, 3.0, 4.0, 5.0])?;

    let cdr_bytes = array_msg.to_cdr()?;
    println!(
        "Original array: {:?}",
        match array_msg.get_dynamic("data")? {
            DynamicValue::Array(arr) => arr.iter().filter_map(|v| v.as_f32()).collect::<Vec<_>>(),
            _ => vec![],
        }
    );
    println!("CDR bytes: {} bytes", cdr_bytes.len());

    let decoded = DynamicMessage::from_cdr(&cdr_bytes, &array_schema)?;
    println!(
        "Decoded array: {:?}\n",
        match decoded.get_dynamic("data")? {
            DynamicValue::Array(arr) => arr.iter().filter_map(|v| v.as_f32()).collect::<Vec<_>>(),
            _ => vec![],
        }
    );

    // Example 4: Nested message
    println!("--- Example 4: Nested message round-trip ---");
    let vector3_schema = MessageSchema::builder("geometry_msgs/msg/Vector3")
        .field("x", FieldType::Float64)
        .field("y", FieldType::Float64)
        .field("z", FieldType::Float64)
        .build()?;

    let twist_schema = MessageSchema::builder("geometry_msgs/msg/Twist")
        .field("linear", FieldType::Message(vector3_schema.clone()))
        .field("angular", FieldType::Message(vector3_schema))
        .build()?;

    let mut twist = DynamicMessage::new(&twist_schema);
    twist.set("linear.x", 1.0f64)?;
    twist.set("linear.y", 0.0f64)?;
    twist.set("linear.z", 0.0f64)?;
    twist.set("angular.x", 0.0f64)?;
    twist.set("angular.y", 0.0f64)?;
    twist.set("angular.z", 0.5f64)?;

    let cdr_bytes = twist.to_cdr()?;
    println!("Original Twist:");
    println!(
        "  linear:  ({}, {}, {})",
        twist.get::<f64>("linear.x")?,
        twist.get::<f64>("linear.y")?,
        twist.get::<f64>("linear.z")?
    );
    println!(
        "  angular: ({}, {}, {})",
        twist.get::<f64>("angular.x")?,
        twist.get::<f64>("angular.y")?,
        twist.get::<f64>("angular.z")?
    );
    println!("CDR bytes: {} bytes", cdr_bytes.len());

    let decoded = DynamicMessage::from_cdr(&cdr_bytes, &twist_schema)?;
    println!("Decoded Twist:");
    println!(
        "  linear:  ({}, {}, {})",
        decoded.get::<f64>("linear.x")?,
        decoded.get::<f64>("linear.y")?,
        decoded.get::<f64>("linear.z")?
    );
    println!(
        "  angular: ({}, {}, {})",
        decoded.get::<f64>("angular.x")?,
        decoded.get::<f64>("angular.y")?,
        decoded.get::<f64>("angular.z")?
    );

    // Example 5: All primitive types
    println!("\n--- Example 5: All primitive types ---");
    let all_types_schema = MessageSchema::builder("test_msgs/msg/AllTypes")
        .field("bool_val", FieldType::Bool)
        .field("int8_val", FieldType::Int8)
        .field("int16_val", FieldType::Int16)
        .field("int32_val", FieldType::Int32)
        .field("int64_val", FieldType::Int64)
        .field("uint8_val", FieldType::Uint8)
        .field("uint16_val", FieldType::Uint16)
        .field("uint32_val", FieldType::Uint32)
        .field("uint64_val", FieldType::Uint64)
        .field("float32_val", FieldType::Float32)
        .field("float64_val", FieldType::Float64)
        .field("string_val", FieldType::String)
        .build()?;

    let mut msg = DynamicMessage::new(&all_types_schema);
    msg.set("bool_val", true)?;
    msg.set("int8_val", -42i8)?;
    msg.set("int16_val", -1000i16)?;
    msg.set("int32_val", -100000i32)?;
    msg.set("int64_val", -10000000000i64)?;
    msg.set("uint8_val", 200u8)?;
    msg.set("uint16_val", 50000u16)?;
    msg.set("uint32_val", 3000000000u32)?;
    msg.set("uint64_val", 10000000000u64)?;
    msg.set("float32_val", 1.5f32)?;
    msg.set("float64_val", 9.87654321f64)?;
    msg.set("string_val", "test")?;

    let cdr_bytes = msg.to_cdr()?;
    println!("All types message CDR: {} bytes", cdr_bytes.len());

    let decoded = DynamicMessage::from_cdr(&cdr_bytes, &all_types_schema)?;
    println!("Round-trip verification:");
    println!(
        "  bool:    {} == {}",
        msg.get::<bool>("bool_val")?,
        decoded.get::<bool>("bool_val")?
    );
    println!(
        "  int8:    {} == {}",
        msg.get::<i8>("int8_val")?,
        decoded.get::<i8>("int8_val")?
    );
    println!(
        "  int32:   {} == {}",
        msg.get::<i32>("int32_val")?,
        decoded.get::<i32>("int32_val")?
    );
    println!(
        "  float64: {} == {}",
        msg.get::<f64>("float64_val")?,
        decoded.get::<f64>("float64_val")?
    );
    println!(
        "  string:  \"{}\" == \"{}\"",
        msg.get::<String>("string_val")?,
        decoded.get::<String>("string_val")?
    );

    println!("\n=== Example Complete ===");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}
