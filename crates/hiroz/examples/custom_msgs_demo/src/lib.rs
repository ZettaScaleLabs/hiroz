//! Custom robot messages generated from ROS2 .msg files.
//!
//! This crate demonstrates how to use hiroz-codegen to generate Rust types
//! from user-defined ROS2 message definitions.
//!
//! # Building
//!
//! Set the `HIROZ_MSG_PATH` environment variable to point to your message package:
//!
//! ```bash
//! cd hiroz/examples/custom_msgs_demo
//! HIROZ_MSG_PATH="./my_robot_msgs" cargo build
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use custom_msgs_demo::ros::my_robot_msgs::{RobotStatus, SensorReading};
//! use custom_msgs_demo::ros::my_robot_msgs::srv::NavigateTo;
//!
//! let status = RobotStatus {
//!     robot_id: "robot_1".to_string(),
//!     battery_percentage: 85.5,
//!     position: hiroz_msgs::ros::geometry_msgs::Point { x: 1.0, y: 2.0, z: 0.0 },
//!     is_moving: true,
//! };
//! ```

// Re-export hiroz-msgs for standard message types
pub use hiroz_msgs::*;

// Include generated user messages
// These are generated at build time by hiroz_codegen::generate_user_messages()
include!(concat!(env!("OUT_DIR"), "/generated.rs"));

// User messages are now available as:
// - crate::ros::my_robot_msgs::RobotStatus
// - crate::ros::my_robot_msgs::SensorReading
// - crate::ros::my_robot_msgs::srv::NavigateTo
// - crate::ros::my_robot_msgs::NavigateToRequest
// - crate::ros::my_robot_msgs::NavigateToResponse
