/// Convert Python-style ROS message type name to Rust-style type name
///
/// Converts names like:
/// - `std_msgs/msg/String` → `std_msgs::msg::dds_::String_`
/// - `geometry_msgs/msg/Vector3` → `geometry_msgs::msg::dds_::Vector3_`
/// - `sensor_msgs/msg/LaserScan` → `sensor_msgs::msg::dds_::LaserScan_`
///
/// The Rust format uses:
/// - `::` separators instead of `/`
/// - `dds_::` prefix before the message name
/// - `_` suffix after the message name
pub(crate) fn python_type_to_rust_type(python_type: &str) -> String {
    // Split by '/'
    let parts: Vec<&str> = python_type.split('/').collect();

    if parts.len() != 3 {
        // If it's not in the expected format, return as-is
        return python_type.to_string();
    }

    let package = parts[0];
    let category = parts[1]; // Usually "msg", "srv", or "action"
    let msg_name = parts[2];

    // Convert to Rust format: package::category::dds_::MessageName_
    format!("{}::{}::dds_::{}_", package, category, msg_name)
}
