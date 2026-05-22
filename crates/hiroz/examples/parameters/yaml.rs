mod common;

use std::collections::HashMap;

use clap::Parser;
use hiroz::{
    Builder, Result,
    parameter::{ParameterDescriptor, ParameterType, ParameterValue},
};

#[derive(Parser)]
#[command(about = "YAML parameter file loading demo")]
struct Args {
    /// Zenoh router endpoint (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}

fn main() -> Result<()> {
    common::init();
    let args = Args::parse();
    let ctx = common::create_context(args.endpoint)?;

    println!("\n=== YAML Parameter Loading Demo ===\n");

    let yaml = r#"
/**:
  ros__parameters:
    global_timeout: 5.0
    debug: true

/yaml_demo:
  ros__parameters:
    sensor_rate: 100
    device_name: "lidar_front"
"#;

    let path = std::env::temp_dir().join("hiroz_param_demo.yaml");
    std::fs::write(&path, yaml).expect("write yaml");
    println!("YAML written to {}", path.display());

    let node = ctx
        .create_node("yaml_demo")
        .with_parameter_file(&path)
        .expect("parse yaml")
        .build()?;

    for (name, ty, default) in [
        (
            "global_timeout",
            ParameterType::Double,
            ParameterValue::Double(1.0),
        ),
        ("debug", ParameterType::Bool, ParameterValue::Bool(false)),
        (
            "sensor_rate",
            ParameterType::Integer,
            ParameterValue::Integer(10),
        ),
        (
            "device_name",
            ParameterType::String,
            ParameterValue::String("unknown".into()),
        ),
    ] {
        let desc = ParameterDescriptor::new(name, ty);
        let value = node
            .declare_parameter(name, default, desc)
            .expect("declare");
        println!("{} = {:?}", name, value);
    }

    println!("\n--- Programmatic overrides ---");

    let mut overrides = HashMap::new();
    overrides.insert("count".to_string(), ParameterValue::Integer(99));

    let node2 = ctx
        .create_node("override_demo")
        .with_parameter_overrides(overrides)
        .build()?;

    let desc = ParameterDescriptor::new("count", ParameterType::Integer);
    let value = node2
        .declare_parameter("count", ParameterValue::Integer(0), desc)
        .expect("declare");
    println!("count = {:?} (default was 0, override is 99)", value);

    Ok(())
}
