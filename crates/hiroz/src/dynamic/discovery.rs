use std::time::Duration;
use std::{collections::BTreeSet, sync::Arc};

use crate::{
    dynamic::{DynamicError, MessageSchema},
    entity::{EndpointKind, Entity},
    graph::Graph,
    node::ZNode,
    topic_name::qualify_topic_name,
};

use super::type_info::{ros_type_name_from_dds, schema_type_info_with_hash};

#[derive(Debug, Clone)]
pub struct DiscoveredTopicSchema {
    pub qualified_topic: String,
    pub schema: Arc<MessageSchema>,
    /// RIHS01 type hash string as reported by the publishing node (e.g. `"RIHS01_abcd..."`).
    ///
    /// Uses the remote hash rather than recomputing locally so the subscriber's key expression
    /// matches the publisher's exact hash even when local recomputation would differ.
    pub type_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TopicSchemaCandidate {
    pub node_name: String,
    pub namespace: String,
    pub type_name: String,
    pub type_hash: String,
}

pub(crate) fn collect_topic_schema_candidates_from_publishers(
    publishers: &[Arc<Entity>],
    qualified_topic: &str,
) -> Result<Vec<TopicSchemaCandidate>, DynamicError> {
    let mut saw_missing_node_identity = false;
    let mut saw_missing_type_info = false;
    let mut candidates = BTreeSet::new();

    for publisher in publishers {
        let Entity::Endpoint(endpoint) = &**publisher else {
            continue;
        };
        let Some(node) = endpoint.node.as_ref() else {
            saw_missing_node_identity = true;
            continue;
        };
        let Some(type_info) = endpoint.type_info.as_ref() else {
            saw_missing_type_info = true;
            continue;
        };

        candidates.insert(TopicSchemaCandidate {
            node_name: node.name.clone(),
            namespace: node.namespace.clone(),
            type_name: ros_type_name_from_dds(&type_info.name),
            type_hash: type_info.hash.to_rihs_string(),
        });
    }

    if !candidates.is_empty() {
        return Ok(candidates.into_iter().collect());
    }

    if saw_missing_node_identity {
        return Err(DynamicError::MissingNodeIdentity {
            topic: qualified_topic.to_string(),
        });
    }

    if saw_missing_type_info {
        return Err(DynamicError::SchemaNotFound(format!(
            "No publishers with type information found for topic: {}",
            qualified_topic
        )));
    }

    Err(DynamicError::SchemaNotFound(format!(
        "No usable publishers found for topic: {}",
        qualified_topic
    )))
}

pub(crate) fn collect_topic_schema_candidates(
    graph: &Graph,
    qualified_topic: &str,
) -> Result<Vec<TopicSchemaCandidate>, DynamicError> {
    let publishers = graph.get_entities_by_topic(EndpointKind::Publisher, qualified_topic);
    if publishers.is_empty() {
        return Err(DynamicError::SchemaNotFound(format!(
            "No publishers found for topic: {}",
            qualified_topic
        )));
    }

    collect_topic_schema_candidates_from_publishers(&publishers, qualified_topic)
}

pub(crate) struct SchemaDiscovery<'a> {
    node: &'a ZNode,
    timeout: Duration,
}

impl<'a> SchemaDiscovery<'a> {
    pub(crate) fn new(node: &'a ZNode, timeout: Duration) -> Self {
        Self { node, timeout }
    }

    pub(crate) async fn discover(
        &self,
        topic: &str,
    ) -> Result<DiscoveredTopicSchema, DynamicError> {
        let qualified_topic = qualify_topic_name(topic, self.node.namespace(), self.node.name())
            .map_err(|error| {
                DynamicError::SchemaNotFound(format!("Failed to qualify topic: {error}"))
            })?;

        let graph = self.node.graph();
        if !graph
            .wait_for_publisher(&qualified_topic, self.timeout)
            .await
        {
            return Err(DynamicError::SchemaNotFound(format!(
                "No publishers found for topic: {}",
                qualified_topic
            )));
        }

        let candidates = collect_topic_schema_candidates(graph.as_ref(), &qualified_topic)?;
        let (schema, type_hash) = self.try_standard(&candidates[..]).await?;

        Ok(DiscoveredTopicSchema {
            qualified_topic,
            schema,
            type_hash,
        })
    }

    async fn try_standard(
        &self,
        candidates: &[TopicSchemaCandidate],
    ) -> Result<(Arc<MessageSchema>, String), DynamicError> {
        let mut last_error = None;

        for candidate in candidates {
            match super::type_description_client::query_type_description(
                self.node,
                candidate,
                self.timeout,
                false,
            )
            .await
            {
                Ok(result) => return Ok(result),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            DynamicError::SchemaNotFound("No standard schema source succeeded".to_string())
        }))
    }
}

pub(crate) fn discovered_schema_type_info(
    discovered: &DiscoveredTopicSchema,
) -> crate::entity::TypeInfo {
    schema_type_info_with_hash(&discovered.schema, &discovered.type_hash)
}
