//! Golden-reference tests for standard ROS 2 action service type hashes.
//!
//! The `cancel_goal_type_info` hash produced by codegen MUST match the
//! RIHS01 hash that ROS 2 (rmw_zenoh) computes for `action_msgs/srv/CancelGoal`,
//! because rmw_zenoh routes service queries by key expression — which embeds
//! the type hash. A mismatch makes the GUI's "cancel mission" action silently
//! find no queryable and fail with
//! "Service call ended before any response was received".
//!
//! Ground truth: the shipped ROS type-description JSON of the target distro
//! (`/opt/ros/jazzy/share/action_msgs/srv/CancelGoal.json` on this machine,
//! verbatim below).

use std::collections::BTreeMap;

use hiroz_schema::{
    calculate_hash, to_hash_version, to_ros2_json, FieldDescription, TypeDescription,
    TypeDescriptionMsg, TypeId,
};
use serde_json::Value;

/// Verbatim from Jazzy: /opt/ros/jazzy/share/action_msgs/srv/CancelGoal.json
const ROS_CANCELGOAL_JSON: &str = r#"
{
  "type_description_msg": {
    "type_description": {
      "type_name": "action_msgs/srv/CancelGoal",
      "fields": [
        {
          "name": "request_message",
          "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "action_msgs/srv/CancelGoal_Request"},
          "default_value": ""
        },
        {
          "name": "response_message",
          "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "action_msgs/srv/CancelGoal_Response"},
          "default_value": ""
        },
        {
          "name": "event_message",
          "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "action_msgs/srv/CancelGoal_Event"},
          "default_value": ""
        }
      ]
    },
    "referenced_type_descriptions": [
      {
        "type_name": "action_msgs/msg/GoalInfo",
        "fields": [
          {
            "name": "goal_id",
            "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "unique_identifier_msgs/msg/UUID"},
            "default_value": ""
          },
          {
            "name": "stamp",
            "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "builtin_interfaces/msg/Time"},
            "default_value": ""
          }
        ]
      },
      {
        "type_name": "action_msgs/srv/CancelGoal_Event",
        "fields": [
          {
            "name": "info",
            "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "service_msgs/msg/ServiceEventInfo"},
            "default_value": ""
          },
          {
            "name": "request",
            "type": {"type_id": 97, "capacity": 1, "string_capacity": 0, "nested_type_name": "action_msgs/srv/CancelGoal_Request"},
            "default_value": ""
          },
          {
            "name": "response",
            "type": {"type_id": 97, "capacity": 1, "string_capacity": 0, "nested_type_name": "action_msgs/srv/CancelGoal_Response"},
            "default_value": ""
          }
        ]
      },
      {
        "type_name": "action_msgs/srv/CancelGoal_Request",
        "fields": [
          {
            "name": "goal_info",
            "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "action_msgs/msg/GoalInfo"},
            "default_value": ""
          }
        ]
      },
      {
        "type_name": "action_msgs/srv/CancelGoal_Response",
        "fields": [
          {
            "name": "return_code",
            "type": {"type_id": 2, "capacity": 0, "string_capacity": 0, "nested_type_name": ""},
            "default_value": ""
          },
          {
            "name": "goals_canceling",
            "type": {"type_id": 145, "capacity": 0, "string_capacity": 0, "nested_type_name": "action_msgs/msg/GoalInfo"},
            "default_value": ""
          }
        ]
      },
      {
        "type_name": "builtin_interfaces/msg/Time",
        "fields": [
          {"name": "sec", "type": {"type_id": 6, "capacity": 0, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""},
          {"name": "nanosec", "type": {"type_id": 7, "capacity": 0, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""}
        ]
      },
      {
        "type_name": "service_msgs/msg/ServiceEventInfo",
        "fields": [
          {"name": "event_type", "type": {"type_id": 3, "capacity": 0, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""},
          {"name": "stamp", "type": {"type_id": 1, "capacity": 0, "string_capacity": 0, "nested_type_name": "builtin_interfaces/msg/Time"}, "default_value": ""},
          {"name": "client_gid", "type": {"type_id": 51, "capacity": 16, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""},
          {"name": "sequence_number", "type": {"type_id": 8, "capacity": 0, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""}
        ]
      },
      {
        "type_name": "unique_identifier_msgs/msg/UUID",
        "fields": [
          {"name": "uuid", "type": {"type_id": 51, "capacity": 16, "string_capacity": 0, "nested_type_name": ""}, "default_value": ""}
        ]
      }
    ]
  }
}"#;

/// The hash ROS 2 (Jazzy) assigns to `action_msgs/srv/CancelGoal`.
pub const ROS_JAZZY_CANCELGOAL_RIHS: &str =
    "RIHS01_573d8b0a534451d7bc2ac8c5ffde8ac14b8593b7001175d0cd6516dcbeb8689a";

fn parse_ros_json() -> TypeDescriptionMsg {
    let v: Value = serde_json::from_str(ROS_CANCELGOAL_JSON).expect("embedded ROS JSON must parse");
    let msg = &v["type_description_msg"];
    serde_json::from_value(msg.clone()).expect("ROS JSON must map to TypeDescriptionMsg")
}

/// The RIHS01 algorithm on exactly the ROS-shipped description must reproduce
/// ROS's own hash (validates our implementation against ground truth).
#[test]
fn hash_of_verbatim_ros_description_matches_ros() {
    let msg = parse_ros_json();
    let got = calculate_hash(&msg);
    let got_str = format!("{got}");
    assert_eq!(
        got_str, ROS_JAZZY_CANCELGOAL_RIHS,
        "verbatim ROS description does not reproduce the ROS hash — hashing implementation diverges"
    );
}

/// The `build_service_description_msg` reconstruction must equal the ROS-shipped
/// structure (same referenced set, same field orders), so its hash must equal
/// ROS's own CancelGoal hash.
#[test]
fn built_cancel_goal_description_matches_ros() {
    let ros = parse_ros_json();
    let referenced_by_name: BTreeMap<String, TypeDescription> = ros
        .referenced_type_descriptions
        .iter()
        .map(|d| (d.type_name.clone(), d.clone()))
        .collect();

    // The parts generated code would build: request/response literal descs
    // (they exist in the ROS set already — use them verbatim to isolate the
    // builder's behavior).
    let request_desc = referenced_by_name
        .get("action_msgs/srv/CancelGoal_Request")
        .expect("ROS JSON request desc")
        .clone();
    let response_desc = referenced_by_name
        .get("action_msgs/srv/CancelGoal_Response")
        .expect("ROS JSON response desc")
        .clone();
    let service_event_info_desc = referenced_by_name
        .get("service_msgs/msg/ServiceEventInfo")
        .expect("ROS JSON ServiceEventInfo")
        .clone();

    // Deps the resolver would seed: GoalInfo + its nested closure (Time, UUID)
    // + ServiceEventInfo. Every referenced type except req/resp/event, which
    // the builder adds itself.
    let mut deps: BTreeMap<String, TypeDescription> = BTreeMap::new();
    for name in [
        "action_msgs/msg/GoalInfo",
        "builtin_interfaces/msg/Time",
        "service_msgs/msg/ServiceEventInfo",
        "unique_identifier_msgs/msg/UUID",
    ] {
        let d = referenced_by_name
            .get(name)
            .unwrap_or_else(|| panic!("ROS JSON missing {name}"))
            .clone();
        deps.insert(d.type_name.clone(), d);
    }

    let built = hiroz_codegen::hashing::build_service_description_msg(
        "action_msgs",
        "CancelGoal",
        event_desc_from_ros(&referenced_by_name),
        service_desc_from_ros(&ros),
        &request_desc,
        &response_desc,
        &deps,
    );

    let got = calculate_hash(&built);
    let got_str = format!("{got}");
    if got_str != ROS_JAZZY_CANCELGOAL_RIHS {
        // Diff the hashable JSON against the ROS reference for a useful message.
        let built_json = to_ros2_json(&to_hash_version(&built))
            .expect("serialize built description");
        let ros_json = to_ros2_json(&to_hash_version(&ros)).expect("serialize ROS description");
        panic!(
            "builder hash {got_str} != ROS {ROS_JAZZY_CANCELGOAL_RIHS}\n\
             BUILT: {built_json}\nROS:   {ros_json}"
        );
    }
}

/// Extract the CancelGoal_Event description from the ROS reference (the builder
/// otherwise constructs it internally; feeding the ROS-shipped one via the
/// `event_desc` parameter keeps this test focused on the referenced set).
fn event_desc_from_ros(
    referenced: &BTreeMap<String, TypeDescription>,
) -> TypeDescription {
    referenced
        .get("action_msgs/srv/CancelGoal_Event")
        .expect("ROS JSON event desc")
        .clone()
}

/// Extract the top-level service description from the ROS reference.
fn service_desc_from_ros(msg: &TypeDescriptionMsg) -> TypeDescription {
    msg.type_description.clone()
}
