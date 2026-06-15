#[cfg(not(test))]
use clap::Parser;
use hiroz::{
    Builder, Result,
    context::{ZContext, ZContextBuilder},
};
use hiroz_msgs::example_interfaces::{AddTwoIntsRequest, AddTwoIntsResponse, srv::AddTwoInts};

#[cfg(not(test))]
#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "server", help = "Mode: server or client")]
    mode: String,

    #[arg(short, long, default_value = "1", help = "First number (client mode)")]
    a: i64,

    #[arg(short, long, default_value = "2", help = "Second number (client mode)")]
    b: i64,

    /// Zenoh session mode: peer or client
    #[arg(long, default_value = "peer")]
    zenoh_mode: String,

    /// Connect endpoint (e.g. tcp/127.0.0.1:7447); enables client mode when set
    #[arg(long)]
    endpoint: Option<String>,
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let format = hiroz_protocol::KeyExprFormat::RmwZenoh;

    let ctx = if let Some(ref ep) = args.endpoint {
        ZContextBuilder::default()
            .with_mode(args.zenoh_mode.clone())
            .with_connect_endpoints([ep.as_str()])
            .keyexpr_format(format)
            .build()?
    } else {
        ZContextBuilder::default()
            .with_mode(args.zenoh_mode.clone())
            .keyexpr_format(format)
            .build()?
    };

    match args.mode.as_str() {
        "server" => run_server(ctx),
        "client" => run_client(ctx, args.a, args.b).await,
        mode => {
            eprintln!("Invalid mode: {}. Use 'server' or 'client'", mode);
            std::process::exit(1);
        }
    }
}

pub fn run_server(ctx: ZContext) -> Result<()> {
    let node = ctx.create_node("add_two_ints_server").build()?;
    let mut zsrv = node.create_service::<AddTwoInts>("add_two_ints").build()?;

    println!("AddTwoInts service server started, waiting for requests...");

    loop {
        let req = zsrv.take_request()?;
        println!(
            "Received request: {} + {}",
            req.message().a,
            req.message().b
        );

        let resp = AddTwoIntsResponse {
            sum: req.message().a + req.message().b,
        };

        println!("Sending response: {}", resp.sum);
        req.reply_blocking(&resp)?;
    }
}

pub async fn run_client(ctx: ZContext, a: i64, b: i64) -> Result<()> {
    let node = ctx.create_node("add_two_ints_client").build()?;
    let zcli = node.create_client::<AddTwoInts>("add_two_ints").build()?;

    println!("AddTwoInts service client started");

    let req = AddTwoIntsRequest { a, b };
    println!("Sending request: {} + {}", req.a, req.b);

    let resp = zcli
        .call_with_timeout(&req, std::time::Duration::from_secs(5))
        .await?;

    println!("Received response: {}", resp.sum);

    Ok(())
}
