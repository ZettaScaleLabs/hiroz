use std::path::PathBuf;

// Re-export TypeHash from hiroz-schema
pub use hiroz_schema::TypeHash;

/// Parsed message before dependency resolution
#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub name: String,
    pub package: String,
    pub fields: Vec<Field>,
    pub constants: Vec<Constant>,
    pub source: String,
    pub path: PathBuf,
}

/// Parsed service definition
#[derive(Debug, Clone)]
pub struct ParsedService {
    pub name: String,
    pub package: String,
    pub request: ParsedMessage,
    pub response: ParsedMessage,
    pub source: String,
    pub path: PathBuf,
}

/// Parsed action definition
#[derive(Debug, Clone)]
pub struct ParsedAction {
    pub name: String,
    pub package: String,
    pub goal: ParsedMessage,
    pub result: Option<ParsedMessage>,
    pub feedback: Option<ParsedMessage>,
    pub source: String,
    pub path: PathBuf,
}

/// Field in a message
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub field_type: FieldType,
    pub default: Option<DefaultValue>,
}

/// Constant definition in a message
#[derive(Debug, Clone)]
pub struct Constant {
    pub name: String,
    pub const_type: String,
    pub value: String,
}

/// Field type with array information
#[derive(Debug, Clone)]
pub struct FieldType {
    pub base_type: String,
    pub package: Option<String>,
    pub array: ArrayType,
    /// For bounded strings (string<=N), the maximum length
    pub string_bound: Option<usize>,
}

/// Array type specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayType {
    Single,
    Fixed(usize),
    Bounded(usize),
    Unbounded,
}

/// Default value for a field (ROS2 syntax)
#[derive(Debug, Clone)]
pub enum DefaultValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    BoolArray(Vec<bool>),
    IntArray(Vec<i64>),
    FloatArray(Vec<f64>),
    StringArray(Vec<String>),
}

/// Resolved message with type hash (after dependency resolution)
#[derive(Debug, Clone)]
pub struct ResolvedMessage {
    pub parsed: ParsedMessage,
    pub type_hash: TypeHash,
    pub definition: String,
}

/// Resolved service with type hash
#[derive(Debug, Clone)]
pub struct ResolvedService {
    pub parsed: ParsedService,
    pub request: ResolvedMessage,
    pub response: ResolvedMessage,
    pub type_hash: TypeHash,
}

/// Resolved action with type hash
#[derive(Debug, Clone)]
pub struct ResolvedAction {
    pub parsed: ParsedAction,
    pub goal: ResolvedMessage,
    pub result: Option<ResolvedMessage>,
    pub feedback: Option<ResolvedMessage>,
    pub type_hash: TypeHash,
    // Type hashes for action protocol services/messages
    pub send_goal_hash: TypeHash,
    pub get_result_hash: TypeHash,
    pub feedback_message_hash: TypeHash,
    // Standard ROS2 action protocol type hashes
    pub cancel_goal_hash: TypeHash,
    pub status_hash: TypeHash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_array_type_equality() {
        assert_eq!(ArrayType::Single, ArrayType::Single);
        assert_eq!(ArrayType::Fixed(10), ArrayType::Fixed(10));
        assert_eq!(ArrayType::Bounded(5), ArrayType::Bounded(5));
        assert_eq!(ArrayType::Unbounded, ArrayType::Unbounded);

        assert_ne!(ArrayType::Fixed(10), ArrayType::Fixed(20));
        assert_ne!(ArrayType::Single, ArrayType::Unbounded);
    }

    #[test]
    fn test_parsed_message_creation() {
        let msg = ParsedMessage {
            name: "TestMessage".to_string(),
            package: "test_pkg".to_string(),
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
            path: PathBuf::from("test.msg"),
        };

        assert_eq!(msg.name, "TestMessage");
        assert_eq!(msg.package, "test_pkg");
        assert_eq!(msg.fields.len(), 1);
    }

    #[test]
    fn test_field_type_with_package() {
        let field_type = FieldType {
            base_type: "Point".to_string(),
            package: Some("geometry_msgs".to_string()),
            array: ArrayType::Unbounded,
            string_bound: None,
        };

        assert_eq!(field_type.base_type, "Point");
        assert_eq!(field_type.package, Some("geometry_msgs".to_string()));
        assert_eq!(field_type.array, ArrayType::Unbounded);
    }

    #[test]
    fn test_default_value_variants() {
        let bool_val = DefaultValue::Bool(true);
        let int_val = DefaultValue::Int(42);
        let float_val = DefaultValue::Float(1.23);
        let string_val = DefaultValue::String("hello".to_string());

        match bool_val {
            DefaultValue::Bool(b) => assert!(b),
            _ => panic!("Expected Bool variant"),
        }

        match int_val {
            DefaultValue::Int(i) => assert_eq!(i, 42),
            _ => panic!("Expected Int variant"),
        }

        match float_val {
            DefaultValue::Float(f) => assert!((f - 1.23).abs() < 0.001),
            _ => panic!("Expected Float variant"),
        }

        match string_val {
            DefaultValue::String(s) => assert_eq!(s, "hello"),
            _ => panic!("Expected String variant"),
        }
    }
}
