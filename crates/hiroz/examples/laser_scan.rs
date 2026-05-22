use std::{thread, time::Duration};

use clap::Parser;
use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::LaserScan, std_msgs::Header};

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "pub", help = "Mode: pub or sub")]
    mode: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.mode.as_str() {
        "pub" => run_publisher(),
        "sub" => run_subscriber(),
        _ => {
            eprintln!("Invalid mode: {}. Use 'pub' or 'sub'", args.mode);
            std::process::exit(1);
        }
    }
}

fn run_publisher() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("laser_scan_publisher").build()?;
    let zpub = node.create_pub::<LaserScan>("scan").build()?;

    println!("Publishing LaserScan messages on /scan...");

    let mut seq = 0u32;
    loop {
        // Simulate a 270-degree laser scanner with 540 points
        let angle_min = -135.0_f32.to_radians();
        let angle_max = 135.0_f32.to_radians();
        let num_readings = 540;
        let angle_increment = (angle_max - angle_min) / (num_readings as f32 - 1.0);

        let mut ranges = Vec::with_capacity(num_readings);
        let mut intensities = Vec::with_capacity(num_readings);

        // Generate simulated laser scan data
        for i in 0..num_readings {
            let angle = angle_min + (i as f32) * angle_increment;

            // Simulate a simple environment: closer ranges in front, farther on sides
            let base_range = 3.0 + 2.0 * angle.cos();

            // Add some variation
            let variation = 0.1 * ((seq as f32 * 0.1 + i as f32 * 0.05).sin());
            let range = base_range + variation;

            ranges.push(range);
            intensities.push(100.0 + 50.0 * (i as f32 / num_readings as f32));
        }

        let msg = LaserScan {
            header: Header {
                stamp: Time {
                    sec: (seq / 10) as i32,
                    nanosec: (seq % 10) * 100_000_000,
                },
                frame_id: "laser".to_string(),
            },
            angle_min,
            angle_max,
            angle_increment,
            time_increment: 0.0001,
            scan_time: 0.1,
            range_min: 0.1,
            range_max: 10.0,
            ranges,
            intensities,
        };

        zpub.publish(&msg)?;
        println!(
            "Published LaserScan #{}: {} ranges, angle [{:.2}, {:.2}] rad",
            seq,
            msg.ranges.len(),
            msg.angle_min,
            msg.angle_max
        );

        seq += 1;
        thread::sleep(Duration::from_millis(100));
    }
}

fn run_subscriber() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("laser_scan_subscriber").build()?;
    let zsub = node.create_sub::<LaserScan>("scan").build()?;

    println!("Listening for LaserScan messages on /scan...");

    loop {
        let msg = zsub.recv()?;
        println!("Received LaserScan:");
        println!("  Frame: {}", msg.header.frame_id);
        println!(
            "  Angle range: [{:.2}, {:.2}] rad",
            msg.angle_min, msg.angle_max
        );
        println!("  Angle increment: {:.4} rad", msg.angle_increment);
        println!("  Range: [{:.2}, {:.2}] m", msg.range_min, msg.range_max);
        println!("  Number of ranges: {}", msg.ranges.len());
        println!("  Scan time: {:.3} s", msg.scan_time);

        if !msg.ranges.is_empty() {
            let valid_ranges: Vec<f32> = msg
                .ranges
                .iter()
                .filter(|&&r| r >= msg.range_min && r <= msg.range_max)
                .copied()
                .collect();

            if !valid_ranges.is_empty() {
                let min = valid_ranges.iter().fold(f32::INFINITY, |a, &b| a.min(b));
                let max = valid_ranges
                    .iter()
                    .fold(f32::NEG_INFINITY, |a, &b| a.max(b));
                println!(
                    "  Valid ranges: {} (min: {:.2}m, max: {:.2}m)",
                    valid_ranges.len(),
                    min,
                    max
                );
            }
        }
        println!("---");
    }
}
