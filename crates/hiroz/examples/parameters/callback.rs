mod common;

use clap::Parser;
use hiroz::{
    Builder, Result,
    context::ZContext,
    parameter::{
        FloatingPointRange, Parameter, ParameterDescriptor, ParameterType, ParameterValue,
        SetParametersResult,
    },
};

#[derive(Parser)]
#[command(about = "Parameter validation callback demo")]
struct Args {
    /// Zenoh router endpoint (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}

// ANCHOR: callback_snippet
fn run(ctx: ZContext) -> Result<()> {
    println!("\n=== Validation Callback Demo ===\n");

    let node = ctx.create_node("callback_demo").build()?;

    let mut desc = ParameterDescriptor::new("temperature", ParameterType::Double);
    desc.floating_point_range = Some(FloatingPointRange {
        from_value: -40.0,
        to_value: 85.0,
        step: 0.0,
    });

    node.declare_parameter("temperature", ParameterValue::Double(20.0), desc)
        .expect("declare temperature");

    node.on_set_parameters(|params| {
        for p in params {
            if let ParameterValue::Double(v) = &p.value
                && *v > 50.0
            {
                return SetParametersResult::failure(format!(
                    "{} = {} exceeds safety limit 50.0",
                    p.name, v
                ));
            }
        }
        SetParametersResult::success()
    });

    node.set_parameter(Parameter::new("temperature", ParameterValue::Double(25.0)))
        .expect("set to 25.0");
    println!("temperature = {:?}", node.get_parameter("temperature"));

    let err = node
        .set_parameter(Parameter::new("temperature", ParameterValue::Double(60.0)))
        .unwrap_err();
    println!("Callback rejected: {}", err);

    println!(
        "temperature unchanged = {:?}",
        node.get_parameter("temperature")
    );

    Ok(())
}
// ANCHOR_END: callback_snippet

fn main() -> Result<()> {
    common::init();
    let args = Args::parse();
    let ctx = common::create_context(args.endpoint)?;
    run(ctx)
}
