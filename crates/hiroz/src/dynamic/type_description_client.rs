//! Standard type-description protocol helpers.

use std::{future::Future, sync::Arc, time::Duration};

use tracing::{debug, warn};

use crate::{Builder, node::ZNode};

#[cfg(test)]
use super::discovery::collect_topic_schema_candidates_from_endpoints;
use super::{discovery::TopicSchemaCandidate, error::DynamicError, schema::MessageSchema};

use super::type_description::type_description_msg_to_schema;
use super::type_description_service::{
    GetTypeDescription, GetTypeDescriptionRequest, GetTypeDescriptionResponse,
    wire_to_schema_type_description,
};

// Zenoh 1.9's forced declare_keyexpr can return another thread's scope while
// its LocalResource is still declared=false, before DeclareKeyExpr is sent.
// A concurrent querier then sends an interest using that unknown scope. Keep
// only discovery-client construction ordered; route waits and queries remain
// concurrent. The async lock wait is included in each caller's deadline.
static CLIENT_DECLARATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// Match Tokio's duration overflow semantics: no representable deadline means
// an unbounded wait, including construction, route readiness and the response.
async fn with_deadline<F: Future>(
    deadline: Option<tokio::time::Instant>,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, future).await,
        None => Ok(future.await),
    }
}

pub(crate) async fn query_type_description(
    node: &ZNode,
    candidate: &TopicSchemaCandidate,
    deadline: Option<tokio::time::Instant>,
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

    let remote_node = if candidate.namespace.is_empty() || candidate.namespace == "/" {
        candidate.node_name.clone()
    } else {
        format!("{}/{}", candidate.namespace, candidate.node_name)
    };
    let timeout_error = || DynamicError::ServiceTimeout {
        node: remote_node.clone(),
        service: service_name.clone(),
    };
    let expired = || deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline);
    let client = {
        let _declaration = with_deadline(deadline, CLIENT_DECLARATION.lock())
            .await
            .map_err(|_| timeout_error())?;
        let remaining = deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
            .unwrap_or(Duration::MAX);
        if remaining.is_zero() {
            return Err(timeout_error());
        }
        node.create_client::<GetTypeDescription>(&service_name)
            .with_querier_timeout(remaining)
            .build()
            .map_err(|e| DynamicError::SerializationError(e.to_string()))?
    };
    let request = GetTypeDescriptionRequest {
        type_name: candidate.type_name.clone(),
        type_hash: candidate.type_hash.clone(),
        include_type_sources: include_sources,
    };
    let response = with_deadline(deadline, async {
        if expired() {
            return Err(crate::error::Error::timeout(std::time::Duration::ZERO));
        }
        // call() includes queryable-route readiness under this same deadline.
        client.call(&request).await
    })
    .await
    .map_err(|_| timeout_error())?
    .map_err(|error| {
        warn!(error = %error, service = %service_name, "Type description request failed");
        if expired() || crate::error::is_timeout(error.as_ref()) {
            timeout_error()
        } else {
            DynamicError::ServiceCallFailed {
                node: remote_node.clone(),
                service: service_name.clone(),
                reason: error.to_string(),
            }
        }
    })?;

    // Tokio polls a ready response before its timer. Recheck the same deadline
    // after caller scheduling and after schema conversion so queued/large valid
    // replies cannot turn an expired discovery into success.
    if expired() {
        return Err(timeout_error());
    }
    if response.successful {
        let schema = schema_from_type_description_response(&response)?;
        if expired() {
            return Err(timeout_error());
        }
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

        let candidates = collect_topic_schema_candidates_from_endpoints(&publishers, "/chatter")
            .expect("expected type info to be discovered");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].node_name, "talker");
        assert_eq!(candidates[0].namespace, "/");
        assert_eq!(candidates[0].type_name, "std_msgs/msg/String");
        assert_eq!(candidates[0].type_hash, TypeHash::zero().to_rihs_string());
    }

    #[test]
    fn test_topic_discovery_normalizes_legacy_names_without_changing_wire_identity() {
        let wire_name = "example_interfaces/AddTwoIntsRequest";
        let publisher = publisher_entity(Some("talker"), Some(wire_name));
        let candidates = collect_topic_schema_candidates_from_endpoints(
            std::slice::from_ref(&publisher),
            "/chatter",
        )
        .unwrap();

        assert_eq!(
            candidates[0].type_name,
            "example_interfaces/msg/AddTwoIntsRequest"
        );
        assert_eq!(candidates[0].transport_type_name, wire_name);
        assert_eq!(candidates[0].type_hash, TypeHash::zero().to_rihs_string());
        let Entity::Endpoint(endpoint) = publisher.as_ref() else {
            panic!("expected publisher endpoint");
        };
        let advertised = endpoint.type_info.as_ref().unwrap();
        assert_eq!(advertised.name, wire_name);
        assert_eq!(advertised.hash.to_rihs_string(), candidates[0].type_hash);

        let discovered = crate::dynamic::DiscoveredTopicSchema {
            qualified_topic: "/chatter".to_string(),
            schema: MessageSchema::builder("example_interfaces/msg/AddTwoIntsRequest")
                .field("a", FieldType::Int64)
                .build()
                .unwrap(),
            transport_type_name: wire_name.to_string(),
            type_hash: candidates[0].type_hash.clone(),
        };
        let subscriber_type = crate::dynamic::discovered_schema_type_info(&discovered);
        assert_eq!(subscriber_type.name, advertised.name);
        assert_eq!(subscriber_type.hash, advertised.hash);
    }

    #[test]
    fn test_topic_discovery_reports_missing_node_identity_only_when_all_publishers_lack_it() {
        let publishers = vec![publisher_entity(None, Some("std_msgs::msg::dds_::String_"))];

        let err = collect_topic_schema_candidates_from_endpoints(&publishers, "/chatter")
            .expect_err("expected missing node identity error");

        assert!(matches!(
            err,
            DynamicError::MissingNodeIdentity { ref topic } if topic == "/chatter"
        ));
    }
}
