mod common;

use clap::Parser;
use hiroz::{
    Builder, Result,
    parameter::{Parameter, ParameterDescriptor, ParameterType, ParameterValue},
};

#[derive(Parser)]
#[command(about = "Parameter declaration and lifecycle demo")]
struct Args {
    /// Zenoh router endpoint (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}

// ANCHOR: full_example
#[tokio::main]
async fn main() -> Result<()> {
    common::init();
    let args = Args::parse();
    let ctx = common::create_context(args.endpoint)?;

    println!("\n=== Parameter Declaration Demo ===\n");

    let node = ctx.create_node("declare_demo").build()?;

    let mut desc = ParameterDescriptor::new("max_speed", ParameterType::Integer);
    desc.integer_range = Some(hiroz::parameter::IntegerRange {
        from_value: 0,
        to_value: 100,
        step: 1,
    });
    desc.description = "Maximum speed in m/s".to_string();

    let initial = node
        .declare_parameter("max_speed", ParameterValue::Integer(50), desc)
        .expect("declare max_speed");
    println!("Declared max_speed = {:?}", initial);

    let desc = ParameterDescriptor::new("robot_name", ParameterType::String);
    node.declare_parameter("robot_name", ParameterValue::String("rosbot".into()), desc)
        .expect("declare robot_name");

    println!("max_speed = {:?}", node.get_parameter("max_speed"));
    println!("robot_name = {:?}", node.get_parameter("robot_name"));

    node.set_parameter(Parameter::new("max_speed", ParameterValue::Integer(75)))
        .expect("set max_speed");
    println!(
        "max_speed after set = {:?}",
        node.get_parameter("max_speed")
    );

    let err = node
        .set_parameter(Parameter::new(
            "max_speed",
            ParameterValue::String("fast".into()),
        ))
        .unwrap_err();
    println!("Type mismatch rejected: {}", err);

    node.undeclare_parameter("robot_name")
        .expect("undeclare robot_name");
    println!(
        "robot_name after undeclare: {:?}",
        node.get_parameter("robot_name")
    );

    Ok(())
}
// ANCHOR_END: full_example
