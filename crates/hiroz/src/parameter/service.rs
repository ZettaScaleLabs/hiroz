//! Parameter service implementation.
//!
//! Creates the 6 parameter service servers and the /parameter_events publisher
//! for a node. Follows the same pattern as TypeDescriptionService.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use tracing::{debug, info, warn};
use zenoh::{Result as ZResult, Session, query::Query};

use super::{
    store::ParameterStore,
    types::{Parameter, ParameterDescriptor, ParameterValue, SetParametersResult},
    wire_types::{
        self, DescribeParametersSrv, GetParameterTypesSrv, GetParametersSrv, ListParametersSrv,
        SetParametersAtomicallySrv, SetParametersSrv, WireParameter, WireParameterEvent,
        WireSetParametersResult, WireTime, parameter_event_type_info,
    },
};
use crate::{
    Builder, ServiceTypeInfo,
    attachment::Attachment,
    context::GlobalCounter,
    entity::{EndpointEntity, EndpointKind, NodeEntity, TypeInfo},
    msg::{SerdeCdrSerdes, ZDeserializer, ZSerializer},
    pubsub::{ZPub, ZPubBuilder},
    qos::{QosDurability, QosHistory, QosProfile, QosReliability},
    service::ZServerBuilder,
};

type SetCallback = Arc<dyn Fn(&[Parameter]) -> SetParametersResult + Send + Sync>;

/// Holds a type-erased ZServer to keep it alive without naming the full type.
type BoxedServer = Arc<dyn std::any::Any + Send + Sync>;

pub(crate) struct ParameterServiceConfig<'a> {
    pub session: Arc<Session>,
    pub graph: Arc<crate::graph::Graph>,
    pub node_name: &'a str,
    pub namespace: &'a str,
    pub node_id: usize,
    pub counter: &'a GlobalCounter,
    pub clock: &'a crate::time::ZClock,
    pub overrides: HashMap<String, ParameterValue>,
}

struct ParameterState {
    store: RwLock<ParameterStore>,
    on_set_callback: RwLock<Option<SetCallback>>,
    event_publisher: ZPub<WireParameterEvent, SerdeCdrSerdes<WireParameterEvent>>,
    node_fqn: String,
}

#[derive(Debug, Default)]
struct ParameterEventChanges {
    new_parameters: Vec<WireParameter>,
    changed_parameters: Vec<WireParameter>,
    deleted_parameters: Vec<WireParameter>,
}

impl ParameterEventChanges {
    fn record_declared(&mut self, parameter: &Parameter) {
        self.new_parameters.push(parameter.to_wire());
    }

    fn record_changed(&mut self, parameter: &Parameter) {
        self.changed_parameters.push(parameter.to_wire());
    }

    fn record_deleted(&mut self, parameter: &Parameter) {
        self.deleted_parameters.push(parameter.to_wire());
    }

    fn is_empty(&self) -> bool {
        self.new_parameters.is_empty()
            && self.changed_parameters.is_empty()
            && self.deleted_parameters.is_empty()
    }
}

impl ParameterState {
    fn publish_changes(&self, changes: ParameterEventChanges) {
        if changes.is_empty() {
            return;
        }

        let event = WireParameterEvent {
            stamp: WireTime::default(),
            node: self.node_fqn.clone(),
            new_parameters: changes.new_parameters,
            changed_parameters: changes.changed_parameters,
            deleted_parameters: changes.deleted_parameters,
        };

        if let Err(e) = self.event_publisher.publish(&event) {
            warn!("[PARAMS] Failed to publish parameter event: {}", e);
        }
    }

    fn declare_parameter(
        &self,
        name: &str,
        default: ParameterValue,
        descriptor: ParameterDescriptor,
    ) -> Result<ParameterValue, String> {
        let initial = self
            .store
            .write()
            .map_err(|_| "parameter store lock poisoned".to_string())?
            .declare(name, default, descriptor)?;

        let mut changes = ParameterEventChanges::default();
        changes.record_declared(&Parameter::new(name, initial.clone()));
        self.publish_changes(changes);

        Ok(initial)
    }

    fn get_parameter(&self, name: &str) -> Option<ParameterValue> {
        self.store.read().ok()?.get(name)
    }

    fn describe_parameter(&self, name: &str) -> Option<ParameterDescriptor> {
        self.store.read().ok()?.describe(name)
    }

    fn list_parameters(
        &self,
        prefixes: &[String],
        depth: u64,
    ) -> wire_types::WireListParametersResult {
        self.store
            .read()
            .map(|store| store.list(prefixes, depth))
            .unwrap_or_default()
    }

    fn on_set_parameters<F>(&self, callback: F)
    where
        F: Fn(&[Parameter]) -> SetParametersResult + Send + Sync + 'static,
    {
        if let Ok(mut cb) = self.on_set_callback.write() {
            *cb = Some(Arc::new(callback));
        }
    }

    fn undeclare_parameter(&self, name: &str) -> Result<(), String> {
        let removed = self
            .store
            .write()
            .map_err(|_| "parameter store lock poisoned".to_string())?
            .undeclare(name)?;

        let mut changes = ParameterEventChanges::default();
        changes.record_deleted(&removed);
        self.publish_changes(changes);

        Ok(())
    }

    fn validate_and_apply(&self, params: &[Parameter], atomic: bool) -> Vec<SetParametersResult> {
        let mut results = Vec::with_capacity(params.len());

        {
            let store = match self.store.read() {
                Ok(store) => store,
                Err(_) => {
                    return params
                        .iter()
                        .map(|_| SetParametersResult::failure("store lock poisoned"))
                        .collect();
                }
            };

            for param in params {
                match store.validate_set(param) {
                    Ok(_) => results.push(SetParametersResult::success()),
                    Err(reason) => results.push(SetParametersResult::failure(reason)),
                }
            }
        }

        if let Ok(cb_guard) = self.on_set_callback.read()
            && let Some(cb) = cb_guard.as_ref()
        {
            if atomic {
                // Atomic: call callback once with all params; on rejection fail all.
                if results.iter().all(|r| r.successful) {
                    let cb_result = cb(params);
                    if !cb_result.successful {
                        return params
                            .iter()
                            .map(|_| SetParametersResult::failure(cb_result.reason.clone()))
                            .collect();
                    }
                }
            } else {
                // Non-atomic (ROS 2 spec): call callback once per parameter independently.
                for (i, param) in params.iter().enumerate() {
                    if results[i].successful {
                        let cb_result = cb(std::slice::from_ref(param));
                        if !cb_result.successful {
                            results[i] = SetParametersResult::failure(cb_result.reason);
                        }
                    }
                }
            }
        }

        if atomic && !results.iter().all(|result| result.successful) {
            return results;
        }

        let mut changes = ParameterEventChanges::default();

        match self.store.write() {
            Ok(mut store) => {
                for (index, param) in params.iter().enumerate() {
                    if !results[index].successful {
                        continue;
                    }

                    if store.set(param).is_some() {
                        changes.record_changed(param);
                    }
                }
            }
            Err(_) => {
                return params
                    .iter()
                    .map(|_| SetParametersResult::failure("store lock poisoned"))
                    .collect();
            }
        }

        self.publish_changes(changes);
        results
    }
}

/// Manages ROS 2 parameter services and event publication for a node.
#[derive(Clone)]
pub struct ParameterService {
    state: Arc<ParameterState>,
    /// Keeps all service servers alive.
    _servers: Arc<[BoxedServer]>,
}

impl ParameterService {
    /// Create a new ParameterService for the given node.
    ///
    /// Spawns 6 parameter service servers and creates the /parameter_events publisher.
    pub(crate) fn new(config: ParameterServiceConfig<'_>) -> ZResult<Self> {
        let ParameterServiceConfig {
            session,
            graph,
            node_name,
            namespace,
            node_id,
            counter,
            clock,
            overrides,
        } = config;
        let node_entity = NodeEntity::new(
            0,
            session.zid(),
            node_id,
            node_name.to_string(),
            namespace.to_string(),
            String::new(),
        );

        // Compute node fully-qualified name for parameter events
        let node_fqn = if namespace.is_empty() || namespace == "/" {
            format!("/{}", node_name)
        } else {
            format!("{}/{}", namespace, node_name)
        };

        // Helper to build a service server entity
        let make_entity =
            |counter: &GlobalCounter, service_name: &str, type_info: TypeInfo| EndpointEntity {
                id: counter.increment(),
                node: Some(node_entity.clone()),
                kind: EndpointKind::Service,
                topic: service_name.to_string(),
                type_info: Some(type_info),
                qos: Default::default(),
            };

        let ke_format = hiroz_protocol::KeyExprFormat::default();

        // ── /parameter_events publisher ───────────────────────────────────────
        let pub_entity = EndpointEntity {
            id: counter.increment(),
            node: Some(node_entity.clone()),
            kind: EndpointKind::Publisher,
            topic: "/parameter_events".to_string(),
            type_info: Some(parameter_event_type_info()),
            qos: {
                let qos = QosProfile {
                    reliability: QosReliability::Reliable,
                    durability: QosDurability::TransientLocal,
                    history: QosHistory::KeepLast(std::num::NonZeroUsize::new(1000).unwrap()),
                    ..Default::default()
                };
                qos.to_protocol_qos()
            },
        };

        let pub_builder: ZPubBuilder<WireParameterEvent, SerdeCdrSerdes<WireParameterEvent>> =
            ZPubBuilder {
                entity: pub_entity,
                session: session.clone(),
                graph: graph.clone(),
                clock: clock.clone(),
                with_attachment: true,
                shm_config: None,
                keyexpr_format: ke_format.clone(),
                dyn_schema: None,
                encoding: None,
                _phantom_data: Default::default(),
            };

        let state = Arc::new(ParameterState {
            store: RwLock::new(if overrides.is_empty() {
                ParameterStore::new()
            } else {
                ParameterStore::with_overrides(overrides)
            }),
            on_set_callback: RwLock::new(None),
            event_publisher: pub_builder.build()?,
            node_fqn,
        });

        // ── describe_parameters ───────────────────────────────────────────────
        let describe_state = state.clone();
        let describe_server = {
            let entity = make_entity(
                counter,
                "~describe_parameters",
                DescribeParametersSrv::service_type_info(),
            );
            let builder: ZServerBuilder<DescribeParametersSrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_describe_parameters(describe_state.as_ref(), query);
            })?
        };

        // ── get_parameters ───────────────────────────────────────────────────
        let get_state = state.clone();
        let get_server = {
            let entity = make_entity(
                counter,
                "~get_parameters",
                GetParametersSrv::service_type_info(),
            );
            let builder: ZServerBuilder<GetParametersSrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_get_parameters(get_state.as_ref(), query);
            })?
        };

        // ── get_parameter_types ───────────────────────────────────────────────
        let types_state = state.clone();
        let types_server = {
            let entity = make_entity(
                counter,
                "~get_parameter_types",
                GetParameterTypesSrv::service_type_info(),
            );
            let builder: ZServerBuilder<GetParameterTypesSrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_get_parameter_types(types_state.as_ref(), query);
            })?
        };

        // ── list_parameters ───────────────────────────────────────────────────
        let list_state = state.clone();
        let list_server = {
            let entity = make_entity(
                counter,
                "~list_parameters",
                ListParametersSrv::service_type_info(),
            );
            let builder: ZServerBuilder<ListParametersSrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_list_parameters(list_state.as_ref(), query);
            })?
        };

        // ── set_parameters ────────────────────────────────────────────────────
        let set_state = state.clone();
        let set_server = {
            let entity = make_entity(
                counter,
                "~set_parameters",
                SetParametersSrv::service_type_info(),
            );
            let builder: ZServerBuilder<SetParametersSrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_set_parameters(set_state.as_ref(), query);
            })?
        };

        // ── set_parameters_atomically ─────────────────────────────────────────
        let atomic_state = state.clone();
        let atomic_server = {
            let entity = make_entity(
                counter,
                "~set_parameters_atomically",
                SetParametersAtomicallySrv::service_type_info(),
            );
            let builder: ZServerBuilder<SetParametersAtomicallySrv> = ZServerBuilder {
                entity,
                session: session.clone(),
                clock: clock.clone(),
                keyexpr_format: ke_format.clone(),
                _phantom_data: Default::default(),
            };
            builder.build_with_callback(move |query| {
                handle_set_parameters_atomically(atomic_state.as_ref(), query);
            })?
        };

        info!(
            "[PARAMS] ParameterService created for node: {}/{}",
            namespace, node_name
        );

        let servers: Arc<[BoxedServer]> = Arc::from(vec![
            Arc::new(describe_server) as BoxedServer,
            Arc::new(get_server) as BoxedServer,
            Arc::new(types_server) as BoxedServer,
            Arc::new(list_server) as BoxedServer,
            Arc::new(set_server) as BoxedServer,
            Arc::new(atomic_server) as BoxedServer,
        ]);

        Ok(Self {
            state,
            _servers: servers,
        })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    pub fn declare_parameter(
        &self,
        name: &str,
        default: ParameterValue,
        descriptor: ParameterDescriptor,
    ) -> Result<ParameterValue, String> {
        self.state.declare_parameter(name, default, descriptor)
    }

    pub fn get_parameter(&self, name: &str) -> Option<ParameterValue> {
        self.state.get_parameter(name)
    }

    pub fn set_parameter(&self, param: Parameter) -> std::result::Result<(), String> {
        let result = self
            .state
            .validate_and_apply(std::slice::from_ref(&param), false)
            .into_iter()
            .next()
            .unwrap_or_else(|| SetParametersResult::failure("internal error: empty result"));
        if result.successful {
            Ok(())
        } else {
            Err(result.reason)
        }
    }

    pub fn undeclare_parameter(&self, name: &str) -> Result<(), String> {
        self.state.undeclare_parameter(name)
    }

    pub fn describe_parameter(&self, name: &str) -> Option<ParameterDescriptor> {
        self.state.describe_parameter(name)
    }

    pub fn list_parameters(
        &self,
        prefixes: &[String],
        depth: u64,
    ) -> wire_types::WireListParametersResult {
        self.state.list_parameters(prefixes, depth)
    }

    pub fn on_set_parameters<F>(&self, callback: F)
    where
        F: Fn(&[Parameter]) -> SetParametersResult + Send + Sync + 'static,
    {
        self.state.on_set_parameters(callback);
    }
}

// ── Service callback handlers ─────────────────────────────────────────────────

fn reply_with<T: serde::Serialize>(query: Query, response: &T) {
    let bytes = SerdeCdrSerdes::<T>::serialize(response);
    use zenoh::Wait;
    let mut reply = query.reply(query.key_expr().clone(), bytes);
    // Echo the request attachment back so rmw_zenoh_cpp can match response to request.
    if let Some(att_bytes) = query.attachment()
        && let Ok(att) = Attachment::try_from(att_bytes)
    {
        reply = reply.attachment(att);
    }
    if let Err(e) = reply.wait() {
        warn!("[PARAMS] Failed to send response: {}", e);
    }
}

fn deserialize_request<T: for<'de> serde::Deserialize<'de>>(query: &Query) -> Option<T> {
    let payload = query.payload()?;
    match SerdeCdrSerdes::<T>::deserialize(payload.to_bytes().as_ref()) {
        Ok(req) => Some(req),
        Err(e) => {
            warn!("[PARAMS] Failed to deserialize request: {}", e);
            None
        }
    }
}

fn handle_describe_parameters(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::DescribeParametersRequest>(&query) else {
        return;
    };
    debug!("[PARAMS] describe_parameters: {:?}", req.names);
    let descriptors = match state.store.read() {
        Ok(store) => store.describe_many(&req.names),
        Err(_) => return,
    };
    let response = wire_types::DescribeParametersResponse { descriptors };
    reply_with(query, &response);
}

fn handle_get_parameters(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::GetParametersRequest>(&query) else {
        return;
    };
    debug!("[PARAMS] get_parameters: {:?}", req.names);
    let values = match state.store.read() {
        Ok(store) => store.get_many(&req.names),
        Err(_) => return,
    };
    let response = wire_types::GetParametersResponse { values };
    reply_with(query, &response);
}

fn handle_get_parameter_types(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::GetParameterTypesRequest>(&query) else {
        return;
    };
    debug!("[PARAMS] get_parameter_types: {:?}", req.names);
    let types = match state.store.read() {
        Ok(store) => store.get_types(&req.names),
        Err(_) => return,
    };
    let response = wire_types::GetParameterTypesResponse { types };
    reply_with(query, &response);
}

fn handle_list_parameters(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::ListParametersRequest>(&query) else {
        return;
    };
    debug!(
        "[PARAMS] list_parameters: prefixes={:?}, depth={}",
        req.prefixes, req.depth
    );
    let result = match state.store.read() {
        Ok(store) => store.list(&req.prefixes, req.depth),
        Err(_) => return,
    };
    let response = wire_types::ListParametersResponse { result };
    reply_with(query, &response);
}

fn handle_set_parameters(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::SetParametersRequest>(&query) else {
        return;
    };
    debug!("[PARAMS] set_parameters: {} params", req.parameters.len());

    let params: Vec<Parameter> = req.parameters.iter().map(Parameter::from_wire).collect();
    let results = state.validate_and_apply(&params, false);

    let wire_results: Vec<WireSetParametersResult> = results.iter().map(|r| r.to_wire()).collect();
    let response = wire_types::SetParametersResponse {
        results: wire_results,
    };
    reply_with(query, &response);
}

fn handle_set_parameters_atomically(state: &ParameterState, query: Query) {
    let Some(req) = deserialize_request::<wire_types::SetParametersAtomicallyRequest>(&query)
    else {
        return;
    };
    debug!(
        "[PARAMS] set_parameters_atomically: {} params",
        req.parameters.len()
    );

    let params: Vec<Parameter> = req.parameters.iter().map(Parameter::from_wire).collect();
    let results = state.validate_and_apply(&params, true);

    // Atomically: overall success only if all succeeded
    let successful = results.iter().all(|r| r.successful);
    let reason = if successful {
        String::new()
    } else {
        results
            .iter()
            .find(|r| !r.successful)
            .map(|r| r.reason.clone())
            .unwrap_or_default()
    };
    let response = wire_types::SetParametersAtomicallyResponse {
        result: WireSetParametersResult { successful, reason },
    };
    reply_with(query, &response);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_changes_classify_declare_change_delete() {
        let mut changes = ParameterEventChanges::default();

        changes.record_declared(&Parameter::new("declared", ParameterValue::Integer(1)));
        changes.record_changed(&Parameter::new("changed", ParameterValue::NotSet));
        changes.record_deleted(&Parameter::new(
            "deleted",
            ParameterValue::String("gone".into()),
        ));

        assert_eq!(changes.new_parameters.len(), 1);
        assert_eq!(changes.changed_parameters.len(), 1);
        assert_eq!(changes.deleted_parameters.len(), 1);
        assert_eq!(changes.changed_parameters[0].name, "changed");
        assert_eq!(
            changes.changed_parameters[0].value.r#type,
            wire_types::parameter_type::NOT_SET
        );
        assert_eq!(changes.deleted_parameters[0].name, "deleted");
    }
}
