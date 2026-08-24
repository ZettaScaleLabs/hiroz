//! The mapping between the two spellings of a ROS type name.
//!
//! `hiroz_schema::type_name` holds that rule once, for the whole workspace. If
//! two derivations of it disagree, two nodes publish on different key
//! expressions and never see each other. Neither one reports an error.
//!
//! These tests pin the forward rule, both inverses, and the two cases where the
//! inverses are required to disagree.

use hiroz_schema::type_name::{
    KINDS, dds_from_canonical, dds_from_namespace, ros_from_dds, ros_from_dds_strict,
    service_from_response, split_canonical,
};

/// The exact string `rmw_zenoh_cpp` was measured to declare on the wire for
/// `demo_nodes_cpp talker`, recovered from a raw zenoh subscriber on `**`.
const MEASURED_WIRE_NAME: &str = "std_msgs::msg::dds_::String_";

#[test]
fn the_canonical_and_namespace_forms_agree() {
    assert_eq!(
        dds_from_canonical("std_msgs/msg/String").unwrap(),
        dds_from_namespace("std_msgs::msg", "String")
    );
    assert_eq!(
        dds_from_canonical("std_msgs/msg/String").unwrap(),
        MEASURED_WIRE_NAME
    );
}

#[test]
fn every_kind_round_trips() {
    for kind in KINDS {
        let canonical = format!("my_pkg/{kind}/Thing");
        let dds = dds_from_canonical(&canonical).unwrap();
        assert_eq!(dds, format!("my_pkg::{kind}::dds_::Thing_"));
        assert_eq!(ros_from_dds_strict(&dds), canonical);
        assert_eq!(ros_from_dds(&dds), canonical);
    }
}

#[test]
fn an_empty_namespace_is_legal() {
    assert_eq!(dds_from_namespace("", "Bare"), "dds_::Bare_");
    assert_eq!(ros_from_dds_strict("dds_::Bare_"), "Bare");
}

#[test]
fn action_sub_types_go_through_the_same_rule() {
    assert_eq!(
        dds_from_namespace("my_pkg::action", "Fib_SendGoal"),
        "my_pkg::action::dds_::Fib_SendGoal_"
    );
    assert_eq!(
        ros_from_dds_strict("my_pkg::action::dds_::Fib_SendGoal_"),
        "my_pkg/action/Fib_SendGoal"
    );
}

#[test]
fn a_service_name_comes_from_its_response_name() {
    let response = dds_from_namespace("example_interfaces::srv", "AddTwoInts_Response");
    assert_eq!(
        response,
        "example_interfaces::srv::dds_::AddTwoInts_Response_"
    );
    assert_eq!(
        service_from_response(&response).unwrap(),
        dds_from_canonical("example_interfaces/srv/AddTwoInts").unwrap()
    );
}

#[test]
fn a_name_that_is_not_a_response_has_no_service_name() {
    assert_eq!(service_from_response("std_msgs::msg::dds_::String_"), None);
    assert_eq!(service_from_response("AddTwoInts_Response"), None);
}

#[test]
fn split_canonical_rejects_what_is_not_canonical() {
    assert_eq!(
        split_canonical("std_msgs/msg/String"),
        Some(("std_msgs", "msg", "String"))
    );
    assert_eq!(split_canonical("std_msgs/String"), None);
    assert_eq!(split_canonical("a/b/c/d"), None);
    assert_eq!(split_canonical("std_msgs/nope/String"), None);
    assert_eq!(dds_from_canonical("std_msgs/nope/String"), None);
}

/// The one case where the two inverses are required to disagree.
#[test]
fn the_strict_inverse_leaves_a_name_without_dds_alone_and_the_lenient_one_does_not() {
    let no_dds_segment = "rcl_interfaces::msg::ParameterEvent_";
    assert_eq!(ros_from_dds_strict(no_dds_segment), no_dds_segment);
    assert_eq!(
        ros_from_dds(no_dds_segment),
        "rcl_interfaces/msg/ParameterEvent"
    );
}

#[test]
fn both_inverses_leave_a_name_that_is_already_canonical_alone() {
    for input in ["std_msgs/msg/String", "", "plain", "pkg/srv/Name_Request"] {
        assert_eq!(
            ros_from_dds_strict(input),
            input,
            "strict changed {input:?}"
        );
        assert_eq!(ros_from_dds(input), input, "lenient changed {input:?}");
    }
}

/// The second case where the two are required to differ. The lenient one
/// trims a trailing underscore from any name, which is what the schema
/// lookup it serves has always done; the strict one may not, because
/// `rmw_zenoh_cpp` does not.
#[test]
fn only_the_lenient_inverse_trims_a_trailing_underscore_from_a_foreign_name() {
    assert_eq!(
        ros_from_dds_strict("some::other::Type_"),
        "some::other::Type_"
    );
    assert_eq!(ros_from_dds("some::other::Type_"), "some::other::Type");
}
