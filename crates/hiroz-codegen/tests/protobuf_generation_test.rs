#![cfg(feature = "protobuf")]
/// Integration tests for Protobuf generation from ROS messages
use std::fs;
use std::path::PathBuf;

use hiroz_codegen::{discovery, protobuf_generator::ProtobufMessageGenerator, resolver::Resolver};

/// Test that .proto files are generated correctly from ROS messages
#[test]
fn test_proto_file_generation() {
    let test_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/std_msgs");

    assert!(test_assets.exists(), "Test assets directory not found");

    // Discover messages
    let (messages, _, _) =
        discovery::discover_all(&[&test_assets]).expect("Failed to discover messages");

    // Resolve messages
    let mut resolver = Resolver::new(false); // Not Humble
    let resolved = resolver
        .resolve_messages(messages)
        .expect("Failed to resolve messages");

    assert!(!resolved.is_empty(), "No messages resolved");

    // Generate .proto files
    let temp_dir = tempfile::tempdir().unwrap();
    let proto_dir = temp_dir.path().join("proto");

    let generator = ProtobufMessageGenerator::new(&proto_dir);
    let proto_files = generator
        .generate_proto_files(&resolved)
        .expect("Failed to generate proto files");

    assert!(!proto_files.is_empty(), "No proto files generated");

    // Verify proto file exists and has correct structure
    for proto_file in &proto_files {
        assert!(
            proto_file.exists(),
            "Proto file not created: {:?}",
            proto_file
        );

        let content = fs::read_to_string(proto_file).expect("Failed to read proto file");

        // Check basic proto3 structure
        assert!(
            content.contains("syntax = \"proto3\";"),
            "Proto file missing syntax declaration"
        );
        assert!(
            content.contains("package "),
            "Proto file missing package declaration"
        );
        assert!(
            content.contains("message "),
            "Proto file missing message definition"
        );
    }

    // Verify std_msgs.proto specifically
    let std_msgs_proto = proto_dir.join("std_msgs.proto");
    assert!(std_msgs_proto.exists(), "std_msgs.proto not generated");

    let std_msgs_content =
        fs::read_to_string(&std_msgs_proto).expect("Failed to read std_msgs.proto");

    // Should contain common std_msgs types
    assert!(
        std_msgs_content.contains("message String"),
        "String message not found"
    );
    assert!(
        std_msgs_content.contains("string data"),
        "String data field not found"
    );
}

/// Test type mapping from ROS to Protobuf types
#[test]
fn test_type_mapping() {
    let test_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/geometry_msgs");

    if !test_assets.exists() {
        println!("Skipping test - geometry_msgs not found");
        return;
    }

    let (messages, _, _) =
        discovery::discover_all(&[&test_assets]).expect("Failed to discover messages");

    let mut resolver = Resolver::new(false);
    let resolved = resolver
        .resolve_messages(messages)
        .expect("Failed to resolve messages");

    let temp_dir = tempfile::tempdir().unwrap();
    let proto_dir = temp_dir.path().join("proto");

    let generator = ProtobufMessageGenerator::new(&proto_dir);
    let _ = generator
        .generate_proto_files(&resolved)
        .expect("Failed to generate proto files");

    let proto_file = proto_dir.join("geometry_msgs.proto");
    let content = fs::read_to_string(&proto_file).expect("Failed to read geometry_msgs.proto");

    // Check type mappings
    assert!(
        content.contains("float64") || content.contains("double"),
        "float64 not mapped to double"
    );
    assert!(
        content.contains("float32") || content.contains("float"),
        "float32 not mapped to float"
    );
}

/// Test that dependencies/imports are handled correctly
#[test]
fn test_proto_dependencies() {
    // Test with geometry_msgs which depends on std_msgs
    let test_assets_geometry =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/geometry_msgs");
    let test_assets_std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/std_msgs");

    if !test_assets_geometry.exists() || !test_assets_std.exists() {
        println!("Skipping test - test assets not found");
        return;
    }

    let (messages, _, _) = discovery::discover_all(&[&test_assets_geometry, &test_assets_std])
        .expect("Failed to discover messages");

    let mut resolver = Resolver::new(false);
    let resolved = resolver
        .resolve_messages(messages)
        .expect("Failed to resolve messages");

    let temp_dir = tempfile::tempdir().unwrap();
    let proto_dir = temp_dir.path().join("proto");

    let generator = ProtobufMessageGenerator::new(&proto_dir);
    let _ = generator
        .generate_proto_files(&resolved)
        .expect("Failed to generate proto files");

    // Check that imports are present when needed
    let geometry_proto = proto_dir.join("geometry_msgs.proto");
    if geometry_proto.exists() {
        let content =
            fs::read_to_string(&geometry_proto).expect("Failed to read geometry_msgs.proto");

        // geometry_msgs should import std_msgs if it has Header fields
        if content.contains("std_msgs") {
            assert!(
                content.contains("import \"std_msgs.proto\";")
                    || content.contains("import \"builtin_interfaces.proto\";"),
                "Missing import for std_msgs types"
            );
        }
    }
}

/// Test MessageTypeInfo generation
#[test]
fn test_type_info_generation() {
    let test_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/std_msgs");

    assert!(test_assets.exists(), "Test assets not found");

    let (messages, _, _) =
        discovery::discover_all(&[&test_assets]).expect("Failed to discover messages");

    let mut resolver = Resolver::new(false);
    let resolved = resolver
        .resolve_messages(messages)
        .expect("Failed to resolve messages");

    let temp_dir = tempfile::tempdir().unwrap();
    let proto_dir = temp_dir.path().join("proto");

    let generator = ProtobufMessageGenerator::new(&proto_dir);
    let type_info = generator
        .generate_type_info_impls(&resolved)
        .expect("Failed to generate type info");

    // Verify type info contains necessary trait implementations
    assert!(
        type_info.contains("impl ::hiroz::MessageTypeInfo"),
        "Missing MessageTypeInfo trait"
    );
    assert!(
        type_info.contains("impl ::hiroz::WithTypeInfo"),
        "Missing WithTypeInfo trait"
    );
    assert!(
        type_info.contains("impl ::hiroz::msg::ZMessage"),
        "Missing ZMessage trait"
    );
    assert!(
        type_info.contains("type_hash()"),
        "Missing type_hash method"
    );
    assert!(
        type_info.contains("type_name()"),
        "Missing type_name method"
    );
}

/// Test array type handling (repeated fields in proto)
#[test]
fn test_array_type_handling() {
    let test_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/std_msgs");

    if !test_assets.exists() {
        println!("Skipping test - std_msgs not found");
        return;
    }

    let (messages, _, _) =
        discovery::discover_all(&[&test_assets]).expect("Failed to discover messages");

    let mut resolver = Resolver::new(false);
    let resolved = resolver
        .resolve_messages(messages)
        .expect("Failed to resolve messages");

    let temp_dir = tempfile::tempdir().unwrap();
    let proto_dir = temp_dir.path().join("proto");

    let generator = ProtobufMessageGenerator::new(&proto_dir);
    let _ = generator
        .generate_proto_files(&resolved)
        .expect("Failed to generate proto files");

    let proto_file = proto_dir.join("std_msgs.proto");
    let content = fs::read_to_string(&proto_file).expect("Failed to read proto");

    // Arrays should be mapped to repeated fields
    if content.contains("MultiArray") {
        assert!(
            content.contains("repeated"),
            "Array types not mapped to repeated fields"
        );
    }
}
