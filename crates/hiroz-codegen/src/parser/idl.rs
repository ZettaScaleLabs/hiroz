//! Conversion of ROS-generated IDL into the code generator's parsed types.

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use t4_idl_parser::expr::{
    AnnotationAndDef, AnnotationAppl, AnnotationApplParams, AnyDeclarator, ConstDcl, ConstExpr,
    ConstType, ConstrTypeDcl, Definition, Literal, Module, PrimitiveType, ScopedName, SequenceType,
    StringType, StructDcl, TemplateTypeSpec, TypeDcl, TypeSpec, Typedef, TypedefType, UnaryOpExpr,
    WStringType,
};

use crate::types::{ArrayType, Constant, DefaultValue, Field, FieldType, ParsedMessage};

/// A ROS IDL file after preprocessing its includes and converting its structs.
#[derive(Debug, Clone)]
pub struct ParsedIdl {
    pub package: String,
    pub kind: String,
    pub includes: Vec<String>,
    pub messages: Vec<ParsedIdlMessage>,
    /// Complete parser output, including annotations and constant expressions.
    pub definitions: Vec<AnnotationAndDef>,
    pub source: String,
    pub path: PathBuf,
}

/// One IDL struct and the canonical identities of its message-typed fields.
#[derive(Debug, Clone)]
pub struct ParsedIdlMessage {
    pub message: ParsedMessage,
    pub references: HashMap<String, String>,
    /// Parsed `@default` expressions keyed by field name.
    pub defaults: HashMap<String, ConstExpr>,
}

/// Parse a ROS-generated IDL file.
pub fn parse_idl_file(path: &Path) -> Result<ParsedIdl> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_idl_string(&source, path)
}

/// Parse ROS-generated IDL while retaining its includes and source metadata.
///
/// ROS IDL uses quoted `#include` directives. They are collected for the caller's
/// dependency resolver. Other preprocessor directives are rejected.
pub fn parse_idl_string(source: &str, path: &Path) -> Result<ParsedIdl> {
    let (body, includes) = extract_includes(source)?;
    let definitions = catch_unwind(AssertUnwindSafe(|| t4_idl_parser::parse(&body)))
        .map_err(|_| anyhow::anyhow!("IDL parser panicked while parsing {}", path.display()))?
        .map_err(|error| anyhow::anyhow!("Failed to parse {}: {error}", path.display()))?;

    let root = one_module(&definitions, "IDL document")?;
    let namespace = one_module(&root.definitions, &format!("module {}", root.id))?;
    ensure!(
        matches!(namespace.id.as_str(), "msg" | "srv" | "action"),
        "Unsupported ROS interface namespace '{}'; expected msg, srv, or action",
        namespace.id
    );

    let mut constants = Vec::new();
    let mut aliases = HashMap::new();
    let mut structs = Vec::new();
    collect_namespace_items(namespace, &mut constants, &mut aliases, &mut structs)?;

    let mut messages = Vec::with_capacity(structs.len());
    for structure in structs {
        ensure!(
            structure.inheritance.is_none(),
            "Struct inheritance is not supported for {}",
            structure.id
        );
        let mut fields = Vec::new();
        let mut references = HashMap::new();
        let mut defaults = HashMap::new();
        for member in &structure.members {
            let base = convert_type_spec(
                &member.type_spec,
                &root.id,
                &aliases,
                &constants,
                &mut Vec::new(),
            )?;
            let reference = reference_for_type_spec(
                &member.type_spec,
                &root.id,
                &namespace.id,
                &aliases,
                &mut Vec::new(),
            )?;
            let default_expression = find_default_expression(member.annotations.as_deref())?;
            let default = default_expression.and_then(|value| literal_default(value, &base).ok());
            for declarator in &member.declarators {
                let (name, dimensions) = declarator_parts(declarator);
                let field_type = apply_dimensions(base.clone(), dimensions, &constants)?;
                fields.push(Field {
                    name: name.to_string(),
                    field_type,
                    default: default.clone(),
                });
                if let Some(reference) = &reference {
                    references.insert(name.to_string(), reference.clone());
                }
                if let Some(default_expression) = default_expression {
                    defaults.insert(name.to_string(), default_expression.clone());
                }
            }
        }

        let message_constants = constants
            .iter()
            .filter(|(scope, _)| scope.first().is_some_and(|name| name == &structure.id))
            .map(|(_, constant)| convert_constant(constant))
            .collect::<Result<Vec<_>>>()?;
        messages.push(ParsedIdlMessage {
            message: ParsedMessage {
                name: structure.id.clone(),
                package: root.id.clone(),
                fields,
                constants: message_constants,
                source: source.to_string(),
                path: path.to_path_buf(),
            },
            references,
            defaults,
        });
    }

    ensure!(
        !messages.is_empty(),
        "IDL document contains no struct definitions"
    );
    Ok(ParsedIdl {
        package: root.id.clone(),
        kind: namespace.id.clone(),
        includes,
        messages,
        definitions,
        source: source.to_string(),
        path: path.to_path_buf(),
    })
}

fn extract_includes(source: &str) -> Result<(String, Vec<String>)> {
    let mut body = String::with_capacity(source.len());
    let mut includes = Vec::new();
    for (line_number, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let rest = rest.trim();
            ensure!(
                rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2,
                "Line {}: only quoted IDL includes are supported",
                line_number + 1
            );
            includes.push(rest[1..rest.len() - 1].to_string());
            body.push('\n');
        } else if trimmed.starts_with('#') {
            bail!(
                "Line {}: unsupported preprocessor directive: {}",
                line_number + 1,
                trimmed
            );
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    Ok((body, includes))
}

fn one_module<'a>(definitions: &'a [AnnotationAndDef], context: &str) -> Result<&'a Module> {
    ensure!(
        definitions.len() == 1,
        "{context} must contain exactly one module"
    );
    match &definitions[0].definition {
        Definition::Module(module) => Ok(module),
        _ => bail!("{context} must contain a module"),
    }
}

type ConstantTable = Vec<(Vec<String>, ConstDcl)>;
type AliasTable = HashMap<String, (TypedefType, Vec<ConstExpr>)>;

fn collect_namespace_items<'a>(
    namespace: &'a Module,
    constants: &mut ConstantTable,
    aliases: &mut AliasTable,
    structs: &mut Vec<&'a t4_idl_parser::expr::StructDef>,
) -> Result<()> {
    for item in &namespace.definitions {
        match &item.definition {
            Definition::Const(constant) => {
                constants.push((vec![constant.id.clone()], constant.clone()));
            }
            Definition::Type(TypeDcl::Typedef(alias)) => collect_alias(alias, aliases)?,
            Definition::Type(TypeDcl::ConstrType(ConstrTypeDcl::Struct(StructDcl::Def(
                structure,
            )))) => structs.push(structure),
            Definition::Module(module) if module.id.ends_with("_Constants") => {
                let owner = module.id.trim_end_matches("_Constants").to_string();
                for nested in &module.definitions {
                    match &nested.definition {
                        Definition::Const(constant) => {
                            constants
                                .push((vec![owner.clone(), constant.id.clone()], constant.clone()));
                        }
                        _ => bail!("Unsupported definition in constants module {}", module.id),
                    }
                }
            }
            Definition::Type(TypeDcl::ConstrType(ConstrTypeDcl::Struct(
                StructDcl::ForwardDcl(_),
            ))) => bail!("Struct forward declarations are not supported"),
            _ => bail!("Unsupported definition in ROS interface module: {item:?}"),
        }
    }
    Ok(())
}

fn collect_alias(alias: &Typedef, aliases: &mut AliasTable) -> Result<()> {
    for declarator in &alias.declarators {
        let (name, dimensions) = declarator_parts(declarator);
        ensure!(
            aliases
                .insert(
                    name.to_string(),
                    (alias.type_dcl.clone(), dimensions.to_vec())
                )
                .is_none(),
            "Duplicate typedef '{name}'"
        );
    }
    Ok(())
}

fn declarator_parts(declarator: &AnyDeclarator) -> (&str, &[ConstExpr]) {
    match declarator {
        AnyDeclarator::Simple(name) => (name, &[]),
        AnyDeclarator::Array(array) => (&array.id, &array.array_size),
    }
}

fn convert_type_spec(
    type_spec: &TypeSpec,
    package: &str,
    aliases: &AliasTable,
    constants: &ConstantTable,
    alias_stack: &mut Vec<String>,
) -> Result<FieldType> {
    match type_spec {
        TypeSpec::PrimitiveType(primitive) => primitive_type(primitive),
        TypeSpec::Template(template) => {
            convert_template(template, package, aliases, constants, alias_stack)
        }
        TypeSpec::ScopedName(name) => {
            let parts = scoped_parts(name);
            if parts.len() == 1
                && let Some((alias, dimensions)) = aliases.get(&parts[0])
            {
                ensure!(
                    !alias_stack.contains(&parts[0]),
                    "Recursive typedef involving '{}'",
                    parts[0]
                );
                alias_stack.push(parts[0].clone());
                let field_type = convert_typedef(alias, package, aliases, constants, alias_stack)?;
                alias_stack.pop();
                return apply_dimensions(field_type, dimensions, constants);
            }
            ensure!(!parts.is_empty(), "Empty scoped type name");
            let (referenced_package, base_type, explicitly_scoped): (&str, &str, bool) =
                if parts.len() == 3 {
                    ensure!(
                        matches!(parts[1].as_str(), "msg" | "srv" | "action"),
                        "Unsupported ROS interface namespace in scoped type: {}",
                        parts.join("::")
                    );
                    (&parts[0], &parts[2], true)
                } else if parts.len() == 1 {
                    (package, &parts[0], false)
                } else {
                    bail!("Unsupported scoped type name: {}", parts.join("::"));
                };
            Ok(FieldType {
                base_type: base_type.to_string(),
                package: explicitly_scoped.then(|| referenced_package.to_string()),
                array: ArrayType::Single,
                string_bound: None,
            })
        }
    }
}

fn convert_typedef(
    alias: &TypedefType,
    package: &str,
    aliases: &AliasTable,
    constants: &ConstantTable,
    alias_stack: &mut Vec<String>,
) -> Result<FieldType> {
    match alias {
        TypedefType::Simple(spec) => {
            convert_type_spec(spec, package, aliases, constants, alias_stack)
        }
        TypedefType::Template(template) => {
            convert_template(template, package, aliases, constants, alias_stack)
        }
        TypedefType::Constr(_) => bail!("Constructed typedefs are not supported"),
    }
}

fn reference_for_type_spec(
    type_spec: &TypeSpec,
    package: &str,
    kind: &str,
    aliases: &AliasTable,
    alias_stack: &mut Vec<String>,
) -> Result<Option<String>> {
    match type_spec {
        TypeSpec::PrimitiveType(_) => Ok(None),
        TypeSpec::Template(template) => match template.as_ref() {
            TemplateTypeSpec::Sequence(SequenceType::Unlimited(element))
            | TemplateTypeSpec::Sequence(SequenceType::Limited(element, _)) => {
                reference_for_type_spec(element, package, kind, aliases, alias_stack)
            }
            _ => Ok(None),
        },
        TypeSpec::ScopedName(name) => {
            let parts = scoped_parts(name);
            if parts.len() == 1
                && let Some((alias, _)) = aliases.get(&parts[0])
            {
                ensure!(
                    !alias_stack.contains(&parts[0]),
                    "Recursive typedef involving '{}'",
                    parts[0]
                );
                alias_stack.push(parts[0].clone());
                let reference = match alias {
                    TypedefType::Simple(spec) => {
                        reference_for_type_spec(spec, package, kind, aliases, alias_stack)
                    }
                    TypedefType::Template(template) => match template {
                        TemplateTypeSpec::Sequence(SequenceType::Unlimited(element))
                        | TemplateTypeSpec::Sequence(SequenceType::Limited(element, _)) => {
                            reference_for_type_spec(element, package, kind, aliases, alias_stack)
                        }
                        _ => Ok(None),
                    },
                    TypedefType::Constr(_) => bail!("Constructed typedefs are not supported"),
                };
                alias_stack.pop();
                return reference;
            }
            if let [name] = parts.as_slice() {
                return Ok(Some(format!("{package}/{kind}/{name}")));
            }
            ensure!(
                parts.len() == 3,
                "Unsupported scoped type name: {}",
                parts.join("::")
            );
            let referenced_kind = &parts[1];
            ensure!(
                matches!(referenced_kind.as_str(), "msg" | "srv" | "action"),
                "Unsupported ROS interface namespace in scoped type: {}",
                parts.join("::")
            );
            Ok(Some(format!(
                "{}/{}/{}",
                parts[0], referenced_kind, parts[2]
            )))
        }
    }
}

fn convert_template(
    template: &TemplateTypeSpec,
    package: &str,
    aliases: &AliasTable,
    constants: &ConstantTable,
    alias_stack: &mut Vec<String>,
) -> Result<FieldType> {
    match template {
        TemplateTypeSpec::String(size) => string_type("string", size, constants),
        TemplateTypeSpec::WString(size) => wstring_type(size, constants),
        TemplateTypeSpec::Sequence(sequence) => {
            let (element, bound) = match sequence {
                SequenceType::Unlimited(element) => (element, None),
                SequenceType::Limited(element, bound) => {
                    (element, Some(eval_bound(bound, constants)?))
                }
            };
            let element = convert_type_spec(element, package, aliases, constants, alias_stack)?;
            ensure!(
                matches!(element.array, ArrayType::Single),
                "Sequences of array typedefs are not supported"
            );
            Ok(FieldType {
                base_type: element.base_type,
                package: element.package,
                array: bound.map_or(ArrayType::Unbounded, ArrayType::Bounded),
                string_bound: element.string_bound,
            })
        }
        _ => bail!("Unsupported IDL template type: {template:?}"),
    }
}

fn string_type(name: &str, size: &StringType, constants: &ConstantTable) -> Result<FieldType> {
    let string_bound = match size {
        StringType::UnlimitedSize => None,
        StringType::Sized(bound) => Some(eval_bound(bound, constants)?),
    };
    Ok(FieldType {
        base_type: name.to_string(),
        package: None,
        array: ArrayType::Single,
        string_bound,
    })
}

fn wstring_type(size: &WStringType, constants: &ConstantTable) -> Result<FieldType> {
    let string_bound = match size {
        WStringType::UnlimitedSize => None,
        WStringType::Sized(bound) => Some(eval_bound(bound, constants)?),
    };
    Ok(FieldType {
        base_type: "wstring".to_string(),
        package: None,
        array: ArrayType::Single,
        string_bound,
    })
}

fn primitive_type(primitive: &PrimitiveType) -> Result<FieldType> {
    let base_type = match primitive {
        PrimitiveType::Boolean => "bool",
        PrimitiveType::Char => "idl_char",
        PrimitiveType::WChar => "wchar",
        PrimitiveType::Octet => "byte",
        PrimitiveType::Short | PrimitiveType::Int16 => "int16",
        PrimitiveType::Long | PrimitiveType::Int32 => "int32",
        PrimitiveType::LongLong | PrimitiveType::Int64 => "int64",
        PrimitiveType::UnsignedShort | PrimitiveType::Uint16 => "uint16",
        PrimitiveType::UnsignedLong | PrimitiveType::Uint32 => "uint32",
        PrimitiveType::UnsignedLongLong | PrimitiveType::Uint64 => "uint64",
        PrimitiveType::Int8 => "int8",
        PrimitiveType::Uint8 => "uint8",
        PrimitiveType::Float => "float32",
        PrimitiveType::Double => "float64",
        PrimitiveType::LongDouble | PrimitiveType::Any => {
            bail!("Unsupported IDL primitive type: {primitive:?}")
        }
    };
    Ok(FieldType {
        base_type: base_type.to_string(),
        package: None,
        array: ArrayType::Single,
        string_bound: None,
    })
}

fn apply_dimensions(
    mut field_type: FieldType,
    dimensions: &[ConstExpr],
    constants: &ConstantTable,
) -> Result<FieldType> {
    if dimensions.is_empty() {
        return Ok(field_type);
    }
    ensure!(
        dimensions.len() == 1 && matches!(field_type.array, ArrayType::Single),
        "Multidimensional arrays are not supported"
    );
    field_type.array = ArrayType::Fixed(eval_bound(&dimensions[0], constants)?);
    Ok(field_type)
}

fn eval_bound(expr: &ConstExpr, constants: &ConstantTable) -> Result<usize> {
    let value = eval_integer(expr, constants, &mut Vec::new())?;
    ensure!(value >= 0, "IDL bound cannot be negative: {value}");
    usize::try_from(value).context("IDL bound does not fit usize")
}

fn eval_integer(
    expr: &ConstExpr,
    constants: &ConstantTable,
    stack: &mut Vec<Vec<String>>,
) -> Result<i128> {
    let mut binary =
        |left: &ConstExpr, right: &ConstExpr, op: fn(i128, i128) -> Option<i128>| -> Result<i128> {
            op(
                eval_integer(left, constants, stack)?,
                eval_integer(right, constants, stack)?,
            )
            .context("Integer overflow in IDL constant expression")
        };
    match expr {
        ConstExpr::Literal(Literal::Integer(value)) => value
            .to_string()
            .parse::<i128>()
            .context("IDL integer literal is outside the supported range"),
        ConstExpr::ScopedName(name) => {
            let parts = scoped_parts(name);
            ensure!(!stack.contains(&parts), "Recursive IDL constant expression");
            let constant = constants
                .iter()
                .find(|(name, _)| name == &parts)
                .map(|(_, constant)| constant)
                .or_else(|| {
                    parts.last().and_then(|last| {
                        constants
                            .iter()
                            .find(|(name, _)| name.len() == 1 && &name[0] == last)
                            .map(|(_, constant)| constant)
                    })
                })
                .with_context(|| format!("Unknown IDL constant {}", parts.join("::")))?;
            stack.push(parts);
            let value = eval_integer(&constant.expr, constants, stack);
            stack.pop();
            value
        }
        ConstExpr::Add(left, right) => binary(left, right, i128::checked_add),
        ConstExpr::Sub(left, right) => binary(left, right, i128::checked_sub),
        ConstExpr::Mul(left, right) => binary(left, right, i128::checked_mul),
        ConstExpr::Div(left, right) => binary(left, right, i128::checked_div),
        ConstExpr::Mod(left, right) => binary(left, right, i128::checked_rem),
        ConstExpr::And(left, right) => {
            Ok(eval_integer(left, constants, stack)? & eval_integer(right, constants, stack)?)
        }
        ConstExpr::Or(left, right) => {
            Ok(eval_integer(left, constants, stack)? | eval_integer(right, constants, stack)?)
        }
        ConstExpr::Xor(left, right) => {
            Ok(eval_integer(left, constants, stack)? ^ eval_integer(right, constants, stack)?)
        }
        ConstExpr::LShift(left, right) => {
            let shift = u32::try_from(eval_integer(right, constants, stack)?)?;
            eval_integer(left, constants, stack)?
                .checked_shl(shift)
                .context("Invalid shift in IDL constant expression")
        }
        ConstExpr::RShift(left, right) => {
            let shift = u32::try_from(eval_integer(right, constants, stack)?)?;
            eval_integer(left, constants, stack)?
                .checked_shr(shift)
                .context("Invalid shift in IDL constant expression")
        }
        ConstExpr::UnaryOp(UnaryOpExpr::Minus(inner)) => eval_integer(inner, constants, stack)?
            .checked_neg()
            .context("Integer overflow in IDL constant expression"),
        ConstExpr::UnaryOp(UnaryOpExpr::Plus(inner)) => eval_integer(inner, constants, stack),
        ConstExpr::UnaryOp(UnaryOpExpr::Negate(inner)) => {
            Ok(!eval_integer(inner, constants, stack)?)
        }
        _ => bail!("IDL expression is not an integer: {expr:?}"),
    }
}

fn scoped_parts(name: &ScopedName) -> Vec<String> {
    match name {
        ScopedName::Absolute(parts) | ScopedName::Relative(parts) => parts.clone(),
    }
}

fn find_default_expression(annotations: Option<&[AnnotationAppl]>) -> Result<Option<&ConstExpr>> {
    let Some(annotation) = annotations.and_then(|annotations| {
        annotations.iter().find(|annotation| {
            scoped_parts(&annotation.name)
                .last()
                .is_some_and(|name| name == "default")
        })
    }) else {
        return Ok(None);
    };
    let expression = match annotation.params.as_ref() {
        Some(AnnotationApplParams::ConstExpr(expression)) => expression,
        Some(AnnotationApplParams::ApplParams(params)) => {
            &params
                .iter()
                .find(|param| param.id == "value")
                .context("@default annotation has no value parameter")?
                .expr
        }
        None => bail!("@default annotation has no value"),
    };
    Ok(Some(expression))
}

fn literal_default(expression: &ConstExpr, field_type: &FieldType) -> Result<DefaultValue> {
    match expression {
        ConstExpr::Literal(Literal::Boolean(value)) => Ok(DefaultValue::Bool(*value)),
        ConstExpr::Literal(Literal::Integer(value)) => Ok(DefaultValue::Int(
            value
                .to_string()
                .parse()
                .context("Default integer does not fit i64")?,
        )),
        ConstExpr::Literal(Literal::FloatingPoint(value)) => Ok(DefaultValue::Float(*value)),
        ConstExpr::Literal(Literal::String(value)) => Ok(DefaultValue::String(value.clone())),
        ConstExpr::Literal(Literal::Char(value)) if field_type.base_type == "idl_char" => {
            Ok(DefaultValue::Int(i64::from(u32::from(*value))))
        }
        ConstExpr::UnaryOp(UnaryOpExpr::Minus(inner)) => match inner.as_ref() {
            ConstExpr::Literal(Literal::Integer(value)) => Ok(DefaultValue::Int(
                i64::try_from(
                    value
                        .to_string()
                        .parse::<i128>()?
                        .checked_neg()
                        .context("Default integer is outside the supported range")?,
                )
                .context("Default integer does not fit i64")?,
            )),
            ConstExpr::Literal(Literal::FloatingPoint(value)) => Ok(DefaultValue::Float(-value)),
            _ => bail!("Unsupported negative @default expression: {inner:?}"),
        },
        ConstExpr::UnaryOp(UnaryOpExpr::Plus(inner)) => literal_default(inner, field_type),
        _ => bail!("Unsupported @default expression: {expression:?}"),
    }
}

fn convert_constant(constant: &ConstDcl) -> Result<Constant> {
    Ok(Constant {
        name: constant.id.clone(),
        const_type: const_type_name(&constant.const_type)?,
        value: expression_text(&constant.expr),
    })
}

fn const_type_name(const_type: &ConstType) -> Result<String> {
    match const_type {
        ConstType::PrimitiveType(primitive) => Ok(primitive_type(primitive)?.base_type),
        ConstType::StringType(_) => Ok("string".to_string()),
        ConstType::WStringType(_) => Ok("wstring".to_string()),
        ConstType::ScopedName(name) => Ok(scoped_parts(name).join("/")),
        ConstType::FixedPointConst => bail!("Fixed-point constants are not supported"),
    }
}

fn expression_text(expression: &ConstExpr) -> String {
    match expression {
        ConstExpr::Literal(Literal::Char(value)) => format!("'{value}'"),
        ConstExpr::Literal(Literal::String(value)) => format!("\"{value}\""),
        ConstExpr::Literal(Literal::Integer(value)) => value.to_string(),
        ConstExpr::Literal(Literal::FloatingPoint(value)) => value.to_string(),
        ConstExpr::Literal(Literal::FixedPoint(value)) => {
            format!("{}e-{}", value.value, value.scale)
        }
        ConstExpr::Literal(Literal::Boolean(value)) => value.to_string(),
        ConstExpr::ScopedName(name) => scoped_parts(name).join("::"),
        ConstExpr::UnaryOp(UnaryOpExpr::Minus(inner)) => {
            format!("-{}", unary_operand_text(inner))
        }
        ConstExpr::UnaryOp(UnaryOpExpr::Plus(inner)) => {
            format!("+{}", unary_operand_text(inner))
        }
        other => format!("{other:?}"),
    }
}

fn unary_operand_text(expression: &ConstExpr) -> String {
    match expression {
        ConstExpr::Literal(_) | ConstExpr::ScopedName(_) => expression_text(expression),
        _ => format!("({})", expression_text(expression)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDL: &str = r#"
#include "geometry_msgs/msg/point.idl"
module demo_interfaces {
  module msg {
    const unsigned long NAME_BOUND = 8;
    typedef double double__3[3];
    module Sample_Constants {
      const long MODE = 2;
      const int64 MINIMUM = -9223372036854775808;
      const double NEGATIVE_FLOAT = -1.25;
    };
    struct Sample {
      @default(value=3) long count;
      char code;
      wchar wide_code;
      octet raw;
      string<NAME_BOUND> name;
      wstring<4> wide_name;
      sequence<unsigned short, NAME_BOUND> values;
      double__3 point;
      geometry_msgs::msg::Point nested;
      demo_interfaces::action::Other_Goal action_goal;
      @default(value=-9223372036854775808) int64 minimum;
      @default(value=18446744073709551615) uint64 maximum;
    };
  };
};
"#;

    #[test]
    fn parses_ros_idl_types_and_metadata() {
        let parsed = parse_idl_string(IDL, Path::new("Sample.idl")).unwrap();
        assert_eq!(parsed.package, "demo_interfaces");
        assert_eq!(parsed.kind, "msg");
        assert_eq!(parsed.includes, ["geometry_msgs/msg/point.idl"]);
        let parsed_message = &parsed.messages[0];
        let message = &parsed_message.message;
        assert_eq!(message.name, "Sample");
        assert_eq!(message.constants[0].name, "MODE");
        assert_eq!(message.constants[1].value, "-9223372036854775808");
        assert_eq!(message.constants[2].value, "-1.25");
        assert!(matches!(
            message.fields[0].default,
            Some(DefaultValue::Int(3))
        ));
        assert_eq!(message.fields[1].field_type.base_type, "idl_char");
        assert_eq!(message.fields[2].field_type.base_type, "wchar");
        assert_eq!(message.fields[3].field_type.base_type, "byte");
        assert_eq!(message.fields[4].field_type.string_bound, Some(8));
        assert_eq!(message.fields[5].field_type.string_bound, Some(4));
        assert_eq!(message.fields[6].field_type.array, ArrayType::Bounded(8));
        assert_eq!(message.fields[7].field_type.array, ArrayType::Fixed(3));
        assert_eq!(
            message.fields[8].field_type.package.as_deref(),
            Some("geometry_msgs")
        );
        assert_eq!(
            parsed_message.references["nested"],
            "geometry_msgs/msg/Point"
        );
        assert_eq!(
            parsed_message.references["action_goal"],
            "demo_interfaces/action/Other_Goal"
        );
        assert!(matches!(
            message.fields[10].default,
            Some(DefaultValue::Int(i64::MIN))
        ));
        assert!(message.fields[11].default.is_none());
        assert!(matches!(
            parsed_message.defaults["maximum"],
            ConstExpr::Literal(Literal::Integer(ref value))
                if value.to_string() == "18446744073709551615"
        ));
        assert_eq!(parsed.definitions.len(), 1);
    }

    #[test]
    fn rejects_unsupported_preprocessing_and_multidimensional_arrays() {
        let preprocessing =
            IDL.replace("#include \"geometry_msgs/msg/point.idl\"", "#define SIZE 3");
        assert!(
            parse_idl_string(&preprocessing, Path::new("Sample.idl"))
                .unwrap_err()
                .to_string()
                .contains("unsupported preprocessor")
        );

        let multidimensional = IDL.replace("double__3 point;", "double matrix[2][3];");
        assert!(
            parse_idl_string(&multidimensional, Path::new("Sample.idl"))
                .unwrap_err()
                .to_string()
                .contains("Multidimensional")
        );

        let nested_scope = IDL.replace(
            "geometry_msgs::msg::Point nested;",
            "vendor::geometry_msgs::msg::Point nested;",
        );
        assert!(
            parse_idl_string(&nested_scope, Path::new("Sample.idl"))
                .unwrap_err()
                .to_string()
                .contains("Unsupported scoped type")
        );
    }

    #[test]
    fn retains_local_action_and_explicit_cross_namespace_references() {
        let parsed = parse_idl_string(
            "module demo_interfaces { module action { \
               struct Count_Goal { long order; }; \
               struct Count_SendGoal_Request { \
                 Count_Goal goal; \
                 demo_interfaces::srv::Control_Request control; \
               }; \
             }; };",
            Path::new("Count.idl"),
        )
        .unwrap();
        let request = parsed
            .messages
            .iter()
            .find(|message| message.message.name == "Count_SendGoal_Request")
            .unwrap();

        assert_eq!(
            request.references["goal"],
            "demo_interfaces/action/Count_Goal"
        );
        assert_eq!(
            request.references["control"],
            "demo_interfaces/srv/Control_Request"
        );
    }
}
