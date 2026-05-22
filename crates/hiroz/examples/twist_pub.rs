use std::{thread, time::Duration};

use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::geometry_msgs::{Twist, Vector3};

fn main() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("twist_publisher").build()?;
    let zpub = node.create_pub::<Twist>("cmd_vel").build()?;

    println!("Publishing Twist messages on /cmd_vel...");

    let mut counter = 0.0_f64;
    loop {
        let msg = Twist {
            linear: Vector3 {
                x: 0.5 * (counter * 0.1).sin(),
                y: 0.0,
                z: 0.0,
            },
            angular: Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.3 * (counter * 0.1).cos(),
            },
        };

        zpub.publish(&msg)?;
        println!(
            "Published: linear.x={:.2}, angular.z={:.2}",
            msg.linear.x, msg.angular.z
        );

        counter += 1.0;
        thread::sleep(Duration::from_millis(100));
    }
}
