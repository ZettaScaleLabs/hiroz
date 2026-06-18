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
