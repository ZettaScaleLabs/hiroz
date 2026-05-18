use hiroz::{Builder, Result, context::ZContextBuilder};
use hiroz_msgs::sensor_msgs::BatteryState;

fn main() -> Result<()> {
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("battery_state_subscriber").build()?;
    let zsub = node.create_sub::<BatteryState>("battery_status").build()?;

    println!("Listening for BatteryState messages on /battery_status...");

    loop {
        let msg = zsub.recv()?;
        println!("Received BatteryState:");
        println!("  Voltage: {:.2}V", msg.voltage);
        println!("  Percentage: {:.1}%", msg.percentage * 100.0);
        println!(
            "  Status: {}",
            match msg.power_supply_status {
                BatteryState::POWER_SUPPLY_STATUS_UNKNOWN => "Unknown",
                BatteryState::POWER_SUPPLY_STATUS_CHARGING => "Charging",
                BatteryState::POWER_SUPPLY_STATUS_DISCHARGING => "Discharging",
                BatteryState::POWER_SUPPLY_STATUS_NOT_CHARGING => "Not Charging",
                BatteryState::POWER_SUPPLY_STATUS_FULL => "Full",
                _ => "Invalid",
            }
        );
        println!("  Temperature: {:.1}°C", msg.temperature);
        println!("  Current: {:.2}A", msg.current);
        println!("  Charge: {:.2}Ah", msg.charge);
        println!("  Capacity: {:.2}Ah", msg.capacity);
        println!("---");
    }
}
