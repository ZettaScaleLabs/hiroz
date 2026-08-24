use tracing::warn;

use crate::dynamic::{MessageSchema, MessageSchemaTypeDescription};
use crate::entity::{TypeHash, TypeInfo};

pub(crate) fn dds_type_name_from_schema(schema: &MessageSchema) -> String {
    // A schema whose type_name is not canonical has nothing to mangle. Pass it
    // through. The old string replacement half-mangled it instead.
    hiroz_schema::type_name::dds_from_canonical(&schema.type_name)
        .unwrap_or_else(|| schema.type_name.clone())
}

/// Convert a DDS-mangled type name as it appears in liveliness tokens and the
/// graph (`std_msgs::msg::dds_::String_`) into the canonical ROS form the schema
/// registry and `.msg` loader expect (`std_msgs/msg/String`). Public because
/// out-of-crate consumers (e.g. `hu`'s WASM host) resolve graph-reported types
/// against `load_schema` and must use this exact normalisation rather than
/// re-deriving one -- see issue #172.
///
/// This is the **lenient** inverse. It tolerates a name that has no `dds_::`
/// segment, because such a name must still resolve against the schema registry.
/// An RMW graph boundary needs the strict inverse instead, which is
/// [`hiroz_schema::ros_from_dds_strict`]. That one matches `rmw_zenoh_cpp`.
/// Both live in `hiroz_schema::type_name`, the one place that states the
/// rule.
pub fn ros_type_name_from_dds(dds_name: &str) -> String {
    hiroz_schema::type_name::ros_from_dds(dds_name)
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
