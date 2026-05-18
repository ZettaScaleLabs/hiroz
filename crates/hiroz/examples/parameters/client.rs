mod common;

use std::{sync::Arc, time::Duration};

use clap::Parser;
use hiroz::{
    Builder, Result,
    parameter::{
        Parameter, ParameterClient, ParameterDescriptor, ParameterTarget, ParameterType,
        ParameterValue,
    },
};

#[derive(Parser)]
#[command(about = "Remote parameter client demo")]
struct Args {
    /// Zenoh router endpoint (e.g., tcp/localhost:7447)
    #[arg(short, long)]
    endpoint: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init();
    let args = Args::parse();
    let ctx = common::create_context(args.endpoint)?;

    println!("\n=== Remote Parameter Client Demo ===\n");

    let server = ctx.create_node("client_demo_server").build()?;
    let desc = ParameterDescriptor::new("max_speed", ParameterType::Integer);
    server
        .declare_parameter("max_speed", ParameterValue::Integer(50), desc)
        .expect("declare max_speed");

    let client_node = Arc::new(ctx.create_node("client_demo").build()?);
    let client = ParameterClient::new(
        client_node,
        ParameterTarget::from_fqn("/client_demo_server").expect("valid node name"),
    )?;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let values = client.get(&["max_speed"]).await?;
    let types = client.get_types(&["max_speed"]).await?;
    let result = client
        .set_atomically(&[Parameter::new("max_speed", ParameterValue::Integer(80))])
        .await?;

    println!("remote values = {:?}", values);
    println!("remote types = {:?}", types);
    println!("remote atomic set = {:?}", result);
    println!(
        "max_speed after remote set = {:?}",
        server.get_parameter("max_speed")
    );

    Ok(())
}
