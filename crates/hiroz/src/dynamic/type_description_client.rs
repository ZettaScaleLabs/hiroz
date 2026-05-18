//! Standard type-description protocol helpers.

use std::{sync::Arc, time::Duration};

use tracing::{debug, warn};

use crate::{Builder, node::ZNode};

#[cfg(test)]
use super::discovery::collect_topic_schema_candidates_from_publishers;
use super::{discovery::TopicSchemaCandidate, error::DynamicError, schema::MessageSchema};

use super::type_description::type_description_msg_to_schema;
use super::type_description_service::{
    GetTypeDescription, GetTypeDescriptionRequest, GetTypeDescriptionResponse,
    wire_to_schema_type_description,
};

pub(crate) async fn query_type_description(
    node: &ZNode,
    candidate: &TopicSchemaCandidate,
    timeout: Duration,
    include_sources: bool,
) -> Result<(Arc<MessageSchema>, String), DynamicError> {
    debug!(
        "[TDC] Querying type description: node={}/{}, type={}",
        candidate.namespace, candidate.node_name, candidate.type_name
    );

    let service_name = if candidate.namespace.is_empty() || candidate.namespace == "/" {
        format!("/{}/get_type_description", candidate.node_name)
    } else {
        format!(
            "{}/{}/get_type_description",
            candidate.namespace, candidate.node_name
        )
    };

    let client = node
        .create_client::<GetTypeDescription>(&service_name)
        .build()
        .map_err(|e| DynamicError::SerializationError(e.to_string()))?;
    let request = GetTypeDescriptionRequest {
        type_name: candidate.type_name.clone(),
        type_hash: candidate.type_hash.clone(),
        include_type_sources: include_sources,
    };

    let response = client
        .call_with_timeout(&request, timeout)
        .await
        .map_err(|_| DynamicError::ServiceTimeout {
            node: if candidate.namespace.is_empty() || candidate.namespace == "/" {
                candidate.node_name.clone()
            } else {
                format!("{}/{}", candidate.namespace, candidate.node_name)
            },
            service: service_name,
        })?;

    if response.successful {
        let schema = schema_from_type_description_response(&response)?;
        Ok((schema, candidate.type_hash.clone()))
    } else {
        warn!(
            "[TDC] Type description query failed: {}",
            response.failure_reason
        );
        Err(DynamicError::SerializationError(response.failure_reason))
    }
}

pub fn schema_from_type_description_response(
    response: &GetTypeDescriptionResponse,
) -> Result<Arc<MessageSchema>, DynamicError> {
    if !response.successful {
        return Err(DynamicError::SerializationError(format!(
            "Response indicates failure: {}",
            response.failure_reason
        )));
    }

    let type_desc_msg = wire_to_schema_type_description(&response.type_description);
    type_description_msg_to_schema(&type_desc_msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::schema::FieldType;
    use crate::dynamic::type_description_service::{
        WireTypeDescription, schema_to_wire_type_description,
    };
    use crate::entity::{EndpointEntity, EndpointKind, Entity, NodeEntity, TypeHash, TypeInfo};

    fn publisher_entity(node_name: Option<&str>, type_name: Option<&str>) -> Arc<Entity> {
        let node = node_name.map(|name| {
            NodeEntity::new(
                1,
                "1234567890abcdef1234567890abcdef".parse().unwrap(),
                0,
                name.to_string(),
                "/".to_string(),
                String::new(),
            )
        });

        Arc::new(Entity::Endpoint(EndpointEntity {
            id: 1,
            node,
            kind: EndpointKind::Publisher,
            topic: "/chatter".to_string(),
            type_info: type_name.map(|name| TypeInfo::new(name, TypeHash::zero())),
            qos: Default::default(),
        }))
    }

    #[test]
    fn test_response_to_schema_success() {
        // Build a schema and convert to wire format
        let original = MessageSchema::builder("std_msgs/msg/String")
            .field("data", FieldType::String)
            .build()
            .unwrap();

        let wire_td = schema_to_wire_type_description(&original).unwrap();

        let response = GetTypeDescriptionResponse {
            successful: true,
            failure_reason: String::new(),
            type_description: wire_td,
            type_sources: vec![],
            extra_information: vec![],
        };

        let schema = schema_from_type_description_response(&response).unwrap();
        assert_eq!(schema.type_name, "std_msgs/msg/String");
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "data");
    }

    #[test]
    fn test_response_to_schema_failure() {
        let response = GetTypeDescriptionResponse {
            successful: false,
            failure_reason: "Type not found".to_string(),
            type_description: WireTypeDescription::default(),
            type_sources: vec![],
            extra_information: vec![],
        };

        let result = schema_from_type_description_response(&response);
        assert!(result.is_err());
    }

    #[test]
    fn test_response_to_schema_nested() {
        // Build a nested schema
        let vector3 = MessageSchema::builder("geometry_msgs/msg/Vector3")
            .field("x", FieldType::Float64)
            .field("y", FieldType::Float64)
            .field("z", FieldType::Float64)
            .build()
            .unwrap();

        let twist = MessageSchema::builder("geometry_msgs/msg/Twist")
            .field("linear", FieldType::Message(vector3.clone()))
            .field("angular", FieldType::Message(vector3))
            .build()
            .unwrap();

        let wire_td = schema_to_wire_type_description(&twist).unwrap();

        let response = GetTypeDescriptionResponse {
            successful: true,
            failure_reason: String::new(),
            type_description: wire_td,
            type_sources: vec![],
            extra_information: vec![],
        };

        let schema = schema_from_type_description_response(&response).unwrap();
        assert_eq!(schema.type_name, "geometry_msgs/msg/Twist");
        assert_eq!(schema.fields.len(), 2);

        // Verify nested types are resolved
        if let FieldType::Message(nested) = &schema.fields[0].field_type {
            assert_eq!(nested.type_name, "geometry_msgs/msg/Vector3");
            assert_eq!(nested.fields.len(), 3);
        } else {
            panic!("Expected Message type for linear field");
        }
    }

    #[test]
    fn test_topic_discovery_uses_type_info_from_any_publisher() {
        let publishers = vec![
            publisher_entity(None, Some("std_msgs::msg::dds_::String_")),
            publisher_entity(Some("talker"), Some("std_msgs::msg::dds_::String_")),
        ];

        let candidates = collect_topic_schema_candidates_from_publishers(&publishers, "/chatter")
            .expect("expected type info to be discovered");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].node_name, "talker");
        assert_eq!(candidates[0].namespace, "/");
        assert_eq!(candidates[0].type_name, "std_msgs/msg/String");
        assert_eq!(candidates[0].type_hash, TypeHash::zero().to_rihs_string());
    }

    #[test]
    fn test_topic_discovery_reports_missing_node_identity_only_when_all_publishers_lack_it() {
        let publishers = vec![publisher_entity(None, Some("std_msgs::msg::dds_::String_"))];

        let err = collect_topic_schema_candidates_from_publishers(&publishers, "/chatter")
            .expect_err("expected missing node identity error");

        assert!(matches!(
            err,
            DynamicError::MissingNodeIdentity { ref topic } if topic == "/chatter"
        ));
    }
}
