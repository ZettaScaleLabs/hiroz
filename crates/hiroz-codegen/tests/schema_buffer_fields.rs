//! The message schema that the Rust generator writes for the sequence kinds a buffer rule reads.
//!
//! A subscriber joins rmw_zenoh's CPU buffer group when its message schema has an unbounded
//! sequence of `Uint8`. So the generator decides which ROS field kinds count. These tests parse a
//! small `.msg` text and read the schema tokens of each field.

use std::{collections::HashSet, path::Path};

use hiroz_codegen::{
    generator::rust::{GenerationContext, generate_message_impl_with_cdr},
    parser::msg::parse_msg_string,
    resolver::Resolver,
};

/// The generated code of `source`, with all white space removed.
fn generated(source: &str) -> String {
    let parsed = parse_msg_string(source, "test_msgs", Path::new("Kinds.msg")).expect("parse");
    let resolved = Resolver::new(false)
        .resolve_messages(vec![parsed])
        .expect("resolve")
        .remove(0);
    let ctx = GenerationContext::new(None, HashSet::from(["test_msgs".to_string()]));
    generate_message_impl_with_cdr(&resolved, &ctx, &HashSet::new())
        .expect("generate")
        .to_string()
        .split_whitespace()
        .collect()
}

/// The schema builder call for a field of the given kind, in the form of `generated`.
fn field(name: &str, field_type: &str) -> String {
    format!(".field(\"{name}\",::hiroz::dynamic::FieldType::{field_type})")
}

fn sequence_of(inner: &str) -> String {
    format!("Sequence(::std::boxed::Box::new(::hiroz::dynamic::FieldType::{inner}))")
}

/// An unbounded `uint8[]`, `char[]` and `byte[]` all become a sequence of `Uint8`, so the typed
/// join counts all three. An unbounded `int8[]` stays a sequence of `Int8`, and a bounded or fixed
/// `uint8` is not a sequence.
///
/// Control in the same test: the `uint8[]` field has the sequence form that the other rows are
/// compared against, so a generator that writes no schema at all cannot pass.
#[test]
fn byte_char_and_uint8_sequences_share_one_schema_kind() {
    const NAME: &str = "byte_char_and_uint8_sequences_share_one_schema_kind";
    let code = generated(
        "uint8[] u\nchar[] c\nbyte[] b\nint8[] i\nuint8[<=4] bounded\nuint8[4] fixed\nfloat64[] f\n",
    );
    let uint8_sequence = sequence_of("Uint8");
    let rows = [
        ("u", uint8_sequence.clone()),
        ("c", uint8_sequence.clone()),
        ("b", uint8_sequence.clone()),
        ("i", sequence_of("Int8")),
        (
            "bounded",
            "BoundedSequence(::std::boxed::Box::new(::hiroz::dynamic::FieldType::Uint8),4usize)"
                .to_string(),
        ),
        (
            "fixed",
            "Array(::std::boxed::Box::new(::hiroz::dynamic::FieldType::Uint8),4usize)".to_string(),
        ),
        ("f", sequence_of("Float64")),
    ];
    for (name, field_type) in rows {
        let want = field(name, &field_type);
        assert!(
            code.contains(&want),
            "{NAME}: no `{want}` in the generated schema of field {name}"
        );
    }
}
