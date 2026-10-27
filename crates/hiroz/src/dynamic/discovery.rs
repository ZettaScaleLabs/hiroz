use std::time::Duration;
use std::{collections::BTreeSet, sync::Arc};
use std::{future::Future, task::Poll};

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
    /// Exact advertised type name used by the publisher's transport key.
    /// This can differ from the canonical name in the schema.
    pub transport_type_name: String,
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
    pub transport_type_name: String,
    pub type_hash: String,
}

pub(crate) fn collect_topic_schema_candidates_from_endpoints(
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
            transport_type_name: type_info.name.clone(),
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
            "No endpoints with type information found for topic: {}",
            qualified_topic
        )));
    }

    Err(DynamicError::SchemaNotFound(format!(
        "No usable endpoints found for topic: {}",
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

    collect_topic_schema_candidates_from_endpoints(&publishers, qualified_topic)
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
        let deadline = tokio::time::Instant::now().checked_add(self.timeout);
        let qualified_topic = qualify_topic_name(topic, self.node.namespace(), self.node.name())
            .map_err(|error| {
                DynamicError::SchemaNotFound(format!("Failed to qualify topic: {error}"))
            })?;

        let graph = self.node.graph();
        if !graph
            .wait_for_publisher_until(&qualified_topic, deadline)
            .await
        {
            return Err(DynamicError::SchemaNotFound(format!(
                "No publishers found for topic: {}",
                qualified_topic
            )));
        }

        let candidates = collect_topic_schema_candidates(graph.as_ref(), &qualified_topic)?;
        let (schema, type_hash, transport_type_name) =
            self.try_standard(&candidates, deadline).await?;

        Ok(DiscoveredTopicSchema {
            qualified_topic,
            schema,
            transport_type_name,
            type_hash,
        })
    }

    /// Like [`discover`], but resolves the schema from a **subscriber** when no
    /// publisher is present. `discover` requires a live publisher (which defines
    /// the on-wire type); this variant additionally accepts a subscriber's
    /// advertised type, so a topic that only has a consumer — the usual target
    /// of `hu meter pub` — is still resolvable. A publisher is still preferred
    /// when one exists.
    ///
    /// [`discover`]: SchemaDiscovery::discover
    pub(crate) async fn discover_including_subscribers(
        &self,
        topic: &str,
    ) -> Result<DiscoveredTopicSchema, DynamicError> {
        let deadline = tokio::time::Instant::now().checked_add(self.timeout);
        let qualified_topic = qualify_topic_name(topic, self.node.namespace(), self.node.name())
            .map_err(|error| {
                DynamicError::SchemaNotFound(format!("Failed to qualify topic: {error}"))
            })?;

        let graph = self.node.graph();
        // Prefer a publisher already present (it defines the wire type); else a
        // subscriber already present; else wait for a publisher to appear, and
        // fall back to a subscriber one last time. This keeps the common case
        // (a consumer is already there) fast rather than always waiting the full
        // publisher timeout.
        let endpoints = {
            let pubs = graph.get_entities_by_topic(EndpointKind::Publisher, &qualified_topic);
            if !pubs.is_empty() {
                pubs
            } else {
                let subs =
                    graph.get_entities_by_topic(EndpointKind::Subscription, &qualified_topic);
                if !subs.is_empty() {
                    subs
                } else if graph
                    .wait_for_publisher_until(&qualified_topic, deadline)
                    .await
                {
                    graph.get_entities_by_topic(EndpointKind::Publisher, &qualified_topic)
                } else {
                    let subs =
                        graph.get_entities_by_topic(EndpointKind::Subscription, &qualified_topic);
                    if subs.is_empty() {
                        return Err(DynamicError::SchemaNotFound(format!(
                            "No publishers or subscribers found for topic: {}",
                            qualified_topic
                        )));
                    }
                    subs
                }
            }
        };

        let candidates =
            collect_topic_schema_candidates_from_endpoints(&endpoints, &qualified_topic)?;
        let (schema, type_hash, transport_type_name) =
            self.try_standard(&candidates, deadline).await?;

        Ok(DiscoveredTopicSchema {
            qualified_topic,
            schema,
            transport_type_name,
            type_hash,
        })
    }

    async fn try_standard(
        &self,
        candidates: &[TopicSchemaCandidate],
        deadline: Option<tokio::time::Instant>,
    ) -> Result<(Arc<MessageSchema>, String, String), DynamicError> {
        let mut last_error = None;
        // One borrowed future per deduplicated candidate; no detached tasks or
        // retries. A missing description service must not starve another source.
        // Returning the first success drops all remaining queriers/listeners.
        let mut pending: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                Box::pin(async move {
                    let (schema, hash) = super::type_description_client::query_type_description(
                        self.node, candidate, deadline, false,
                    )
                    .await?;
                    Ok((schema, hash, candidate.transport_type_name.clone()))
                })
            })
            .collect();
        std::future::poll_fn(|cx| {
            let mut index = 0;
            while index < pending.len() {
                match pending[index].as_mut().poll(cx) {
                    Poll::Ready(Ok(discovered)) => return Poll::Ready(Ok(discovered)),
                    Poll::Ready(Err(error)) => {
                        drop(pending.swap_remove(index));
                        last_error = Some(error);
                    }
                    Poll::Pending => index += 1,
                }
            }
            if pending.is_empty() {
                Poll::Ready(Err(last_error.take().unwrap_or_else(|| {
                    DynamicError::SchemaNotFound("No standard schema source succeeded".to_string())
                })))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

pub(crate) fn discovered_schema_type_info(
    discovered: &DiscoveredTopicSchema,
) -> crate::entity::TypeInfo {
    let mut info = schema_type_info_with_hash(&discovered.schema, &discovered.type_hash);
    info.name = discovered.transport_type_name.clone();
    info
}
