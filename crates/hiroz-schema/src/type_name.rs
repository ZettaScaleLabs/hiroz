//! The mapping between the two spellings of a ROS type name.
//!
//! ROS names a type `std_msgs/msg/String`. The DDS wire spells the same type
//! `std_msgs::msg::dds_::String_`. Every rmw_zenoh key expression and every
//! liveliness token uses that second form.
//!
//! Both forms appear throughout hiroz. A disagreement between two derivations
//! of them is not a cosmetic problem. Two nodes that mangle differently publish
//! on different key expressions. They then never see each other, and neither
//! reports an error.
//!
//! This module holds that rule once. It lives in `hiroz-schema` for two
//! reasons. This crate already owns the canonical form, because
//! `TypeDescription::type_name` is `std_msgs/msg/String`. This crate is also a
//! leaf, so the runtime, the code generator and the derive macro all reach it.

/// The three interface kinds a canonical ROS type name can carry.
pub const KINDS: [&str; 3] = ["msg", "srv", "action"];

const DDS_SEGMENT: &str = "dds_::";

/// Build the DDS form from a namespace that is already joined with `::` and a
/// bare type name: `("std_msgs::msg", "String")` gives
/// `std_msgs::msg::dds_::String_`.
///
/// The C++ typesupport hands `rmw-zenoh-rs` this shape at runtime, because
/// `message_namespace_` and `message_name_` arrive separately. An empty
/// namespace is legal, and gives `dds_::String_`.
///
/// The action sub-types also use this function. Pass the decorated name, for
/// example `("my_pkg::action", "Fib_SendGoal")`.
pub fn dds_from_namespace(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        format!("{DDS_SEGMENT}{name}_")
    } else {
        format!("{namespace}::{DDS_SEGMENT}{name}_")
    }
}

/// Split a canonical ROS type name into `(package, kind, name)`.
///
/// This function returns `None` unless the input has exactly three
/// `/`-separated parts. Its middle part must be one of [`KINDS`]. A caller that
/// supports fewer kinds checks the returned kind itself, so that each caller
/// gives its own error message.
pub fn split_canonical(canonical: &str) -> Option<(&str, &str, &str)> {
    let mut parts = canonical.split('/');
    let (package, kind, name) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !KINDS.contains(&kind) {
        return None;
    }
    Some((package, kind, name))
}

/// Build the DDS form from a canonical name: `std_msgs/msg/String` gives
/// `std_msgs::msg::dds_::String_`.
///
/// This function returns `None` when the input is not a canonical three-part
/// name. The caller then decides whether that is an error, or a value to pass
/// through.
pub fn dds_from_canonical(canonical: &str) -> Option<String> {
    let (package, kind, name) = split_canonical(canonical)?;
    Some(dds_from_namespace(&format!("{package}::{kind}"), name))
}

/// Derive a service's DDS type name from its response type's DDS type name:
/// `example_interfaces::srv::dds_::AddTwoInts_Response_` gives
/// `example_interfaces::srv::dds_::AddTwoInts_`.
///
/// The rmw_zenoh key for a service names the service. A typesupport gives only
/// the response type. This function returns `None` when the input does not end
/// in the response suffix, so the caller chooses whether that is an error.
pub fn service_from_response(response_dds: &str) -> Option<String> {
    response_dds
        .strip_suffix("_Response_")
        .map(|stem| format!("{stem}_"))
}

/// Recover the canonical form from the DDS form, exactly as `rmw_zenoh_cpp`
/// does.
///
/// This is a direct port of `_demangle_if_ros_type` in `rmw_zenoh_cpp`'s
/// `graph_cache.cpp`. It keeps both bail-out arms. It returns unchanged an
/// input with no trailing `_`, and an input with no `dds_::` segment.
///
/// **Use this function at an RMW boundary.** There, the code must match the
/// reference implementation byte for byte. Use [`ros_from_dds`] instead when
/// the goal is to find a schema.
pub fn ros_from_dds_strict(dds: &str) -> String {
    let Some(stem) = dds.strip_suffix('_') else {
        return dds.to_string();
    };
    let Some(pos) = stem.find(DDS_SEGMENT) else {
        return dds.to_string();
    };
    let namespace = stem[..pos].replace("::", "/");
    let name = &stem[pos + DDS_SEGMENT.len()..];
    format!("{namespace}{name}")
}

/// Recover the canonical form from the DDS form, tolerating a name whose
/// `dds_::` segment is missing.
///
/// [`ros_from_dds_strict`] returns `rcl_interfaces::msg::ParameterEvent_`
/// unchanged, because `rmw_zenoh_cpp` returns it unchanged. That is correct at
/// an RMW boundary. It is wrong when the caller looks the result up in the
/// schema registry. That caller needs `rcl_interfaces/msg/ParameterEvent`, and
/// a name that stays mangled fails to resolve.
///
/// So this function applies the strict rule first. It falls back to replacing a
/// bare `::<kind>::` separator only when the strict rule declines. The two
/// functions agree on every name that carries a `dds_::` segment. They differ
/// only on the names that the strict rule must leave alone.
///
/// The fallback also trims a trailing underscore from **any** name it sees,
/// including a name that is not a ROS type. This behaviour is deliberate. This
/// path has always done it, and its callers look a name up in the schema
/// registry. There, a stray underscore decides a hit or a miss.
pub fn ros_from_dds(dds: &str) -> String {
    let strict = ros_from_dds_strict(dds);
    if strict != dds {
        return strict;
    }
    let mut out = dds.to_string();
    for kind in KINDS {
        out = out.replace(&format!("::{kind}::"), &format!("/{kind}/"));
    }
    out.trim_end_matches('_').to_string()
}
