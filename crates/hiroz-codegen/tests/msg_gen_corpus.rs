//! Parser + resolver + hash golden tests for the test_interface_files corpus.
//!
//! These tests exercise the full codegen pipeline (parser → resolver → hash)
//! against the vendored test_interface_files message definitions without
//! requiring a ROS 2 installation or running Zenoh router.

use std::{collections::HashMap, path::PathBuf};

use hiroz_codegen::{
    discovery::{discover_actions, discover_messages, discover_services},
    resolver::Resolver,
    types::{
        ArrayType, ParsedAction, ParsedMessage, ParsedService, ResolvedAction, ResolvedMessage,
        ResolvedService,
    },
};

/// Path to the test_interface_files assets directory.
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/test_interface_files")
}

/// Parse all messages from the corpus.
fn parse_corpus_messages() -> Vec<ParsedMessage> {
    discover_messages(&corpus_dir(), "test_interface_files")
        .expect("Failed to discover test_interface_files messages")
}

/// Parse all services from the corpus.
fn parse_corpus_services() -> Vec<ParsedService> {
    discover_services(&corpus_dir(), "test_interface_files")
        .expect("Failed to discover test_interface_files services")
}

/// Parse all actions from the corpus.
fn parse_corpus_actions() -> Vec<ParsedAction> {
    discover_actions(&corpus_dir(), "test_interface_files")
        .expect("Failed to discover test_interface_files actions")
}

/// Get a parsed message by name.
fn get_msg<'a>(msgs: &'a [ParsedMessage], name: &str) -> &'a ParsedMessage {
    msgs.iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("Message '{name}' not found in corpus"))
}

/// Get a parsed service by name.
fn get_srv<'a>(srvs: &'a [ParsedService], name: &str) -> &'a ParsedService {
    srvs.iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("Service '{name}' not found in corpus"))
}

/// Get a parsed action by name.
fn get_action<'a>(actions: &'a [ParsedAction], name: &str) -> &'a ParsedAction {
    actions
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("Action '{name}' not found in corpus"))
}

// ============================================================================
// Parser tests
// ============================================================================

#[test]
fn test_parse_basic_types() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "BasicTypes");

    assert_eq!(msg.package, "test_interface_files");
    // 13 primitive fields: bool, byte, char, float32, float64,
    // int8, uint8, int16, uint16, int32, uint32, int64, uint64
    assert_eq!(msg.fields.len(), 13, "BasicTypes should have 13 fields");
    assert!(
        msg.constants.is_empty(),
        "BasicTypes should have no constants"
    );

    let field_names: Vec<&str> = msg.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(field_names.contains(&"bool_value"));
    assert!(field_names.contains(&"float64_value"));
    assert!(field_names.contains(&"int64_value"));

    // All fields should be scalar (Single array type)
    for field in &msg.fields {
        assert_eq!(
            field.field_type.array,
            ArrayType::Single,
            "BasicTypes field '{}' should be scalar",
            field.name
        );
    }
}

#[test]
fn test_parse_arrays() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Arrays");

    // 32 fields: 17 plain (13 primitives + string + BasicTypes + Constants + Defaults),
    // 14 with array defaults (bool..string), plus alignment_check (scalar).
    assert_eq!(msg.fields.len(), 32, "Arrays should have 32 fields");

    // First 17 plain array fields are all Fixed(3)
    for field in &msg.fields[..17] {
        assert_eq!(
            field.field_type.array,
            ArrayType::Fixed(3),
            "Arrays plain field '{}' should be Fixed(3)",
            field.name
        );
    }
    // alignment_check is the last field and is scalar
    let last = msg.fields.last().unwrap();
    assert_eq!(last.name, "alignment_check");
    assert_eq!(last.field_type.array, ArrayType::Single);
}

#[test]
fn test_parse_bounded_sequences() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "BoundedSequences");

    // 32 fields: 17 plain + 14 with defaults + alignment_check
    assert_eq!(
        msg.fields.len(),
        32,
        "BoundedSequences should have 32 fields"
    );

    // First 17 are Bounded(3)
    for field in &msg.fields[..17] {
        assert_eq!(
            field.field_type.array,
            ArrayType::Bounded(3),
            "BoundedSequences plain field '{}' should be Bounded(3)",
            field.name
        );
    }
    let last = msg.fields.last().unwrap();
    assert_eq!(last.name, "alignment_check");
    assert_eq!(last.field_type.array, ArrayType::Single);
}

#[test]
fn test_parse_unbounded_sequences() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "UnboundedSequences");

    // 32 fields: 17 plain + 14 with defaults + alignment_check
    assert_eq!(
        msg.fields.len(),
        32,
        "UnboundedSequences should have 32 fields"
    );

    // First 17 are Unbounded
    for field in &msg.fields[..17] {
        assert_eq!(
            field.field_type.array,
            ArrayType::Unbounded,
            "UnboundedSequences plain field '{}' should be Unbounded",
            field.name
        );
    }
    let last = msg.fields.last().unwrap();
    assert_eq!(last.name, "alignment_check");
    assert_eq!(last.field_type.array, ArrayType::Single);
}

#[test]
fn test_parse_constants() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Constants");

    // Constants.msg has 13 constants (bool..uint64), no fields, no string constant
    assert!(msg.fields.is_empty(), "Constants.msg should have no fields");
    assert_eq!(
        msg.constants.len(),
        13,
        "Constants.msg should have 13 constants"
    );

    let const_names: Vec<&str> = msg.constants.iter().map(|c| c.name.as_str()).collect();
    assert!(const_names.contains(&"BOOL_CONST"));
    assert!(const_names.contains(&"INT64_CONST"));
    assert!(const_names.contains(&"UINT64_CONST"));
}

#[test]
fn test_parse_defaults() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Defaults");

    // Defaults.msg has 13 fields (bool..uint64) with default values, no string field
    assert_eq!(msg.fields.len(), 13, "Defaults.msg should have 13 fields");

    for field in &msg.fields {
        assert!(
            field.default.is_some(),
            "Defaults.msg field '{}' should have a default value",
            field.name
        );
    }
}

#[test]
fn test_parse_empty() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Empty");

    assert!(msg.fields.is_empty(), "Empty.msg should have no fields");
    assert!(
        msg.constants.is_empty(),
        "Empty.msg should have no constants"
    );
}

#[test]
fn test_parse_nested() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Nested");

    assert_eq!(msg.fields.len(), 1, "Nested.msg should have 1 field");
    assert_eq!(msg.fields[0].name, "basic_types_value");
    assert_eq!(msg.fields[0].field_type.base_type, "BasicTypes");
    assert_eq!(msg.fields[0].field_type.array, ArrayType::Single);
}

#[test]
fn test_parse_multi_nested() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "MultiNested");

    // 9 fields: Arrays/BoundedSequences/UnboundedSequences × 3 array kinds (single, fixed[3], unbounded)
    assert_eq!(msg.fields.len(), 9, "MultiNested.msg should have 9 fields");

    // First 3 are scalar references to the three complex types
    assert_eq!(msg.fields[0].name, "array_of_arrays");
    assert_eq!(msg.fields[0].field_type.base_type, "Arrays");
    assert_eq!(msg.fields[0].field_type.array, ArrayType::Single);
}

#[test]
fn test_parse_strings() {
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "Strings");

    // 8 fields: string_value, bounded_string_value, and 6 array variants
    assert_eq!(msg.fields.len(), 8, "Strings.msg should have 8 fields");

    // bounded_string_value should have string_bound = Some(10)
    let bounded = msg
        .fields
        .iter()
        .find(|f| f.name == "bounded_string_value")
        .expect("Should have bounded_string_value field");
    assert_eq!(
        bounded.field_type.string_bound,
        Some(10),
        "bounded_string_value should have string_bound=10"
    );

    // unbounded_string_array should be Unbounded
    let arr = msg
        .fields
        .iter()
        .find(|f| f.name == "unbounded_string_array")
        .unwrap();
    assert_eq!(arr.field_type.array, ArrayType::Unbounded);

    // string_array_three should be Fixed(3)
    let arr3 = msg
        .fields
        .iter()
        .find(|f| f.name == "string_array_three")
        .unwrap();
    assert_eq!(arr3.field_type.array, ArrayType::Fixed(3));
}

#[test]
fn test_parse_wstrings_filtered() {
    // WStrings.msg should parse without panicking; has wstring fields.
    // The filter_messages function in MessageGenerator would exclude it,
    // but the parser itself should succeed.
    let msgs = parse_corpus_messages();
    let msg = get_msg(&msgs, "WStrings");

    // All fields have wstring base type
    assert!(!msg.fields.is_empty(), "WStrings.msg should have fields");
    for field in &msg.fields {
        assert!(
            field.field_type.base_type.contains("wstring"),
            "WStrings.msg field '{}' should have wstring type",
            field.name
        );
    }
}

#[test]
fn test_parse_service_basic_types() {
    let srvs = parse_corpus_services();
    let srv = get_srv(&srvs, "BasicTypes");

    assert_eq!(srv.package, "test_interface_files");
    // BasicTypes.srv has 14 fields (13 primitives + string) in both request and response
    assert_eq!(
        srv.request.fields.len(),
        14,
        "Request should have 14 fields"
    );
    assert_eq!(
        srv.response.fields.len(),
        14,
        "Response should have 14 fields"
    );
    assert_eq!(srv.request.name, "BasicTypesRequest");
    assert_eq!(srv.response.name, "BasicTypesResponse");
}

#[test]
fn test_parse_fibonacci_action() {
    let actions = parse_corpus_actions();
    let action = get_action(&actions, "Fibonacci");

    assert_eq!(action.package, "test_interface_files");
    assert_eq!(action.name, "Fibonacci");

    // Goal: int32 order
    assert_eq!(action.goal.fields.len(), 1);
    assert_eq!(action.goal.fields[0].name, "order");
    assert_eq!(action.goal.fields[0].field_type.base_type, "int32");

    // Result: int32[] sequence
    let result = action
        .result
        .as_ref()
        .expect("Fibonacci action should have result");
    assert_eq!(result.fields.len(), 1);
    assert_eq!(result.fields[0].name, "sequence");
    assert_eq!(result.fields[0].field_type.array, ArrayType::Unbounded);

    // Feedback: int32[] sequence
    let feedback = action
        .feedback
        .as_ref()
        .expect("Fibonacci action should have feedback");
    assert_eq!(feedback.fields.len(), 1);
    assert_eq!(feedback.fields[0].name, "sequence");
}

#[test]
fn test_parse_service_empty() {
    let srvs = parse_corpus_services();
    let srv = get_srv(&srvs, "Empty");
    assert_eq!(srv.package, "test_interface_files");
    assert!(
        srv.request.fields.is_empty(),
        "Empty.srv request should have no fields"
    );
    assert!(
        srv.response.fields.is_empty(),
        "Empty.srv response should have no fields"
    );
    assert_eq!(srv.request.name, "EmptyRequest");
    assert_eq!(srv.response.name, "EmptyResponse");
}

#[test]
fn test_parse_service_arrays() {
    let srvs = parse_corpus_services();
    let srv = get_srv(&srvs, "Arrays");
    assert_eq!(srv.package, "test_interface_files");
    // Arrays.srv: request and response each have 14 fixed-size array fields
    assert_eq!(
        srv.request.fields.len(),
        14,
        "Arrays.srv request should have 14 fields"
    );
    assert_eq!(
        srv.response.fields.len(),
        14,
        "Arrays.srv response should have 14 fields"
    );
    // Spot-check: first field is bool[3]
    assert_eq!(srv.request.fields[0].name, "bool_values");
    assert_eq!(srv.request.fields[0].field_type.array, ArrayType::Fixed(3));
}

// ============================================================================
// Hash tests
// ============================================================================

/// Resolve corpus messages for hashing. Excludes WStrings because TypeId has no
/// array variants for wstring (WSTRING_ARRAY etc. are not defined in the schema).
fn resolve_corpus() -> HashMap<String, ResolvedMessage> {
    let msgs = parse_corpus_messages()
        .into_iter()
        .filter(|m| m.name != "WStrings")
        .collect();
    let mut resolver = Resolver::new(false);
    let resolved = resolver
        .resolve_messages(msgs)
        .expect("Failed to resolve test_interface_files messages");
    resolved
        .into_iter()
        .map(|r| (r.parsed.name.clone(), r))
        .collect()
}

/// Resolve all corpus services. Uses humble mode (is_humble=true) because Jazzy
/// service hashing requires service_msgs/ServiceEventInfo which is not bundled here.
fn resolve_corpus_services() -> (Vec<ResolvedService>, Vec<ResolvedMessage>) {
    let msgs = parse_corpus_messages()
        .into_iter()
        .filter(|m| m.name != "WStrings")
        .collect();
    let mut resolver = Resolver::new(true);
    let resolved_msgs = resolver
        .resolve_messages(msgs)
        .expect("Failed to resolve messages for service resolver");
    let srvs = parse_corpus_services();
    let resolved_srvs = resolver
        .resolve_services(srvs)
        .expect("Failed to resolve test_interface_files services");
    (resolved_srvs, resolved_msgs)
}

/// Resolve all corpus actions. Uses humble mode for the same reason as services.
fn resolve_corpus_actions() -> Vec<ResolvedAction> {
    let msgs = parse_corpus_messages()
        .into_iter()
        .filter(|m| m.name != "WStrings")
        .collect();
    let mut resolver = Resolver::new(true);
    resolver
        .resolve_messages(msgs)
        .expect("Failed to resolve messages for action resolver");
    let actions = parse_corpus_actions();
    resolver
        .resolve_actions(actions)
        .expect("Failed to resolve test_interface_files actions")
}

#[test]
fn test_hash_format_basic_types() {
    let resolved = resolve_corpus();
    let hash = resolved["BasicTypes"].type_hash.to_rihs_string();
    assert!(
        hash.starts_with("RIHS01_"),
        "Hash must start with RIHS01_: {hash}"
    );
    assert_eq!(
        hash.len(),
        7 + 64,
        "Hash must be RIHS01_ + 64 hex chars: {hash}"
    );
}

#[test]
fn test_hash_format_empty() {
    let resolved = resolve_corpus();
    let hash = resolved["Empty"].type_hash.to_rihs_string();
    assert!(
        hash.starts_with("RIHS01_"),
        "Hash must start with RIHS01_: {hash}"
    );
    assert_eq!(
        hash.len(),
        7 + 64,
        "Hash must be RIHS01_ + 64 hex chars: {hash}"
    );
}

#[test]
fn test_hash_deterministic() {
    let resolved1 = resolve_corpus();
    let resolved2 = resolve_corpus();
    let h1 = resolved1["BasicTypes"].type_hash.to_rihs_string();
    let h2 = resolved2["BasicTypes"].type_hash.to_rihs_string();
    assert_eq!(h1, h2, "Hash must be deterministic");
}

#[test]
fn test_hashes_differ_between_types() {
    let resolved = resolve_corpus();
    let h_basic = resolved["BasicTypes"].type_hash.to_rihs_string();
    let h_empty = resolved["Empty"].type_hash.to_rihs_string();
    let h_nested = resolved["Nested"].type_hash.to_rihs_string();
    let h_arrays = resolved["Arrays"].type_hash.to_rihs_string();

    let all = [&h_basic, &h_empty, &h_nested, &h_arrays];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            if i != j {
                assert_ne!(
                    a, b,
                    "Hashes must differ for different types (indices {i},{j})"
                );
            }
        }
    }
}

#[test]
fn test_hash_nested_depends_on_basic_types() {
    // Nested.msg contains BasicTypes; its hash must differ from BasicTypes's hash
    let resolved = resolve_corpus();
    let h_basic = resolved["BasicTypes"].type_hash.to_rihs_string();
    let h_nested = resolved["Nested"].type_hash.to_rihs_string();
    assert_ne!(
        h_basic, h_nested,
        "Nested hash must differ from BasicTypes hash"
    );
}

// ============================================================================
// Golden hash regression tests
// Captured from this implementation. Must be verified against rmw_zenoh_cpp
// on a live ROS node before being considered canonical. To regenerate run:
//   cargo test -p hiroz-codegen --test msg_gen_corpus -- print_golden_hashes \
//     --ignored --nocapture
// ============================================================================

#[test]
fn test_golden_hashes() {
    let resolved = resolve_corpus();
    let cases = [
        (
            "Arrays",
            "RIHS01_f2a373b74b93feccf53bd195bfd5d6d33f3b8770c44fbe07e5bdb80bcba91b0a",
        ),
        (
            "BasicTypes",
            "RIHS01_8614a017261eb9eecc4cf82f044feab34bedbd2b83c031f451ba15e8daa97247",
        ),
        (
            "BoundedPlainSequences",
            "RIHS01_577a9b4de61219f4699274702985dfd35f4ada2bc88cf57251a174339c35f9fa",
        ),
        (
            "BoundedSequences",
            "RIHS01_a9c6ddf375f5ffd4f37f3dbe73082652c7ca150b52bd4e03a73d016fc3e72ff7",
        ),
        (
            "Constants",
            "RIHS01_b72f1a454ff7936bbaabe3a92f8fdff0bf15287f7d2862aef9f705ff018c9ab4",
        ),
        (
            "Defaults",
            "RIHS01_577d96cd5175213ee96074a8fd9bf4b83e00fd2e1db6e5e75ae688cd27029212",
        ),
        (
            "Empty",
            "RIHS01_9dcf6bec5f8eb5056b8a0a6b3aab22f43d31b5ee3b6d8c9842005d32816001f9",
        ),
        (
            "MultiNested",
            "RIHS01_677bdd1471c63e02356beaf1e09409a875c0e604f86cfefcad70166b1019e718",
        ),
        (
            "Nested",
            "RIHS01_f7508dd21e8e3a45218396836e71303befa72c54067dc6ceb50f499e636379ae",
        ),
        (
            "Strings",
            "RIHS01_909e50e6359955bed5e712561cd7946c25688525615dca4601697a77ee49934f",
        ),
        (
            "UnboundedSequences",
            "RIHS01_c4c1310b554263d0e2c3ec3d4f3f86bd5a91b773fd7faba57a3e58b70da180f5",
        ),
    ];
    for (name, expected) in cases {
        let hash = resolved[name].type_hash.to_rihs_string();
        assert_eq!(hash, expected, "{name} hash regression");
    }
}

// ============================================================================
// Service and action hash tests
// ============================================================================

#[test]
fn test_service_hash_format() {
    let (srvs, _) = resolve_corpus_services();
    for srv in &srvs {
        let hash = srv.type_hash.to_rihs_string();
        assert!(
            hash.starts_with("RIHS01_"),
            "Service {} hash must start with RIHS01_: {hash}",
            srv.parsed.name
        );
        assert_eq!(
            hash.len(),
            7 + 64,
            "Service {} hash must be RIHS01_ + 64 hex chars: {hash}",
            srv.parsed.name
        );
    }
}

#[test]
fn test_service_hashes_differ_from_messages() {
    let resolved_msgs = resolve_corpus();
    let (srvs, _) = resolve_corpus_services();

    // BasicTypes.srv hash must differ from BasicTypes.msg hash
    let msg_hash = resolved_msgs["BasicTypes"].type_hash.to_rihs_string();
    let srv = srvs.iter().find(|s| s.parsed.name == "BasicTypes").unwrap();
    let srv_hash = srv.type_hash.to_rihs_string();
    assert_ne!(
        msg_hash, srv_hash,
        "BasicTypes.srv hash must differ from BasicTypes.msg hash"
    );
}

#[test]
fn test_action_hash_format() {
    let actions = resolve_corpus_actions();
    for action in &actions {
        let hash = action.type_hash.to_rihs_string();
        assert!(
            hash.starts_with("RIHS01_"),
            "Action {} hash must start with RIHS01_: {hash}",
            action.parsed.name
        );
        assert_eq!(
            hash.len(),
            7 + 64,
            "Action {} hash must be RIHS01_ + 64 hex chars: {hash}",
            action.parsed.name
        );
    }
}

#[test]
fn test_action_goal_result_feedback_hashes_differ() {
    let actions = resolve_corpus_actions();
    let fib = actions
        .iter()
        .find(|a| a.parsed.name == "Fibonacci")
        .unwrap();

    let goal_hash = fib.goal.type_hash.to_rihs_string();
    let result = fib.result.as_ref().unwrap();
    let feedback = fib.feedback.as_ref().unwrap();
    let result_hash = result.type_hash.to_rihs_string();
    let feedback_hash = feedback.type_hash.to_rihs_string();

    assert_ne!(
        goal_hash, result_hash,
        "Fibonacci goal/result hashes must differ"
    );
    assert_ne!(
        goal_hash, feedback_hash,
        "Fibonacci goal/feedback hashes must differ"
    );
}

/// Print service and action hashes for verification against rmw_zenoh_cpp.
#[test]
#[ignore = "capture: run once to record or verify golden hashes"]
fn print_golden_hashes() {
    let resolved = resolve_corpus();
    let mut names: Vec<&String> = resolved.keys().collect();
    names.sort();
    println!("=== Messages ===");
    for name in names {
        println!("{}: {}", name, resolved[name].type_hash.to_rihs_string());
    }
    let (srvs, _) = resolve_corpus_services();
    println!("=== Services ===");
    for srv in &srvs {
        println!("{}: {}", srv.parsed.name, srv.type_hash.to_rihs_string());
    }
    let actions = resolve_corpus_actions();
    println!("=== Actions ===");
    for action in &actions {
        println!(
            "{}(goal): {}",
            action.parsed.name,
            action.goal.type_hash.to_rihs_string()
        );
        if let Some(ref r) = action.result {
            println!(
                "{}(result): {}",
                action.parsed.name,
                r.type_hash.to_rihs_string()
            );
        }
        if let Some(ref f) = action.feedback {
            println!(
                "{}(feedback): {}",
                action.parsed.name,
                f.type_hash.to_rihs_string()
            );
        }
    }
}

// ============================================================================
// Parser error path tests
// ============================================================================

mod parser_errors {
    use hiroz_codegen::parser::{
        parse_constant, parse_default_value, parse_field, parse_field_type,
    };

    #[test]
    fn test_parse_field_too_few_tokens() {
        // A field line needs at least "type name"; one token is invalid.
        let err = parse_field("uint8", "pkg", 1).unwrap_err();
        assert!(
            err.to_string().contains("Invalid field format"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_parse_field_type_unclosed_bracket() {
        let err = parse_field_type("uint8[", "pkg").unwrap_err();
        assert!(
            err.to_string().contains("Invalid array syntax"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_parse_field_type_invalid_fixed_size() {
        let err = parse_field_type("uint8[abc]", "pkg").unwrap_err();
        assert!(
            err.to_string().contains("Invalid fixed array size"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_parse_field_type_invalid_bounded_array_size() {
        let err = parse_field_type("uint8[<=xyz]", "pkg").unwrap_err();
        assert!(
            err.to_string().contains("Invalid bounded array size"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_parse_field_type_invalid_string_bound() {
        let err = parse_field_type("string<=xyz", "pkg").unwrap_err();
        assert!(
            err.to_string().contains("Invalid string bound"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn test_parse_default_value_empty_array() {
        // An empty array literal "[]" is not a valid default.
        let err = parse_default_value("[]").unwrap_err();
        assert!(err.to_string().contains("Empty array"), "unexpected: {err}");
    }

    #[test]
    fn test_parse_constant_missing_value() {
        // A constant line must have "TYPE NAME = VALUE"; missing "= value" is invalid.
        let err = parse_constant("uint8 FOO", 1).unwrap_err();
        assert!(err.to_string().contains("constant"), "unexpected: {err}");
    }
}
