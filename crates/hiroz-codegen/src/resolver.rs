use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use anyhow::{Result, bail};

use crate::{
    hashing::{TypeDescription, calculate_type_hash, is_primitive_type},
    types::{ParsedMessage, ParsedService, ResolvedMessage, ResolvedService, TypeHash},
};

pub struct Resolver {
    /// Map of package/name -> TypeDescription for resolved types
    type_descriptions: BTreeMap<String, TypeDescription>,
    /// Map of package/name -> ResolvedMessage for fully resolved messages
    resolved_messages: HashMap<String, ResolvedMessage>,
    /// Humble compatibility mode (no ServiceEventInfo, placeholder type hashes)
    is_humble: bool,
    /// External packages whose types are assumed resolved (from hiroz_msgs)
    /// Types from these packages are treated as resolved without needing type descriptions
    external_packages: HashSet<String>,
}

impl Resolver {
    /// Create a new resolver
    pub fn new(is_humble: bool) -> Self {
        Self {
            type_descriptions: BTreeMap::new(),
            resolved_messages: HashMap::new(),
            is_humble,
            external_packages: HashSet::new(),
        }
    }

    /// Create a resolver with external packages
    /// External packages are treated as already resolved (types come from another crate)
    pub fn with_external_packages(is_humble: bool, external_packages: HashSet<String>) -> Self {
        Self {
            type_descriptions: BTreeMap::new(),
            resolved_messages: HashMap::new(),
            is_humble,
            external_packages,
        }
    }

    /// Check if a package is external (types come from another crate)
    fn is_external_package(&self, package: &str) -> bool {
        self.external_packages.contains(package)
    }

    /// Resolve a list of messages, calculating their type hashes and dependencies
    pub fn resolve_messages(
        &mut self,
        messages: Vec<ParsedMessage>,
    ) -> Result<Vec<ResolvedMessage>> {
        let mut queue = VecDeque::from(messages);
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10000;
        let mut last_queue_size = queue.len();
        let mut stagnant_iterations = 0;

        while let Some(msg) = queue.pop_front() {
            if iterations >= MAX_ITERATIONS {
                // Provide detailed error with stuck messages
                let stuck_messages: Vec<String> = queue
                    .iter()
                    .take(10)
                    .map(|m| format!("{}/{}", m.package, m.name))
                    .collect();

                bail!(
                    "Exceeded maximum iterations ({}) - possible circular dependency or missing type.\n\
                     Still unresolved: {} messages\n\
                     First 10 stuck messages: {:?}",
                    MAX_ITERATIONS,
                    queue.len() + 1,
                    stuck_messages
                );
            }
            iterations += 1;

            // Check if all dependencies are resolved
            if self.all_deps_resolved(&msg) {
                let resolved = self.resolve_message(msg)?;
                let key = self.message_key(&resolved.parsed);

                // Store type description so other messages can depend on this one
                let type_desc = resolved.type_description();
                self.type_descriptions.insert(key.clone(), type_desc);

                self.resolved_messages.insert(key, resolved);

                // Reset stagnation counter on progress
                stagnant_iterations = 0;
            } else {
                // Push back to queue to try again later
                queue.push_back(msg);
            }

            // Detect stagnation (no progress being made)
            if queue.len() == last_queue_size {
                stagnant_iterations += 1;
                if stagnant_iterations > 100 {
                    // We've gone through the queue 100 times without resolving anything
                    let stuck_messages: Vec<String> = queue
                        .iter()
                        .take(10)
                        .map(|m| {
                            let unresolved_deps: Vec<String> = m
                                .fields
                                .iter()
                                .filter(|f| {
                                    !is_primitive_type(&f.field_type.base_type)
                                        && !self.is_type_resolved(
                                            &f.field_type.package,
                                            &f.field_type.base_type,
                                        )
                                })
                                .map(|f| format!("{:?}", f.field_type))
                                .collect();
                            format!("{}/{} (missing: {:?})", m.package, m.name, unresolved_deps)
                        })
                        .collect();

                    bail!(
                        "Dependency resolution stalled after {} iterations.\n\
                         {} messages cannot be resolved.\n\
                         First 10 stuck messages with dependencies:\n{}",
                        iterations,
                        queue.len(),
                        stuck_messages.join("\n")
                    );
                }
            } else {
                last_queue_size = queue.len();
                stagnant_iterations = 0;
            }
        }

        Ok(self.resolved_messages.values().cloned().collect())
    }

    /// Resolve a single service (Request + Response + Event)
    pub fn resolve_service(&mut self, srv: ParsedService) -> Result<ResolvedService> {
        // First resolve request and response as standalone messages
        let request = self.resolve_message(srv.request.clone())?;
        let response = self.resolve_message(srv.response.clone())?;

        // Store their type descriptions for service hash calculation
        let request_key = self.message_key(&srv.request);
        let response_key = self.message_key(&srv.response);

        self.type_descriptions
            .insert(request_key.clone(), request.type_description().clone());
        self.type_descriptions
            .insert(response_key.clone(), response.type_description().clone());

        // For Humble or when we have external packages, use placeholder type hash
        // (no ServiceEventInfo support when types are external)
        let has_external_deps = !self.external_packages.is_empty();
        let type_hash = if self.is_humble || has_external_deps {
            TypeHash::zero()
        } else {
            // Get ServiceEventInfo and its dependencies for service hash
            let service_event_info_desc = self
                .type_descriptions
                .get("service_msgs/ServiceEventInfo")
                .ok_or_else(|| anyhow::anyhow!("ServiceEventInfo not found in builtin types"))?;

            // Collect all dependencies needed for service hash calculation
            let mut service_deps = BTreeMap::new();

            // Add ServiceEventInfo and Time
            service_deps.insert(
                service_event_info_desc.type_name.clone(),
                service_event_info_desc.clone(),
            );
            if let Some(time_desc) = self.type_descriptions.get("builtin_interfaces/Time") {
                service_deps.insert(time_desc.type_name.clone(), time_desc.clone());
            }

            // Collect all nested types from request and response
            self.collect_nested_deps(&request.type_description(), &mut service_deps);
            self.collect_nested_deps(&response.type_description(), &mut service_deps);

            // Calculate proper service type hash (Request + Response + Event)
            crate::hashing::calculate_service_type_hash(
                &srv.package,
                &srv.name,
                &request.type_description(),
                &response.type_description(),
                service_event_info_desc,
                &service_deps,
            )?
        };

        Ok(ResolvedService {
            parsed: srv,
            request,
            response,
            type_hash,
        })
    }

    /// Resolve multiple services
    pub fn resolve_services(
        &mut self,
        services: Vec<ParsedService>,
    ) -> Result<Vec<ResolvedService>> {
        services
            .into_iter()
            .map(|srv| self.resolve_service(srv))
            .collect()
    }

    /// Resolve a single action (Goal + Result + Feedback)
    pub fn resolve_action(
        &mut self,
        action: crate::types::ParsedAction,
    ) -> Result<crate::types::ResolvedAction> {
        use crate::types::ResolvedAction;

        // Resolve goal, result, and feedback as standalone messages
        let goal = self.resolve_message(action.goal.clone())?;
        let result = action
            .result
            .as_ref()
            .map(|r| self.resolve_message(r.clone()))
            .transpose()?;
        let feedback = action
            .feedback
            .as_ref()
            .map(|f| self.resolve_message(f.clone()))
            .transpose()?;

        // Store their type descriptions
        let goal_key = self.message_key(&action.goal);
        let result_key = action.result.as_ref().map(|r| self.message_key(r));
        let feedback_key = action.feedback.as_ref().map(|f| self.message_key(f));

        self.type_descriptions
            .insert(goal_key.clone(), goal.type_description().clone());
        if let (Some(result), Some(result_key)) = (&result, &result_key) {
            self.type_descriptions
                .insert(result_key.clone(), result.type_description().clone());
        }
        if let (Some(feedback), Some(feedback_key)) = (&feedback, &feedback_key) {
            self.type_descriptions
                .insert(feedback_key.clone(), feedback.type_description().clone());
        }

        // Calculate action type hash (use goal hash for now)
        // TODO: Implement proper action hash calculation
        let type_hash = goal.type_hash;

        // Calculate type hashes for action protocol services/messages
        // These follow ROS2 action protocol structure
        let send_goal_hash = self.calculate_send_goal_hash(&action, &goal)?;
        let get_result_hash = if let Some(ref result) = result {
            self.calculate_get_result_hash(&action, result)?
        } else {
            TypeHash([0u8; 32]) // Default for actions without result
        };
        let feedback_message_hash = if let Some(ref feedback) = feedback {
            self.calculate_feedback_message_hash(&action, feedback)?
        } else {
            TypeHash([0u8; 32]) // Default for actions without feedback
        };

        // Calculate standard ROS2 action protocol type hashes
        // These are shared by all actions (action_msgs package)
        let cancel_goal_hash = self.calculate_cancel_goal_hash()?;
        let status_hash = self.calculate_status_hash()?;

        Ok(ResolvedAction {
            parsed: action.clone(),
            goal,
            result,
            feedback,
            type_hash,
            send_goal_hash,
            get_result_hash,
            feedback_message_hash,
            cancel_goal_hash,
            status_hash,
        })
    }

    /// Resolve multiple actions
    pub fn resolve_actions(
        &mut self,
        actions: Vec<crate::types::ParsedAction>,
    ) -> Result<Vec<crate::types::ResolvedAction>> {
        actions
            .into_iter()
            .map(|action| self.resolve_action(action))
            .collect()
    }

    /// Check if all dependencies for a message are resolved
    fn all_deps_resolved(&self, msg: &ParsedMessage) -> bool {
        msg.fields.iter().all(|field| {
            is_primitive_type(&field.field_type.base_type)
                || self.is_type_resolved_in_context(
                    &field.field_type.package,
                    &field.field_type.base_type,
                    &msg.package,
                )
        })
    }

    /// Check if a type is already resolved
    fn is_type_resolved(&self, package: &Option<String>, base_type: &str) -> bool {
        if let Some(pkg) = package {
            let key = format!("{}/{}", pkg, base_type);
            self.type_descriptions.contains_key(&key)
        } else {
            false
        }
    }

    /// Check if a type is resolved, considering same-package references and external packages
    fn is_type_resolved_in_context(
        &self,
        package: &Option<String>,
        base_type: &str,
        source_package: &str,
    ) -> bool {
        if let Some(pkg) = package {
            // External packages are treated as already resolved
            if self.is_external_package(pkg) {
                return true;
            }
            let key = format!("{}/{}", pkg, base_type);
            self.type_descriptions.contains_key(&key)
        } else {
            // If package is None, assume it's in the same package as the source message
            let key = format!("{}/{}", source_package, base_type);
            self.type_descriptions.contains_key(&key)
        }
    }

    /// Collect all nested type dependencies from a type description
    fn collect_nested_deps(
        &self,
        type_desc: &TypeDescription,
        collected: &mut BTreeMap<String, TypeDescription>,
    ) {
        for field in &type_desc.fields {
            if !field.field_type.nested_type_name.is_empty() {
                // Convert "pkg/msg/Type" to "pkg/Type" key format
                let nested_name = &field.field_type.nested_type_name;
                let key = if nested_name.contains("/msg/") {
                    nested_name.replace("/msg/", "/")
                } else if nested_name.contains("/srv/") {
                    nested_name.replace("/srv/", "/")
                } else {
                    nested_name.clone()
                };

                if !collected.contains_key(nested_name)
                    && let Some(dep_desc) = self.type_descriptions.get(&key)
                {
                    collected.insert(dep_desc.type_name.clone(), dep_desc.clone());
                    // Recursively collect nested types
                    self.collect_nested_deps(dep_desc, collected);
                }
            }
        }
    }

    /// Resolve a single message
    fn resolve_message(&self, msg: ParsedMessage) -> Result<ResolvedMessage> {
        // Calculate type hash
        let type_hash = calculate_type_hash(&msg, &self.type_descriptions)?;

        // Expand full definition (include all nested types)
        let definition = self.expand_definition(&msg);

        Ok(ResolvedMessage {
            parsed: msg,
            type_hash,
            definition,
        })
    }

    /// Expand message definition to include all nested types
    fn expand_definition(&self, msg: &ParsedMessage) -> String {
        let mut definition = msg.source.clone();

        // Collect all unique nested types
        let mut nested_types = Vec::new();
        for field in &msg.fields {
            if let Some(ref pkg) = field.field_type.package {
                let key = format!("{}/{}", pkg, field.field_type.base_type);
                if !nested_types.contains(&key) {
                    nested_types.push(key);
                }
            }
        }

        // Append nested type definitions (recursive)
        for nested_key in nested_types {
            if let Some(resolved) = self.resolved_messages.get(&nested_key) {
                definition.push_str("\n\n");
                definition.push_str("================================================================================\n");
                definition.push_str(&format!("MSG: {}\n", nested_key));
                definition.push_str(&resolved.definition);
            }
        }

        definition
    }

    /// Get the key for a message (package/name)
    fn message_key(&self, msg: &ParsedMessage) -> String {
        format!("{}/{}", msg.package, msg.name)
    }

    /// Calculate type hash for action SendGoal service
    /// Request: goal_id (UUID) + goal
    /// Response: accepted (bool) + stamp (Time)
    fn calculate_send_goal_hash(
        &self,
        action: &crate::types::ParsedAction,
        goal: &crate::types::ResolvedMessage,
    ) -> Result<crate::types::TypeHash> {
        // For Humble, use placeholder hash (no ServiceEventInfo)
        if self.is_humble {
            return Ok(TypeHash::zero());
        }

        use crate::hashing::{FieldDescription, FieldTypeDescription, TypeDescription};

        // SendGoal_Request type description
        let request_fields = vec![
            FieldDescription {
                name: "goal_id".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "unique_identifier_msgs/msg/UUID".to_string(),
                },
                default_value: String::new(),
            },
            FieldDescription {
                name: "goal".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: format!("{}/action/{}_Goal", action.package, action.name),
                },
                default_value: String::new(),
            },
        ];

        let request_desc = TypeDescription {
            type_name: format!("{}/action/{}_SendGoal_Request", action.package, action.name),
            fields: request_fields,
        };

        // SendGoal_Response type description
        let response_fields = vec![
            FieldDescription {
                name: "accepted".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 15, // bool
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            },
            FieldDescription {
                name: "stamp".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "builtin_interfaces/msg/Time".to_string(),
                },
                default_value: String::new(),
            },
        ];

        let response_desc = TypeDescription {
            type_name: format!(
                "{}/action/{}_SendGoal_Response",
                action.package, action.name
            ),
            fields: response_fields,
        };

        // Get dependencies: goal type
        let mut deps = BTreeMap::new();

        // Get UUID from resolved type descriptions (key is package/name without /msg/)
        if let Some(uuid_desc) = self.type_descriptions.get("unique_identifier_msgs/UUID") {
            deps.insert(uuid_desc.type_name.clone(), uuid_desc.clone());
        }

        // Get Time (key is package/name without /msg/)
        if let Some(time_desc) = self.type_descriptions.get("builtin_interfaces/Time") {
            deps.insert(time_desc.type_name.clone(), time_desc.clone());
        }

        // Get ServiceEventInfo (key is package/name without /msg/)
        let service_event_info_desc = self
            .type_descriptions
            .get("service_msgs/ServiceEventInfo")
            .ok_or_else(|| anyhow::anyhow!("ServiceEventInfo not found in type descriptions"))?;
        deps.insert(
            service_event_info_desc.type_name.clone(),
            service_event_info_desc.clone(),
        );

        // Add goal type description
        let goal_desc = goal.type_description();
        deps.insert(goal_desc.type_name.clone(), goal_desc.clone());

        // Calculate action service hash (uses /action/ path instead of /srv/)
        let service_hash = crate::hashing::calculate_service_type_hash(
            &action.package,
            &format!("{}_SendGoal", action.name),
            &request_desc,
            &response_desc,
            service_event_info_desc,
            &deps,
        )?;

        Ok(service_hash)
    }

    /// Calculate type hash for action GetResult service
    /// Request: goal_id (UUID)
    /// Response: status (int8) + result
    fn calculate_get_result_hash(
        &self,
        action: &crate::types::ParsedAction,
        result: &crate::types::ResolvedMessage,
    ) -> Result<crate::types::TypeHash> {
        // For Humble, use placeholder hash (no ServiceEventInfo)
        if self.is_humble {
            return Ok(TypeHash::zero());
        }

        use crate::hashing::{FieldDescription, FieldTypeDescription, TypeDescription};

        // GetResult_Request type description
        let request_desc = TypeDescription {
            type_name: format!(
                "{}/action/{}_GetResult_Request",
                action.package, action.name
            ),
            fields: vec![FieldDescription {
                name: "goal_id".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "unique_identifier_msgs/msg/UUID".to_string(),
                },
                default_value: String::new(),
            }],
        };

        // GetResult_Response type description
        let response_fields = vec![
            FieldDescription {
                name: "status".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 2, // int8
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            },
            FieldDescription {
                name: "result".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: format!("{}/action/{}_Result", action.package, action.name),
                },
                default_value: String::new(),
            },
        ];

        let response_desc = TypeDescription {
            type_name: format!(
                "{}/action/{}_GetResult_Response",
                action.package, action.name
            ),
            fields: response_fields,
        };

        // Get dependencies
        let mut deps = BTreeMap::new();

        // Get UUID from resolved type descriptions (key is package/name without /msg/)
        if let Some(uuid_desc) = self.type_descriptions.get("unique_identifier_msgs/UUID") {
            deps.insert(uuid_desc.type_name.clone(), uuid_desc.clone());
        }

        // Get Time (key is package/name without /msg/)
        if let Some(time_desc) = self.type_descriptions.get("builtin_interfaces/Time") {
            deps.insert(time_desc.type_name.clone(), time_desc.clone());
        }

        // Get ServiceEventInfo (key is package/name without /msg/)
        let service_event_info_desc = self
            .type_descriptions
            .get("service_msgs/ServiceEventInfo")
            .ok_or_else(|| anyhow::anyhow!("ServiceEventInfo not found in type descriptions"))?;
        deps.insert(
            service_event_info_desc.type_name.clone(),
            service_event_info_desc.clone(),
        );

        // Add result type description
        let result_desc = result.type_description();
        deps.insert(result_desc.type_name.clone(), result_desc.clone());

        // Calculate action service hash
        let service_hash = crate::hashing::calculate_service_type_hash(
            &action.package,
            &format!("{}_GetResult", action.name),
            &request_desc,
            &response_desc,
            service_event_info_desc,
            &deps,
        )?;

        Ok(service_hash)
    }

    /// Calculate type hash for action feedback message
    fn calculate_feedback_message_hash(
        &self,
        action: &crate::types::ParsedAction,
        feedback: &crate::types::ResolvedMessage,
    ) -> Result<crate::types::TypeHash> {
        use crate::hashing::{FieldDescription, FieldTypeDescription, TypeDescription};

        // FeedbackMessage structure: goal_id + feedback
        let fields = vec![
            FieldDescription {
                name: "goal_id".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "unique_identifier_msgs/msg/UUID".to_string(),
                },
                default_value: String::new(),
            },
            FieldDescription {
                name: "feedback".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: format!("{}/action/{}_Feedback", action.package, action.name),
                },
                default_value: String::new(),
            },
        ];

        let type_name = format!("{}/action/{}_FeedbackMessage", action.package, action.name);
        let type_desc = TypeDescription { type_name, fields };

        // Get dependencies
        let mut deps = BTreeMap::new();

        // Get UUID from resolved type descriptions
        if let Some(uuid_desc) = self.type_descriptions.get("unique_identifier_msgs/UUID") {
            deps.insert(uuid_desc.type_name.clone(), uuid_desc.clone());
        }

        // Add feedback type description
        let feedback_desc = feedback.type_description();
        deps.insert(feedback_desc.type_name.clone(), feedback_desc);

        // Build TypeDescriptionMsg and calculate hash
        use crate::hashing::{TypeDescriptionMsg, to_hash_version, to_ros2_json};
        use sha2::Digest;

        // Sort dependencies alphabetically by type_name
        let mut referenced_vec: Vec<TypeDescription> = deps.into_values().collect();
        referenced_vec.sort_by(|a, b| a.type_name.cmp(&b.type_name));

        let type_desc_msg = TypeDescriptionMsg {
            type_description: type_desc,
            referenced_type_descriptions: referenced_vec,
        };

        let hash_version = to_hash_version(&type_desc_msg);
        let json = to_ros2_json(&hash_version)?;

        let mut hasher = sha2::Sha256::new();
        hasher.update(json.as_bytes());
        let hash_bytes: [u8; 32] = hasher.finalize().into();

        Ok(crate::types::TypeHash(hash_bytes))
    }

    /// Calculate type hash for CancelGoal service (action_msgs/srv/CancelGoal)
    /// This is a standard ROS2 action protocol type shared by all actions
    fn calculate_cancel_goal_hash(&self) -> Result<crate::types::TypeHash> {
        // For Humble, use placeholder hash (no ServiceEventInfo)
        if self.is_humble {
            return Ok(TypeHash::zero());
        }

        use crate::hashing::{FieldDescription, FieldTypeDescription, TypeDescription};

        // CancelGoal_Request: goal_info (GoalInfo)
        let request_desc = TypeDescription {
            type_name: "action_msgs/srv/CancelGoal_Request".to_string(),
            fields: vec![FieldDescription {
                name: "goal_info".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 1, // NESTED_TYPE
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "action_msgs/msg/GoalInfo".to_string(),
                },
                default_value: String::new(),
            }],
        };

        // CancelGoal_Response: return_code (int8)
        let response_desc = TypeDescription {
            type_name: "action_msgs/srv/CancelGoal_Response".to_string(),
            fields: vec![FieldDescription {
                name: "return_code".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 2, // int8
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            }],
        };

        // Get dependencies
        let mut deps = BTreeMap::new();

        // Get GoalInfo from resolved types
        if let Some(goal_info_desc) = self.type_descriptions.get("action_msgs/GoalInfo") {
            deps.insert(goal_info_desc.type_name.clone(), goal_info_desc.clone());
        }

        // Get Time (GoalInfo contains Time)
        if let Some(time_desc) = self.type_descriptions.get("builtin_interfaces/Time") {
            deps.insert(time_desc.type_name.clone(), time_desc.clone());
        }

        // Get ServiceEventInfo
        let service_event_info_desc = self
            .type_descriptions
            .get("service_msgs/ServiceEventInfo")
            .ok_or_else(|| anyhow::anyhow!("ServiceEventInfo not found in type descriptions"))?;
        deps.insert(
            service_event_info_desc.type_name.clone(),
            service_event_info_desc.clone(),
        );

        // Calculate service hash
        let service_hash = crate::hashing::calculate_service_type_hash(
            "action_msgs",
            "CancelGoal",
            &request_desc,
            &response_desc,
            service_event_info_desc,
            &deps,
        )?;

        Ok(service_hash)
    }

    /// Calculate type hash for GoalStatusArray message (action_msgs/msg/GoalStatusArray)
    /// This is a standard ROS2 action protocol type shared by all actions
    fn calculate_status_hash(&self) -> Result<crate::types::TypeHash> {
        use crate::hashing::{
            FieldDescription, FieldTypeDescription, TypeDescription, TypeDescriptionMsg,
            to_hash_version, to_ros2_json,
        };
        use sha2::Digest;

        // GoalStatusArray: status_list (GoalStatus[])
        let type_desc = TypeDescription {
            type_name: "action_msgs/msg/GoalStatusArray".to_string(),
            fields: vec![FieldDescription {
                name: "status_list".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 145, // NESTED_TYPE_BOUNDED_SEQUENCE (capacity=0 means unbounded)
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: "action_msgs/msg/GoalStatus".to_string(),
                },
                default_value: String::new(),
            }],
        };

        // Manually construct dependencies to match ROS2 exactly
        let mut deps = BTreeMap::new();

        // Manually construct GoalInfo
        let goal_info_desc = TypeDescription {
            type_name: "action_msgs/msg/GoalInfo".to_string(),
            fields: vec![
                FieldDescription {
                    name: "goal_id".to_string(),
                    field_type: FieldTypeDescription {
                        type_id: 1, // NESTED_TYPE
                        capacity: 0,
                        string_capacity: 0,
                        nested_type_name: "unique_identifier_msgs/msg/UUID".to_string(),
                    },
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "stamp".to_string(),
                    field_type: FieldTypeDescription {
                        type_id: 1, // NESTED_TYPE
                        capacity: 0,
                        string_capacity: 0,
                        nested_type_name: "builtin_interfaces/msg/Time".to_string(),
                    },
                    default_value: String::new(),
                },
            ],
        };
        deps.insert(goal_info_desc.type_name.clone(), goal_info_desc);

        // Manually construct GoalStatus
        let goal_status_desc = TypeDescription {
            type_name: "action_msgs/msg/GoalStatus".to_string(),
            fields: vec![
                FieldDescription {
                    name: "goal_info".to_string(),
                    field_type: FieldTypeDescription {
                        type_id: 1, // NESTED_TYPE
                        capacity: 0,
                        string_capacity: 0,
                        nested_type_name: "action_msgs/msg/GoalInfo".to_string(),
                    },
                    default_value: String::new(),
                },
                FieldDescription {
                    name: "status".to_string(),
                    field_type: FieldTypeDescription {
                        type_id: 2, // INT8
                        capacity: 0,
                        string_capacity: 0,
                        nested_type_name: String::new(),
                    },
                    default_value: String::new(),
                },
            ],
        };
        deps.insert(goal_status_desc.type_name.clone(), goal_status_desc);

        // Get Time
        if let Some(time_desc) = self.type_descriptions.get("builtin_interfaces/Time") {
            deps.insert(time_desc.type_name.clone(), time_desc.clone());
        }

        // Manually construct UUID
        let uuid_desc = TypeDescription {
            type_name: "unique_identifier_msgs/msg/UUID".to_string(),
            fields: vec![FieldDescription {
                name: "uuid".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 51, // UINT8_BOUNDED_ARRAY
                    capacity: 16,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            }],
        };
        deps.insert(uuid_desc.type_name.clone(), uuid_desc);

        // Sort dependencies alphabetically by type_name
        let mut referenced_vec: Vec<TypeDescription> = deps.into_values().collect();
        referenced_vec.sort_by(|a, b| a.type_name.cmp(&b.type_name));

        let type_desc_msg = TypeDescriptionMsg {
            type_description: type_desc,
            referenced_type_descriptions: referenced_vec,
        };

        let hash_version = to_hash_version(&type_desc_msg);
        let json = to_ros2_json(&hash_version)?;

        let mut hasher = sha2::Sha256::new();
        hasher.update(json.as_bytes());
        let hash_bytes: [u8; 32] = hasher.finalize().into();

        Ok(crate::types::TypeHash(hash_bytes))
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(false)
    }
}

/// Extension trait for ResolvedMessage to get TypeDescription
trait ResolvedMessageExt {
    fn type_description(&self) -> TypeDescription;
}

impl ResolvedMessageExt for ResolvedMessage {
    fn type_description(&self) -> TypeDescription {
        use crate::{
            hashing::{FieldDescription, FieldTypeDescription},
            types::ArrayType,
        };

        // Determine if this is an action component based on the source path.
        // Action components (Goal, Result, Feedback) are parsed from .action files, whose
        // paths contain "/action/". Regular messages (even those whose names end with
        // "Goal", "Result", or "Feedback", like ListParametersResult) are parsed from
        // .msg files, whose paths contain "/msg/".
        let is_action_component = self
            .parsed
            .path
            .to_str()
            .is_some_and(|p| p.contains("/action/"));

        let type_name = if is_action_component && self.parsed.name.ends_with("Goal") {
            let base = &self.parsed.name[..self.parsed.name.len() - 4];
            format!("{}/action/{}_Goal", self.parsed.package, base)
        } else if is_action_component && self.parsed.name.ends_with("Result") {
            let base = &self.parsed.name[..self.parsed.name.len() - 6];
            format!("{}/action/{}_Result", self.parsed.package, base)
        } else if is_action_component && self.parsed.name.ends_with("Feedback") {
            let base = &self.parsed.name[..self.parsed.name.len() - 8];
            format!("{}/action/{}_Feedback", self.parsed.package, base)
        } else {
            format!("{}/msg/{}", self.parsed.package, self.parsed.name)
        };
        let mut fields = Vec::new();

        // Handle empty messages
        if self.parsed.fields.is_empty() {
            fields.push(FieldDescription {
                name: "structure_needs_at_least_one_member".to_string(),
                field_type: FieldTypeDescription {
                    type_id: 3, // uint8
                    capacity: 0,
                    string_capacity: 0,
                    nested_type_name: String::new(),
                },
                default_value: String::new(),
            });
        } else {
            for field in &self.parsed.fields {
                let type_id = crate::hashing::get_type_id(&field.field_type).unwrap_or(0); // This shouldn't fail if message is resolved

                let capacity = match &field.field_type.array {
                    ArrayType::Fixed(n) | ArrayType::Bounded(n) => *n as u64,
                    _ => 0,
                };

                // Determine nested type path. Action components (Goal/Result/Feedback)
                // use the /action/ path with an underscore separator, but only when the
                // *current* message is itself an action component (path contains "/action/").
                // Regular messages — including those whose names happen to end with
                // "Goal", "Result", or "Feedback" — always use /msg/.
                let pkg = field
                    .field_type
                    .package
                    .as_ref()
                    .unwrap_or(&self.parsed.package);
                let nested_type_name =
                    if crate::hashing::is_primitive_type(&field.field_type.base_type) {
                        String::new()
                    } else if is_action_component && field.field_type.base_type.ends_with("Goal") {
                        let base =
                            &field.field_type.base_type[..field.field_type.base_type.len() - 4];
                        format!("{}/action/{}_Goal", pkg, base)
                    } else if is_action_component && field.field_type.base_type.ends_with("Result")
                    {
                        let base =
                            &field.field_type.base_type[..field.field_type.base_type.len() - 6];
                        format!("{}/action/{}_Result", pkg, base)
                    } else if is_action_component
                        && field.field_type.base_type.ends_with("Feedback")
                    {
                        let base =
                            &field.field_type.base_type[..field.field_type.base_type.len() - 8];
                        format!("{}/action/{}_Feedback", pkg, base)
                    } else {
                        format!("{}/msg/{}", pkg, field.field_type.base_type)
                    };

                let string_capacity = field.field_type.string_bound.unwrap_or(0) as u64;

                fields.push(FieldDescription {
                    name: field.name.clone(),
                    field_type: FieldTypeDescription {
                        type_id,
                        capacity,
                        string_capacity,
                        nested_type_name,
                    },
                    default_value: String::new(),
                });
            }
        }

        TypeDescription { type_name, fields }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::{ArrayType, Field, FieldType};

    #[test]
    fn test_resolver_new() {
        let resolver = Resolver::new(false);
        // Resolver starts empty - builtin types come from assets
        assert!(resolver.type_descriptions.is_empty());
        assert!(resolver.resolved_messages.is_empty());
    }

    #[test]
    fn test_resolve_simple_message() {
        let msg = ParsedMessage {
            name: "Simple".to_string(),
            package: "test_msgs".to_string(),
            fields: vec![Field {
                name: "value".to_string(),
                field_type: FieldType {
                    base_type: "int32".to_string(),
                    package: None,
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
            source: "int32 value".to_string(),
            path: PathBuf::new(),
        };

        let mut resolver = Resolver::new(false);
        let resolved = resolver.resolve_messages(vec![msg]).unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].parsed.name, "Simple");
        assert_eq!(resolved[0].type_hash.0.len(), 32);
    }

    #[test]
    fn test_resolve_empty_message() {
        let msg = ParsedMessage {
            name: "Empty".to_string(),
            package: "test_msgs".to_string(),
            fields: vec![],
            constants: vec![],
            source: "".to_string(),
            path: PathBuf::new(),
        };

        let mut resolver = Resolver::new(false);
        let resolved = resolver.resolve_messages(vec![msg]).unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].parsed.name, "Empty");
    }

    #[test]
    fn test_resolve_with_builtin_dependency() {
        // First, create the Time message that Stamped depends on
        let time_msg = ParsedMessage {
            name: "Time".to_string(),
            package: "builtin_interfaces".to_string(),
            fields: vec![
                Field {
                    name: "sec".to_string(),
                    field_type: FieldType {
                        base_type: "int32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "nanosec".to_string(),
                    field_type: FieldType {
                        base_type: "uint32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
            source: "int32 sec\nuint32 nanosec".to_string(),
            path: PathBuf::new(),
        };

        let msg = ParsedMessage {
            name: "Stamped".to_string(),
            package: "test_msgs".to_string(),
            fields: vec![
                Field {
                    name: "timestamp".to_string(),
                    field_type: FieldType {
                        base_type: "Time".to_string(),
                        package: Some("builtin_interfaces".to_string()),
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "data".to_string(),
                    field_type: FieldType {
                        base_type: "int32".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
            source: "builtin_interfaces/Time timestamp\nint32 data".to_string(),
            path: PathBuf::new(),
        };

        let mut resolver = Resolver::new(false);
        // Resolve both messages - Time first, then Stamped
        let resolved = resolver.resolve_messages(vec![time_msg, msg]).unwrap();

        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|r| r.parsed.name == "Stamped"));
    }

    #[test]
    fn test_resolve_nested_messages() {
        // Create Point message
        let point = ParsedMessage {
            name: "Point".to_string(),
            package: "geometry_msgs".to_string(),
            fields: vec![
                Field {
                    name: "x".to_string(),
                    field_type: FieldType {
                        base_type: "float64".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
                Field {
                    name: "y".to_string(),
                    field_type: FieldType {
                        base_type: "float64".to_string(),
                        package: None,
                        array: ArrayType::Single,
                        string_bound: None,
                    },
                    default: None,
                },
            ],
            constants: vec![],
            source: "float64 x\nfloat64 y".to_string(),
            path: PathBuf::new(),
        };

        // Create Pose message that depends on Point
        let pose = ParsedMessage {
            name: "Pose".to_string(),
            package: "geometry_msgs".to_string(),
            fields: vec![Field {
                name: "position".to_string(),
                field_type: FieldType {
                    base_type: "Point".to_string(),
                    package: Some("geometry_msgs".to_string()),
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
            source: "geometry_msgs/Point position".to_string(),
            path: PathBuf::new(),
        };

        let mut resolver = Resolver::new(false);
        let resolved = resolver.resolve_messages(vec![point, pose]).unwrap();

        assert_eq!(resolved.len(), 2);

        // Find the Pose message
        let pose_resolved = resolved.iter().find(|r| r.parsed.name == "Pose").unwrap();
        assert!(
            pose_resolved
                .definition
                .contains("MSG: geometry_msgs/Point")
        );
    }

    #[test]
    fn test_hash_deterministic() {
        let msg = ParsedMessage {
            name: "Test".to_string(),
            package: "test_msgs".to_string(),
            fields: vec![Field {
                name: "value".to_string(),
                field_type: FieldType {
                    base_type: "int32".to_string(),
                    package: None,
                    array: ArrayType::Single,
                    string_bound: None,
                },
                default: None,
            }],
            constants: vec![],
            source: "int32 value".to_string(),
            path: PathBuf::new(),
        };

        let mut resolver1 = Resolver::new(false);
        let resolved1 = resolver1.resolve_messages(vec![msg.clone()]).unwrap();

        let mut resolver2 = Resolver::new(false);
        let resolved2 = resolver2.resolve_messages(vec![msg]).unwrap();

        assert_eq!(resolved1[0].type_hash, resolved2[0].type_hash);
    }
}
