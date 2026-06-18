use anyhow::Result;
use clap::{Args, Subcommand};
use hiroz::parameter::{Parameter, ParameterClient, ParameterTarget, ParameterValue};

use crate::context::Ctx;

#[derive(Args)]
pub struct ParamArgs {
    #[command(subcommand)]
    pub action: ParamAction,
}

#[derive(Subcommand)]
pub enum ParamAction {
    /// List parameters for a node
    List {
        /// Fully-qualified node name (e.g. /talker)
        node: String,
        /// Optional prefix filter
        #[arg(long)]
        filter: Option<String>,
    },
    /// Get parameter value(s)
    Get {
        /// Fully-qualified node name
        node: String,
        /// Parameter name(s)
        names: Vec<String>,
    },
    /// Set a parameter value
    Set {
        /// Fully-qualified node name
        node: String,
        /// Parameter name
        name: String,
        /// Value (parsed as int, float, bool, or string)
        value: String,
    },
    /// Describe parameters
    Describe {
        /// Fully-qualified node name
        node: String,
        /// Parameter name(s)
        names: Vec<String>,
    },
    /// Dump all parameters to YAML (compatible with ros2 param dump)
    Dump {
        /// Fully-qualified node name
        node: String,
    },
    /// Load parameters from a YAML file (compatible with ros2 param dump output)
    Load {
        /// Fully-qualified node name
        node: String,
        /// Path to YAML file
        file: String,
    },
}

fn param_value_to_yaml(v: &ParameterValue) -> serde_yaml::Value {
    match v {
        ParameterValue::NotSet => serde_yaml::Value::Null,
        ParameterValue::Bool(b) => serde_yaml::Value::Bool(*b),
        ParameterValue::Integer(i) => serde_yaml::Value::Number((*i).into()),
        ParameterValue::Double(d) => {
            serde_yaml::Value::Number(serde_yaml::Number::from((*d) as f64))
        }
        ParameterValue::String(s) => serde_yaml::Value::String(s.clone()),
        ParameterValue::BoolArray(a) => {
            serde_yaml::Value::Sequence(a.iter().map(|b| serde_yaml::Value::Bool(*b)).collect())
        }
        ParameterValue::IntegerArray(a) => serde_yaml::Value::Sequence(
            a.iter()
                .map(|i| serde_yaml::Value::Number((*i).into()))
                .collect(),
        ),
        ParameterValue::DoubleArray(a) => serde_yaml::Value::Sequence(
            a.iter()
                .map(|d| serde_yaml::Value::Number(serde_yaml::Number::from(*d as f64)))
                .collect(),
        ),
        ParameterValue::StringArray(a) => serde_yaml::Value::Sequence(
            a.iter()
                .map(|s| serde_yaml::Value::String(s.clone()))
                .collect(),
        ),
        ParameterValue::ByteArray(b) => serde_yaml::Value::Sequence(
            b.iter()
                .map(|byte| serde_yaml::Value::Number((*byte as i64).into()))
                .collect(),
        ),
    }
}

fn yaml_to_param_value(v: &serde_yaml::Value) -> Result<ParameterValue> {
    match v {
        serde_yaml::Value::Null => Ok(ParameterValue::NotSet),
        serde_yaml::Value::Bool(b) => Ok(ParameterValue::Bool(*b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(ParameterValue::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(ParameterValue::Double(f))
            } else {
                anyhow::bail!("Unrepresentable number")
            }
        }
        serde_yaml::Value::String(s) => Ok(ParameterValue::String(s.clone())),
        serde_yaml::Value::Sequence(seq) => {
            // Infer array type from first element
            if seq.is_empty() {
                return Ok(ParameterValue::StringArray(vec![]));
            }
            match &seq[0] {
                serde_yaml::Value::Bool(_) => Ok(ParameterValue::BoolArray(
                    seq.iter()
                        .map(|v| v.as_bool().ok_or_else(|| anyhow::anyhow!("Mixed array")))
                        .collect::<Result<_>>()?,
                )),
                serde_yaml::Value::Number(n) if n.as_i64().is_some() => {
                    Ok(ParameterValue::IntegerArray(
                        seq.iter()
                            .map(|v| v.as_i64().ok_or_else(|| anyhow::anyhow!("Mixed array")))
                            .collect::<Result<_>>()?,
                    ))
                }
                serde_yaml::Value::Number(_) => Ok(ParameterValue::DoubleArray(
                    seq.iter()
                        .map(|v| v.as_f64().ok_or_else(|| anyhow::anyhow!("Mixed array")))
                        .collect::<Result<_>>()?,
                )),
                serde_yaml::Value::String(_) => Ok(ParameterValue::StringArray(
                    seq.iter()
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .ok_or_else(|| anyhow::anyhow!("Mixed array"))
                        })
                        .collect::<Result<_>>()?,
                )),
                _ => anyhow::bail!("Unsupported array element type"),
            }
        }
        _ => anyhow::bail!("Unsupported YAML value type for parameter"),
    }
}

/// Extract ros__parameters map from a YAML document.
/// Accepts both ros2 param dump format:
///   /node_name:
///     ros__parameters:
///       key: value
/// and flat format:
///   key: value
fn extract_ros_params<'a>(
    doc: &'a serde_yaml::Value,
    node_name: &str,
) -> Result<&'a serde_yaml::Mapping> {
    if let serde_yaml::Value::Mapping(root) = doc {
        // Try ros2 dump format: look up by node name
        for key in [node_name, node_name.trim_start_matches('/')] {
            if let Some(node_val) = root.get(key) {
                if let serde_yaml::Value::Mapping(node_map) = node_val {
                    if let Some(rp) = node_map.get("ros__parameters") {
                        if let serde_yaml::Value::Mapping(params) = rp {
                            return Ok(params);
                        }
                    }
                }
            }
        }
        // Fall back: treat the entire mapping as flat params
        return Ok(root);
    }
    anyhow::bail!("YAML document must be a mapping")
}

fn parse_node(fqn: &str) -> Result<ParameterTarget> {
    ParameterTarget::from_fqn(fqn).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid node name '{}'. Use format /namespace/node_name or /node_name",
            fqn
        )
    })
}

fn parse_value(s: &str) -> ParameterValue {
    if let Ok(i) = s.parse::<i64>() {
        return ParameterValue::Integer(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return ParameterValue::Double(f);
    }
    match s.to_lowercase().as_str() {
        "true" => return ParameterValue::Bool(true),
        "false" => return ParameterValue::Bool(false),
        _ => {}
    }
    ParameterValue::String(s.to_string())
}

fn value_to_json(v: &ParameterValue) -> serde_json::Value {
    match v {
        ParameterValue::NotSet => serde_json::Value::Null,
        ParameterValue::Bool(b) => serde_json::json!(b),
        ParameterValue::Integer(i) => serde_json::json!(i),
        ParameterValue::Double(d) => serde_json::json!(d),
        ParameterValue::String(s) => serde_json::json!(s),
        ParameterValue::BoolArray(a) => serde_json::json!(a),
        ParameterValue::IntegerArray(a) => serde_json::json!(a),
        ParameterValue::DoubleArray(a) => serde_json::json!(a),
        ParameterValue::StringArray(a) => serde_json::json!(a),
        ParameterValue::ByteArray(b) => serde_json::json!(b),
    }
}

fn value_to_string(v: &ParameterValue) -> String {
    match v {
        ParameterValue::NotSet => "(not set)".into(),
        ParameterValue::Bool(b) => b.to_string(),
        ParameterValue::Integer(i) => i.to_string(),
        ParameterValue::Double(d) => d.to_string(),
        ParameterValue::String(s) => s.clone(),
        ParameterValue::BoolArray(a) => format!("{:?}", a),
        ParameterValue::IntegerArray(a) => format!("{:?}", a),
        ParameterValue::DoubleArray(a) => format!("{:?}", a),
        ParameterValue::StringArray(a) => format!("{:?}", a),
        ParameterValue::ByteArray(b) => format!("{} bytes", b.len()),
    }
}

pub async fn run(ctx: &Ctx, args: ParamArgs, json: bool) -> Result<()> {
    match args.action {
        ParamAction::List { node, filter } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let prefixes: Vec<String> = filter.into_iter().collect();
            let params = client
                .list(&prefixes, None)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if json {
                println!("{}", serde_json::to_string_pretty(&params.names)?);
            } else {
                for name in &params.names {
                    println!("{}", name);
                }
            }
        }

        ParamAction::Get { node, names } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let values = client
                .get(&names)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if json {
                let map: serde_json::Map<_, _> = names
                    .iter()
                    .zip(values.iter())
                    .map(|(n, v)| (n.clone(), value_to_json(v)))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for (name, value) in names.iter().zip(values.iter()) {
                    println!("{}: {}", name, value_to_string(value));
                }
            }
        }

        ParamAction::Set { node, name, value } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let pv = parse_value(&value);
            let results = client
                .set(&[Parameter::new(&name, pv)])
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            for r in &results {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"successful": r.successful, "reason": r.reason})
                    );
                } else if r.successful {
                    println!("Set {}", name);
                } else {
                    anyhow::bail!("Failed to set {}: {}", name, r.reason);
                }
            }
        }

        ParamAction::Dump { node } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let list_result = client
                .list(&[] as &[String], None)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if list_result.names.is_empty() {
                // ros2 param dump format: node_name:\n  ros__parameters:\n    {}
                let fqn = node.trim_start_matches('/');
                println!("/{}:", fqn);
                println!("  ros__parameters: {{}}");
                return Ok(());
            }

            let values = client
                .get(&list_result.names)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            // Build YAML map matching ros2 param dump format:
            // /node_name:
            //   ros__parameters:
            //     param1: value1
            let mut params_map = serde_yaml::Mapping::new();
            for (name, value) in list_result.names.iter().zip(values.iter()) {
                let yaml_val = param_value_to_yaml(value);
                params_map.insert(serde_yaml::Value::String(name.clone()), yaml_val);
            }
            let mut ros_params = serde_yaml::Mapping::new();
            ros_params.insert(
                serde_yaml::Value::String("ros__parameters".into()),
                serde_yaml::Value::Mapping(params_map),
            );
            let mut root = serde_yaml::Mapping::new();
            root.insert(
                serde_yaml::Value::String(node.clone()),
                serde_yaml::Value::Mapping(ros_params),
            );
            print!("{}", serde_yaml::to_string(&root)?);
        }

        ParamAction::Load { node, file } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let content = std::fs::read_to_string(&file)?;
            let doc: serde_yaml::Value = serde_yaml::from_str(&content)?;

            // Accept both ros2 param dump format (/node: ros__parameters: {...})
            // and a flat key: value mapping.
            let params_map = extract_ros_params(&doc, &node)?;

            let mut params = Vec::new();
            for (k, v) in params_map {
                let name = k
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid parameter key"))?;
                let pv = yaml_to_param_value(v)?;
                params.push(Parameter::new(name, pv));
            }

            let results = client
                .set(&params)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let mut any_failed = false;
            for r in &results {
                if !r.successful {
                    eprintln!("Failed to set parameter: {}", r.reason);
                    any_failed = true;
                }
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({"loaded": results.len(), "failed": any_failed})
                );
            } else {
                println!("Loaded {} parameter(s)", results.len());
            }
            if any_failed {
                anyhow::bail!("One or more parameters failed to set");
            }
        }

        ParamAction::Describe { node, names } => {
            let target = parse_node(&node)?;
            let client = ParameterClient::new(ctx.node.clone(), target)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let descs = client
                .describe(&names)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            if json {
                let entries: Vec<_> = descs
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "name": d.name,
                            "type": format!("{:?}", d.type_),
                            "description": d.description,
                            "read_only": d.read_only,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for d in &descs {
                    println!("{}:", d.name);
                    println!("  Type:        {:?}", d.type_);
                    println!("  Description: {}", d.description);
                    println!("  Read-only:   {}", d.read_only);
                }
            }
        }
    }

    Ok(())
}
