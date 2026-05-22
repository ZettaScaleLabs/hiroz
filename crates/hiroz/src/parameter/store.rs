//! Parameter storage with validation.
//!
//! `ParameterStore` holds all declared parameters for a node. It enforces
//! type constraints, range checks, and read-only restrictions.

use std::collections::HashMap;

use super::{
    types::{
        FloatingPointRange, IntegerRange, Parameter, ParameterDescriptor, ParameterType,
        ParameterValue,
    },
    wire_types::{self, WireListParametersResult},
};

/// Internal storage entry for a single parameter.
#[derive(Debug, Clone)]
pub(crate) struct ParameterEntry {
    pub value: ParameterValue,
    pub descriptor: ParameterDescriptor,
}

/// Parameter store holding all declared parameters for a node.
#[derive(Debug)]
pub(crate) struct ParameterStore {
    parameters: HashMap<String, ParameterEntry>,
    /// Parameter overrides applied at declaration time.
    overrides: HashMap<String, ParameterValue>,
}

impl ParameterStore {
    pub fn new() -> Self {
        Self {
            parameters: HashMap::new(),
            overrides: HashMap::new(),
        }
    }

    pub fn with_overrides(overrides: HashMap<String, ParameterValue>) -> Self {
        Self {
            parameters: HashMap::new(),
            overrides,
        }
    }

    /// Declare a parameter with a default value and descriptor.
    ///
    /// If an override exists for this parameter name, the override value is used
    /// instead of the default. Returns the actual initial value.
    pub fn declare(
        &mut self,
        name: &str,
        default: ParameterValue,
        mut descriptor: ParameterDescriptor,
    ) -> Result<ParameterValue, String> {
        if self.parameters.contains_key(name) {
            return Err(format!("Parameter '{}' already declared", name));
        }

        // Use override if available, otherwise use default
        let initial_value = self.overrides.remove(name).unwrap_or(default);

        // Infer type from value if descriptor type is NotSet
        if descriptor.type_ == ParameterType::NotSet {
            descriptor.type_ = initial_value.parameter_type();
        }

        // Ensure descriptor name matches
        descriptor.name = name.to_string();

        // Validate the initial value against the descriptor
        if let Err(reason) = validate_value(&initial_value, &descriptor) {
            return Err(format!(
                "Invalid initial value for parameter '{}': {}",
                name, reason
            ));
        }

        let entry = ParameterEntry {
            value: initial_value.clone(),
            descriptor,
        };
        self.parameters.insert(name.to_string(), entry);

        Ok(initial_value)
    }

    /// Get the value of a parameter.
    pub fn get(&self, name: &str) -> Option<ParameterValue> {
        self.parameters.get(name).map(|e| e.value.clone())
    }

    /// Get the descriptor of a parameter.
    pub fn describe(&self, name: &str) -> Option<ParameterDescriptor> {
        self.parameters.get(name).map(|e| e.descriptor.clone())
    }

    /// Get the type of a parameter as a u8.
    pub fn get_type(&self, name: &str) -> u8 {
        self.parameters
            .get(name)
            .map(|e| u8::from(e.value.parameter_type()))
            .unwrap_or(super::wire_types::parameter_type::NOT_SET)
    }

    /// Validate a parameter change without committing it.
    ///
    /// Returns Ok(is_new) where is_new indicates whether this is a new
    /// parameter (true) or a change to an existing one (false).
    pub fn validate_set(&self, param: &Parameter) -> Result<bool, String> {
        match self.parameters.get(&param.name) {
            Some(entry) => {
                // Read-only check
                if entry.descriptor.read_only {
                    return Err(format!("Parameter '{}' is read-only", param.name));
                }

                // Type check (unless dynamic_typing is enabled)
                if !entry.descriptor.dynamic_typing
                    && entry.descriptor.type_ != ParameterType::NotSet
                    && param.value.parameter_type() != entry.descriptor.type_
                    && param.value.parameter_type() != ParameterType::NotSet
                {
                    return Err(format!(
                        "Parameter '{}': type mismatch, expected {:?} but got {:?}",
                        param.name,
                        entry.descriptor.type_,
                        param.value.parameter_type()
                    ));
                }

                // Range validation
                validate_value(&param.value, &entry.descriptor)?;

                Ok(false)
            }
            None => {
                // Parameter not declared — reject
                Err(format!("Parameter '{}' not declared", param.name))
            }
        }
    }

    /// Set a parameter value. Must be validated first.
    pub fn set(&mut self, param: &Parameter) -> Option<ParameterValue> {
        if let Some(entry) = self.parameters.get_mut(&param.name) {
            let old = std::mem::replace(&mut entry.value, param.value.clone());
            Some(old)
        } else {
            None
        }
    }

    /// Undeclare a parameter and return the removed value.
    pub fn undeclare(&mut self, name: &str) -> Result<Parameter, String> {
        if let Some((removed_name, entry)) = self.parameters.remove_entry(name) {
            Ok(Parameter::new(removed_name, entry.value))
        } else {
            Err(format!("Parameter '{}' not declared", name))
        }
    }

    /// List parameters matching the given prefixes and depth.
    pub fn list(&self, prefixes: &[String], depth: u64) -> WireListParametersResult {
        let mut names = Vec::new();
        let mut result_prefixes = std::collections::HashSet::new();

        let use_all = prefixes.is_empty();

        // Pre-compute "prefix." strings once to avoid O(N×K) allocations.
        let prefixes_with_dot: Vec<String> = prefixes.iter().map(|p| format!("{}.", p)).collect();

        for param_name in self.parameters.keys() {
            let matches = if use_all {
                true
            } else {
                prefixes
                    .iter()
                    .zip(prefixes_with_dot.iter())
                    .any(|(prefix, dot)| {
                        prefix.is_empty()
                            || param_name == prefix
                            || param_name.starts_with(dot.as_str())
                    })
            };

            if !matches {
                continue;
            }

            // Check depth
            if depth != wire_types::DEPTH_RECURSIVE {
                let matching_prefix = if use_all {
                    ""
                } else {
                    prefixes
                        .iter()
                        .zip(prefixes_with_dot.iter())
                        .find(|(p, dot)| {
                            p.is_empty()
                                || param_name == p.as_str()
                                || param_name.starts_with(dot.as_str())
                        })
                        .map(|(p, _)| p.as_str())
                        .unwrap_or("")
                };

                let suffix = if matching_prefix.is_empty() {
                    param_name.as_str()
                } else if param_name == matching_prefix {
                    ""
                } else {
                    &param_name[matching_prefix.len() + 1..]
                };

                let param_depth = if suffix.is_empty() {
                    0
                } else {
                    suffix.matches('.').count() as u64 + 1
                };

                if param_depth > depth {
                    continue;
                }
            }

            names.push(param_name.clone());

            // Extract prefixes (everything before the last '.')
            if let Some(dot_pos) = param_name.rfind('.') {
                let prefix = &param_name[..dot_pos];
                result_prefixes.insert(prefix.to_string());
            }
        }

        names.sort();
        let mut prefixes_vec: Vec<String> = result_prefixes.into_iter().collect();
        prefixes_vec.sort();

        WireListParametersResult {
            names,
            prefixes: prefixes_vec,
        }
    }

    /// Get descriptors for the given parameter names.
    pub fn describe_many(&self, names: &[String]) -> Vec<wire_types::WireParameterDescriptor> {
        names
            .iter()
            .map(|name| {
                self.parameters
                    .get(name)
                    .map(|e| e.descriptor.to_wire())
                    .unwrap_or_else(|| wire_types::WireParameterDescriptor {
                        name: name.clone(),
                        ..Default::default()
                    })
            })
            .collect()
    }

    /// Get values for the given parameter names.
    pub fn get_many(&self, names: &[String]) -> Vec<wire_types::WireParameterValue> {
        names
            .iter()
            .map(|name| {
                self.parameters
                    .get(name)
                    .map(|e| e.value.to_wire())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Get types for the given parameter names.
    pub fn get_types(&self, names: &[String]) -> Vec<u8> {
        names.iter().map(|name| self.get_type(name)).collect()
    }
}

/// Validate a value against a parameter descriptor's constraints.
fn validate_value(value: &ParameterValue, descriptor: &ParameterDescriptor) -> Result<(), String> {
    // Type check
    if descriptor.type_ != ParameterType::NotSet
        && !descriptor.dynamic_typing
        && value.parameter_type() != ParameterType::NotSet
        && value.parameter_type() != descriptor.type_
    {
        return Err(format!(
            "expected type {:?}, got {:?}",
            descriptor.type_,
            value.parameter_type()
        ));
    }

    // Range checks
    if let Some(ref range) = descriptor.floating_point_range
        && let ParameterValue::Double(v) = value
    {
        validate_float_range(*v, range)?;
    }

    if let Some(ref range) = descriptor.integer_range
        && let ParameterValue::Integer(v) = value
    {
        validate_integer_range(*v, range)?;
    }

    Ok(())
}

fn validate_float_range(value: f64, range: &FloatingPointRange) -> Result<(), String> {
    if value < range.from_value || value > range.to_value {
        return Err(format!(
            "value {} out of range [{}, {}]",
            value, range.from_value, range.to_value
        ));
    }

    if range.step != 0.0 {
        let offset = value - range.from_value;
        let remainder = offset % range.step.abs();
        // Allow small floating point errors
        if remainder > 1e-9 && (range.step.abs() - remainder) > 1e-9 {
            // Also allow values equal to to_value (upper bound always valid)
            if (value - range.to_value).abs() > 1e-9 {
                return Err(format!(
                    "value {} not on step grid (from={}, step={})",
                    value, range.from_value, range.step
                ));
            }
        }
    }

    Ok(())
}

fn validate_integer_range(value: i64, range: &IntegerRange) -> Result<(), String> {
    if value < range.from_value || value > range.to_value {
        return Err(format!(
            "value {} out of range [{}, {}]",
            value, range.from_value, range.to_value
        ));
    }

    if range.step != 0 {
        let offset = (value - range.from_value).unsigned_abs();
        if !offset.is_multiple_of(range.step) {
            // Upper bound is always valid
            if value != range.to_value {
                return Err(format!(
                    "value {} not on step grid (from={}, step={})",
                    value, range.from_value, range.step
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_declare_and_get() {
        let mut store = ParameterStore::new();
        let desc = ParameterDescriptor::new("my_param", ParameterType::Integer);
        let val = store
            .declare("my_param", ParameterValue::Integer(42), desc)
            .unwrap();
        assert_eq!(val, ParameterValue::Integer(42));
        assert_eq!(store.get("my_param"), Some(ParameterValue::Integer(42)));
    }

    #[test]
    fn test_declare_duplicate_fails() {
        let mut store = ParameterStore::new();
        let desc = ParameterDescriptor::new("p", ParameterType::Bool);
        store
            .declare("p", ParameterValue::Bool(true), desc.clone())
            .unwrap();
        assert!(
            store
                .declare("p", ParameterValue::Bool(false), desc)
                .is_err()
        );
    }

    #[test]
    fn test_set_validates_type() {
        let mut store = ParameterStore::new();
        let desc = ParameterDescriptor::new("p", ParameterType::Integer);
        store
            .declare("p", ParameterValue::Integer(1), desc)
            .unwrap();

        // Same type should work
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(2)))
                .is_ok()
        );

        // Different type should fail
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::String("x".into())))
                .is_err()
        );
    }

    #[test]
    fn test_read_only() {
        let mut store = ParameterStore::new();
        let mut desc = ParameterDescriptor::new("p", ParameterType::Integer);
        desc.read_only = true;
        store
            .declare("p", ParameterValue::Integer(1), desc)
            .unwrap();

        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(2)))
                .is_err()
        );
    }

    #[test]
    fn test_integer_range() {
        let mut store = ParameterStore::new();
        let mut desc = ParameterDescriptor::new("p", ParameterType::Integer);
        desc.integer_range = Some(IntegerRange {
            from_value: 0,
            to_value: 10,
            step: 2,
        });
        store
            .declare("p", ParameterValue::Integer(0), desc)
            .unwrap();

        // On step: 0, 2, 4, 6, 8, 10
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(4)))
                .is_ok()
        );
        // Off step but at upper bound
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(10)))
                .is_ok()
        );
        // Off step
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(3)))
                .is_err()
        );
        // Out of range
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Integer(12)))
                .is_err()
        );
    }

    #[test]
    fn test_float_range() {
        let mut store = ParameterStore::new();
        let mut desc = ParameterDescriptor::new("p", ParameterType::Double);
        desc.floating_point_range = Some(FloatingPointRange {
            from_value: 0.0,
            to_value: 1.0,
            step: 0.0, // continuous
        });
        store
            .declare("p", ParameterValue::Double(0.5), desc)
            .unwrap();

        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Double(0.75)))
                .is_ok()
        );
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::Double(1.5)))
                .is_err()
        );
    }

    #[test]
    fn test_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("p".to_string(), ParameterValue::Integer(99));
        let mut store = ParameterStore::with_overrides(overrides);

        let desc = ParameterDescriptor::new("p", ParameterType::Integer);
        let val = store
            .declare("p", ParameterValue::Integer(1), desc)
            .unwrap();
        // Override wins over default
        assert_eq!(val, ParameterValue::Integer(99));
    }

    #[test]
    fn test_undeclare() {
        let mut store = ParameterStore::new();
        let desc = ParameterDescriptor::new("p", ParameterType::Bool);
        store
            .declare("p", ParameterValue::Bool(true), desc)
            .unwrap();
        assert_eq!(
            store.undeclare("p").unwrap(),
            Parameter::new("p", ParameterValue::Bool(true))
        );
        assert!(store.get("p").is_none());
        assert!(store.undeclare("p").is_err());
    }

    #[test]
    fn test_not_set_remains_declared() {
        let mut store = ParameterStore::new();
        let desc = ParameterDescriptor::new("p", ParameterType::Integer);
        store
            .declare("p", ParameterValue::Integer(1), desc)
            .unwrap();

        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::NotSet))
                .is_ok()
        );

        let old = store.set(&Parameter::new("p", ParameterValue::NotSet));
        assert_eq!(old, Some(ParameterValue::Integer(1)));
        assert!(store.get("p").is_some());
        assert_eq!(store.get("p"), Some(ParameterValue::NotSet));
    }

    #[test]
    fn test_list_parameters() {
        let mut store = ParameterStore::new();
        for name in &["a", "a.b", "a.b.c", "x.y"] {
            let desc = ParameterDescriptor::new(*name, ParameterType::Bool);
            store
                .declare(name, ParameterValue::Bool(true), desc)
                .unwrap();
        }

        // List all
        let result = store.list(&[], 0);
        assert_eq!(result.names.len(), 4);

        // List with prefix "a", depth 1
        let result = store.list(&["a".to_string()], 1);
        assert!(result.names.contains(&"a".to_string()));
        assert!(result.names.contains(&"a.b".to_string()));
        assert!(!result.names.contains(&"a.b.c".to_string()));

        // List with prefix "a", unlimited depth
        let result = store.list(&["a".to_string()], 0);
        assert_eq!(result.names.len(), 3);
    }

    #[test]
    fn test_dynamic_typing() {
        let mut store = ParameterStore::new();
        let mut desc = ParameterDescriptor::new("p", ParameterType::Integer);
        desc.dynamic_typing = true;
        store
            .declare("p", ParameterValue::Integer(1), desc)
            .unwrap();

        // Should allow changing type
        assert!(
            store
                .validate_set(&Parameter::new("p", ParameterValue::String("hello".into())))
                .is_ok()
        );
    }
}
