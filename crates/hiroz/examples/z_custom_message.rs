use std::time::Duration;

#[cfg(not(test))]
use clap::Parser;
use hiroz::{
    Builder, MessageTypeInfo, Result, ServiceTypeInfo, context::ZContextBuilder, entity::TypeHash,
    msg::ZService,
};
use serde::{Deserialize, Serialize};

// Custom message for pub/sub example
#[derive(Debug, Clone, Serialize, Deserialize, Default, MessageTypeInfo)]
#[ros_msg(type_name = "custom_msgs/msg/RobotStatus")]
pub struct RobotStatus {
    pub robot_id: String,
    pub battery_percentage: f64,
    pub position_x: f64,
    pub position_y: f64,
    pub is_moving: bool,
}

impl hiroz::msg::ZMessage for RobotStatus {
    type Serdes = hiroz::msg::SerdeCdrSerdes<RobotStatus>;
}

// Custom service request/response: use the `_Request`/`_Response` suffix in type_name.
// There is no "action" kind — derive each action sub-message (Goal, Result, Feedback) directly.
#[derive(Debug, Clone, Serialize, Deserialize, Default, MessageTypeInfo)]
#[ros_msg(type_name = "custom_msgs/srv/NavigateTo_Request")]
pub struct NavigateToRequest {
    pub target_x: f64,
    pub target_y: f64,
    pub max_speed: f64,
}

impl hiroz::msg::ZMessage for NavigateToRequest {
    type Serdes = hiroz::msg::SerdeCdrSerdes<NavigateToRequest>;
}

// Custom service response
#[derive(Debug, Clone, Serialize, Deserialize, Default, MessageTypeInfo)]
#[ros_msg(type_name = "custom_msgs/srv/NavigateTo_Response")]
pub struct NavigateToResponse {
    pub success: bool,
    pub estimated_duration: f64,
    pub message: String,
}

impl hiroz::msg::ZMessage for NavigateToResponse {
    type Serdes = hiroz::msg::SerdeCdrSerdes<NavigateToResponse>;
}

// Service type definition
pub struct NavigateTo;

impl ServiceTypeInfo for NavigateTo {
    fn service_type_info() -> hiroz::entity::TypeInfo {
        hiroz::entity::TypeInfo::new("custom_msgs::srv::dds_::NavigateTo_", TypeHash::zero())
    }
}

impl ZService for NavigateTo {
    type Request = NavigateToRequest;
    type Response = NavigateToResponse;
}

#[cfg(not(test))]
#[derive(Debug, Parser)]
struct Args {
    #[arg(
        short,
        long,
        default_value = "status-pub",
        help = "Mode: status-pub, status-sub, nav-server, or nav-client"
    )]
    mode: String,

    #[arg(
        long,
        default_value = "robot_1",
        help = "Robot ID (for status-pub mode)"
    )]
    robot_id: String,

    #[arg(long, default_value = "10.0", help = "Target X coordinate")]
    target_x: f64,

    #[arg(long, default_value = "20.0", help = "Target Y coordinate")]
    target_y: f64,

    #[arg(long, default_value = "1.5", help = "Maximum speed")]
    max_speed: f64,
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().mode.as_str() {
        "status-pub" => run_status_publisher(Args::parse().robot_id).await,
        "status-sub" => run_status_subscriber().await,
        "nav-server" => run_navigation_server(ZContextBuilder::default().build()?),
        "nav-client" => {
            let args = Args::parse();
            run_navigation_client(
                ZContextBuilder::default().build()?,
                args.target_x,
                args.target_y,
                args.max_speed,
            )
        }
        mode => {
            eprintln!(
                "Invalid mode: {mode}. Use 'status-pub', 'status-sub', 'nav-server', or 'nav-client'"
            );
            std::process::exit(1);
        }
    }
}

async fn run_status_publisher(robot_id: String) -> Result<()> {
    println!("Starting robot status publisher for robot: {robot_id}");

    let ctx = ZContextBuilder::default().build()?;
    let node = ctx
        .create_node("robot_status_publisher")
        .with_type_description_service()
        .build()?;
    let zpub = node.create_pub::<RobotStatus>("/robot_status").build()?;

    println!("Type description service enabled for automatic schema registration");

    let mut position_x = 0.0;
    let mut position_y = 0.0;
    let mut battery = 100.0;
    let mut moving = true;

    loop {
        // Simulate robot movement and battery drain
        if moving {
            position_x += 0.5;
            position_y += 0.3;
            battery -= 0.1;

            if battery < 20.0 {
                moving = false;
                println!("Low battery! Robot stopped.");
            }
        }

        let status = RobotStatus {
            robot_id: robot_id.clone(),
            battery_percentage: battery,
            position_x,
            position_y,
            is_moving: moving,
        };

        println!(
            "Publishing status: pos=({:.1}, {:.1}), battery={:.1}%, moving={}",
            status.position_x, status.position_y, status.battery_percentage, status.is_moving
        );

        zpub.async_publish(&status).await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        // Reset simulation when battery too low
        if battery < 10.0 {
            battery = 100.0;
            moving = true;
            println!("Battery recharged! Resuming movement.");
        }
    }
}

async fn run_status_subscriber() -> Result<()> {
    println!("Starting robot status subscriber...");

    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("robot_status_subscriber").build()?;
    let zsub = node.create_sub::<RobotStatus>("/robot_status").build()?;

    loop {
        let status = zsub.async_recv().await?;
        println!(
            "Received status from {}: pos=({:.1}, {:.1}), battery={:.1}%, moving={}",
            status.robot_id,
            status.position_x,
            status.position_y,
            status.battery_percentage,
            status.is_moving
        );

        if status.battery_percentage < 20.0 {
            println!("WARNING: {} has low battery!", status.robot_id);
        }
    }
}

pub fn run_navigation_server(ctx: hiroz::context::ZContext) -> Result<()> {
    println!("Starting navigation service server...");

    let node = ctx.create_node("navigation_server").build()?;
    let mut zsrv = node.create_service::<NavigateTo>("/navigate_to").build()?;

    println!("Navigation server ready, waiting for requests...");

    loop {
        if let Ok(request) = zsrv.take_request() {
            println!(
                "Received navigation request: target=({:.1}, {:.1}), max_speed={:.1}",
                request.message().target_x,
                request.message().target_y,
                request.message().max_speed
            );

            // Simulate path planning
            std::thread::sleep(Duration::from_millis(500));

            let distance =
                (request.message().target_x.powi(2) + request.message().target_y.powi(2)).sqrt();
            let duration = distance / request.message().max_speed;

            let response = if request.message().max_speed > 0.0 && request.message().max_speed < 5.0
            {
                NavigateToResponse {
                    success: true,
                    estimated_duration: duration,
                    message: format!(
                        "Path planned successfully. Distance: {:.2}m, ETA: {:.2}s",
                        distance, duration
                    ),
                }
            } else {
                NavigateToResponse {
                    success: false,
                    estimated_duration: 0.0,
                    message: "Invalid max_speed. Must be between 0 and 5 m/s.".to_string(),
                }
            };

            println!("Sending response: {:?}", response);
            request.reply_blocking(&response)?;
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn run_navigation_client(
    ctx: hiroz::context::ZContext,
    target_x: f64,
    target_y: f64,
    max_speed: f64,
) -> Result<()> {
    println!("Starting navigation client...");

    let node = ctx.create_node("navigation_client").build()?;
    let zcli = node.create_client::<NavigateTo>("/navigate_to").build()?;

    let request = NavigateToRequest {
        target_x,
        target_y,
        max_speed,
    };

    println!(
        "Sending navigation request: target=({:.1}, {:.1}), max_speed={:.1}",
        request.target_x, request.target_y, request.max_speed
    );

    let response = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { zcli.call(&request).await })?;

    println!("Received response:");
    println!("Success: {}", response.success);
    println!("Duration: {:.2}s", response.estimated_duration);
    println!("Message: {}", response.message);

    Ok(())
}
