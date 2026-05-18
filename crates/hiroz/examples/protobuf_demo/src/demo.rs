use clap::Parser;
use hiroz::{Builder, Result, context::ZContextBuilder};
use protobuf_demo::{run_pubsub_demo, run_service_client, run_service_server};

#[derive(Debug, Parser)]
#[command(
    name = "protobuf_demo",
    about = "Protobuf demonstration for hiroz - pub/sub and services"
)]
struct Args {
    /// Mode to run: pubsub, service-server, service-client, or combined
    #[arg(short, long, default_value = "pubsub")]
    mode: String,

    /// Service name (for service modes)
    #[arg(long, default_value = "/calculator")]
    service: String,

    /// Maximum number of messages/requests (0 for unlimited)
    #[arg(short = 'n', long, default_value = "3")]
    count: usize,

    /// Zenoh session mode (peer, client, router)
    #[arg(long, default_value = "peer")]
    zenoh_mode: String,

    /// Zenoh router endpoint to connect to
    #[arg(short, long)]
    endpoint: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    zenoh::init_log_from_env_or("info");

    // Create the hiroz context
    let ctx = if let Some(ref e) = args.endpoint {
        ZContextBuilder::default()
            .with_mode(&args.zenoh_mode)
            .with_connect_endpoints([e.clone()])
            .build()?
    } else {
        ZContextBuilder::default()
            .with_mode(&args.zenoh_mode)
            .build()?
    };

    let max_count = if args.count == 0 {
        None
    } else {
        Some(args.count)
    };

    match args.mode.as_str() {
        "pubsub" => {
            run_pubsub_demo(ctx, max_count)?;
        }
        "service-server" => {
            run_service_server(ctx, &args.service, max_count)?;
        }
        "service-client" => {
            let operations = vec![
                ("add", 10.0, 5.0),
                ("subtract", 10.0, 5.0),
                ("multiply", 10.0, 5.0),
                ("divide", 10.0, 5.0),
                ("divide", 10.0, 0.0), // This will fail
            ];
            run_service_client(ctx, &args.service, operations)?;
        }
        "combined" => {
            println!("\n=== Running Combined Demo ===\n");

            // Create separate contexts for server and client
            let server_ctx = if let Some(e) = args.endpoint.clone() {
                ZContextBuilder::default()
                    .with_mode(&args.zenoh_mode)
                    .with_connect_endpoints([e])
                    .build()?
            } else {
                ZContextBuilder::default()
                    .with_mode(&args.zenoh_mode)
                    .build()?
            };

            let service_name = args.service.clone();

            // Run server in background thread
            let _server_handle =
                std::thread::spawn(move || run_service_server(server_ctx, &service_name, Some(5)));

            // Give server time to start
            std::thread::sleep(std::time::Duration::from_millis(500));

            // Run client
            let operations = vec![
                ("add", 10.0, 5.0),
                ("subtract", 10.0, 5.0),
                ("multiply", 10.0, 5.0),
                ("divide", 10.0, 5.0),
                ("divide", 10.0, 0.0),
            ];
            run_service_client(ctx, &args.service, operations)?;
        }
        _ => {
            eprintln!("Unknown mode: {}", args.mode);
            eprintln!("Valid modes: pubsub, service-server, service-client, combined");
            std::process::exit(1);
        }
    }

    Ok(())
}
