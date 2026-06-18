use hiroz_protocol::entity::{TypeHash, TypeInfo};

/// Convert a ROS 2 pub/sub name (e.g. `/chatter`) to its DDS topic name (`rt/chatter`).
pub fn ros2_name_to_dds_pub_topic(ros2_name: &str) -> String {
    let stripped = ros2_name.strip_prefix('/').unwrap_or(ros2_name);
    format!("rt/{stripped}")
}

/// Convert a ROS 2 service name (e.g. `/add_two_ints`) to the DDS request topic name.
pub fn ros2_name_to_dds_request_topic(ros2_name: &str) -> String {
    let stripped = ros2_name.strip_prefix('/').unwrap_or(ros2_name);
    format!("rq/{stripped}Request")
}

/// Convert a ROS 2 service name to the DDS reply topic name.
pub fn ros2_name_to_dds_reply_topic(ros2_name: &str) -> String {
    let stripped = ros2_name.strip_prefix('/').unwrap_or(ros2_name);
    format!("rr/{stripped}Reply")
}

const USER_DATA_TYPEHASH_PREFIX: &str = "typehash=";

/// Parse the type hash from DDS `USER_DATA` QoS bytes.
///
/// rmw_zenoh_cpp encodes it as `typehash=RIHS01_<64hex>;` (semicolons separate
/// multiple entries). Returns `None` when absent, malformed, or for Humble nodes
/// which do not set `user_data`.
pub fn type_info_from_user_data(type_name: &str, user_data: &[u8]) -> Option<TypeInfo> {
    let s = std::str::from_utf8(user_data).ok()?;
    for part in s.split(';') {
        if let Some(rihs) = part.trim().strip_prefix(USER_DATA_TYPEHASH_PREFIX)
            && let Some(hash) = TypeHash::from_rihs_string(rihs)
        {
            return Some(TypeInfo::new(type_name, hash));
        }
    }
    None
}

/// Convert a DDS topic name to a ROS 2 topic/service name.
///
/// DDS uses prefixes: `rt/` (topic), `rq/` (request), `rr/` (reply).
/// Returns `None` for unrecognised prefixes (e.g. built-in topics).
pub fn dds_topic_to_ros2_name(dds_topic: &str) -> Option<String> {
    // Topics: "rt/<ros_name>" → "/<ros_name>"
    if let Some(rest) = dds_topic.strip_prefix("rt/") {
        return Some(format!("/{rest}"));
    }
    // Service requests: "rq/<name>Request" → "/<name>"
    if let Some(rest) = dds_topic.strip_prefix("rq/")
        && let Some(name) = rest.strip_suffix("Request")
    {
        return Some(format!("/{name}"));
    }
    // Service replies: "rr/<name>Reply" → "/<name>"
    if let Some(rest) = dds_topic.strip_prefix("rr/")
        && let Some(name) = rest.strip_suffix("Reply")
    {
        return Some(format!("/{name}"));
    }
    None
}

/// True if the DDS topic belongs to a ROS 2 service request channel.
pub fn is_request_topic(dds_topic: &str) -> bool {
    dds_topic.starts_with("rq/")
}

/// True if the DDS topic is a regular pub/sub topic.
pub fn is_pubsub_topic(dds_topic: &str) -> bool {
    dds_topic.starts_with("rt/")
}

/// Convert a DDS type string (e.g. `std_msgs::msg::dds_::String_`) to a ROS 2 type
/// (e.g. `std_msgs/msg/String`).
pub fn dds_type_to_ros2_type(dds_type: &str) -> String {
    let result = dds_type.replace("::dds_::", "::").replace("::", "/");
    if result.ends_with('_') {
        result[..result.len() - 1].into()
    } else {
        result
    }
}

/// Convert a ROS 2 type string to a DDS type string.
pub fn ros2_type_to_dds_type(ros2_type: &str) -> String {
    let mut result = ros2_type.replace('/', "::");
    if let Some(pos) = result.rfind(':') {
        result.insert_str(pos + 1, "dds_::");
    }
    result.push('_');
    result
}

/// Strip the service suffix (`_Request_` / `_Response_`) and convert to ROS 2 type.
pub fn dds_type_to_ros2_service_type(dds_type: &str) -> String {
    dds_type_to_ros2_type(
        dds_type
            .strip_suffix("_Request_")
            .or(dds_type.strip_suffix("_Response_"))
            .unwrap_or(dds_type),
    )
}

/// Strip the action-specific suffix and convert to a ROS 2 action type.
///
/// Mirrors `dds_type_to_ros2_action_type` from zenoh-plugin-ros2dds. Handles all
/// five action component DDS type suffixes: `_SendGoal_{Request,Response}_`,
/// `_GetResult_{Request,Response}_`, and `_FeedbackMessage_`.
pub fn dds_type_to_ros2_action_type(dds_type: &str) -> String {
    dds_type_to_ros2_type(
        dds_type
            .strip_suffix("_SendGoal_Request_")
            .or(dds_type.strip_suffix("_SendGoal_Response_"))
            .or(dds_type.strip_suffix("_GetResult_Request_"))
            .or(dds_type.strip_suffix("_GetResult_Response_"))
            .or(dds_type.strip_suffix("_FeedbackMessage_"))
            .unwrap_or(dds_type),
    )
}

/// True if the DDS request topic is an action `get_result` channel.
///
/// Action get_result calls can block for hundreds of seconds while the goal executes.
/// They need a much longer querier timeout (300 s) than regular services (10 s).
pub fn is_action_get_result_topic(dds_topic: &str) -> bool {
    // Pattern: "rq/<action_name>/_action/get_resultRequest"
    dds_topic.starts_with("rq/") && dds_topic.ends_with("/_action/get_resultRequest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_get_result_topic() {
        assert!(is_action_get_result_topic(
            "rq/fibonacci/_action/get_resultRequest"
        ));
        assert!(is_action_get_result_topic(
            "rq/my_ns/my_action/_action/get_resultRequest"
        ));
        assert!(!is_action_get_result_topic(
            "rq/fibonacci/_action/send_goalRequest"
        ));
        assert!(!is_action_get_result_topic(
            "rq/fibonacci/_action/cancel_goalRequest"
        ));
        assert!(!is_action_get_result_topic("rt/chatter"));
    }

    #[test]
    fn test_dds_topic_conversions() {
        assert_eq!(
            dds_topic_to_ros2_name("rt/chatter"),
            Some("/chatter".into())
        );
        assert_eq!(
            dds_topic_to_ros2_name("rq/add_two_intsRequest"),
            Some("/add_two_ints".into())
        );
        assert_eq!(
            dds_topic_to_ros2_name("rr/add_two_intsReply"),
            Some("/add_two_ints".into())
        );
        assert_eq!(dds_topic_to_ros2_name("DCPS_something"), None);
    }

    #[test]
    fn test_type_conversions() {
        assert_eq!(
            dds_type_to_ros2_type("std_msgs::msg::dds_::String_"),
            "std_msgs/msg/String"
        );
        assert_eq!(
            ros2_type_to_dds_type("std_msgs/msg/String"),
            "std_msgs::msg::dds_::String_"
        );
        assert_eq!(
            dds_type_to_ros2_service_type("example_interfaces::srv::dds_::AddTwoInts_Request_"),
            "example_interfaces/srv/AddTwoInts"
        );
        assert_eq!(
            dds_type_to_ros2_service_type("example_interfaces::srv::dds_::AddTwoInts_Response_"),
            "example_interfaces/srv/AddTwoInts"
        );
        assert_eq!(
            dds_type_to_ros2_type("geometry_msgs::msg::dds_::Twist_"),
            "geometry_msgs/msg/Twist"
        );
        assert_eq!(
            ros2_type_to_dds_type("geometry_msgs/msg/Twist"),
            "geometry_msgs::msg::dds_::Twist_"
        );
    }

    #[test]
    fn test_action_type_conversions() {
        let base = "example_interfaces/action/Fibonacci";
        assert_eq!(
            dds_type_to_ros2_action_type(
                "example_interfaces::action::dds_::Fibonacci_SendGoal_Request_"
            ),
            base
        );
        assert_eq!(
            dds_type_to_ros2_action_type(
                "example_interfaces::action::dds_::Fibonacci_SendGoal_Response_"
            ),
            base
        );
        assert_eq!(
            dds_type_to_ros2_action_type(
                "example_interfaces::action::dds_::Fibonacci_GetResult_Request_"
            ),
            base
        );
        assert_eq!(
            dds_type_to_ros2_action_type(
                "example_interfaces::action::dds_::Fibonacci_GetResult_Response_"
            ),
            base
        );
        assert_eq!(
            dds_type_to_ros2_action_type(
                "example_interfaces::action::dds_::Fibonacci_FeedbackMessage_"
            ),
            base
        );
    }

    #[test]
    fn test_is_request_topic() {
        assert!(is_request_topic("rq/add_two_intsRequest"));
        assert!(is_request_topic("rq/fibonacci/_action/send_goalRequest"));
        assert!(!is_request_topic("rr/add_two_intsReply"));
        assert!(!is_request_topic("rt/chatter"));
    }

    #[test]
    fn test_is_pubsub_topic() {
        assert!(is_pubsub_topic("rt/chatter"));
        assert!(is_pubsub_topic("rt/fibonacci/_action/feedback"));
        assert!(!is_pubsub_topic("rq/add_two_intsRequest"));
        assert!(!is_pubsub_topic("rr/add_two_intsReply"));
    }

    #[test]
    fn test_ros2_type_to_dds_type_roundtrip() {
        let ros2_types = [
            "std_msgs/msg/String",
            "geometry_msgs/msg/Twist",
            "example_interfaces/srv/AddTwoInts",
            "example_interfaces/action/Fibonacci",
        ];
        for ros2 in ros2_types {
            let dds = ros2_type_to_dds_type(ros2);
            let back = dds_type_to_ros2_type(&dds);
            assert_eq!(
                back, ros2,
                "roundtrip failed for {ros2}: dds={dds} back={back}"
            );
        }
    }

    #[test]
    fn test_dds_topic_to_ros2_name_nested_namespace() {
        assert_eq!(
            dds_topic_to_ros2_name("rt/my_ns/chatter"),
            Some("/my_ns/chatter".into())
        );
        assert_eq!(
            dds_topic_to_ros2_name("rq/my_ns/add_two_intsRequest"),
            Some("/my_ns/add_two_ints".into())
        );
    }
}
