use tracing::warn;

use crate::dynamic::{MessageSchema, MessageSchemaTypeDescription};
use crate::entity::{TypeHash, TypeInfo};

pub(crate) fn dds_type_name_from_schema(schema: &MessageSchema) -> String {
    schema
        .type_name
        .replace("/msg/", "::msg::dds_::")
        .replace("/srv/", "::srv::dds_::")
        .replace("/action/", "::action::dds_::")
        + "_"
}

/// Convert a DDS-mangled type name as it appears in liveliness tokens and the
/// graph (`std_msgs::msg::dds_::String_`) into the canonical ROS form the schema
/// registry and `.msg` loader expect (`std_msgs/msg/String`). Public because
/// out-of-crate consumers (e.g. `hu`'s WASM host) resolve graph-reported types
/// against `load_schema` and must use this exact normalisation rather than
/// re-deriving one -- see issue #172.
pub fn ros_type_name_from_dds(dds_name: &str) -> String {
    dds_name
        .replace("::msg::dds_::", "/msg/")
        .replace("::srv::dds_::", "/srv/")
        .replace("::action::dds_::", "/action/")
        .replace("::msg::", "/msg/")
        .replace("::srv::", "/srv/")
        .replace("::action::", "/action/")
        .trim_end_matches('_')
        .to_string()
}

pub(crate) fn schema_hash(schema: &MessageSchema) -> TypeHash {
    match schema.compute_type_hash() {
        Ok(hash) => {
            let rihs_string = hash.to_rihs_string();
            TypeHash::from_rihs_string(&rihs_string).unwrap_or_else(TypeHash::zero)
        }
        Err(error) => {
            warn!(
                "[NOD] Failed to compute type hash for {}: {}",
                schema.type_name, error
            );
            TypeHash::zero()
        }
    }
}

pub(crate) fn schema_type_info(schema: &MessageSchema) -> TypeInfo {
    TypeInfo {
        name: dds_type_name_from_schema(schema),
        hash: schema_hash(schema),
    }
}

/// Like `schema_type_info`, but uses the hash reported by the remote publisher rather
/// than recomputing it locally. This ensures the subscriber's key expression matches
/// the publisher's exact hash even when local recomputation would differ.
pub(crate) fn schema_type_info_with_hash(
    schema: &MessageSchema,
    discovered_hash: &str,
) -> TypeInfo {
    TypeInfo {
        name: dds_type_name_from_schema(schema),
        hash: TypeHash::from_rihs_string(discovered_hash).unwrap_or_else(TypeHash::zero),
    }
}
