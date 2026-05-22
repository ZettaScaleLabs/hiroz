//! Snapshot tests for the Rust code generator (`generator/rust.rs`).
//!
//! These tests lock in the shape of the generated Rust source for representative
//! message types from the test_interface_files corpus. Any change to derive macros,
//! trait impls, field types, or naming conventions will fail these tests and require
//! an explicit `cargo insta review` to accept the new output.
//!
//! Run `cargo insta review -p hiroz-codegen` to inspect and accept snapshot changes.

use std::{collections::HashSet, path::PathBuf};

use hiroz_codegen::{
    generator::rust::{
        GenerationContext, generate_action_impl, generate_message_impl_with_cdr,
        generate_service_impl,
    },
    resolver::Resolver,
    types::ResolvedMessage,
};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy/test_interface_files")
}

/// Resolve all test_interface_files messages into a map keyed by name.
fn resolve_corpus() -> Vec<ResolvedMessage> {
    let parsed = hiroz_codegen::discovery::discover_messages(&corpus_dir(), "test_interface_files")
        .expect("discover messages")
        .into_iter()
        .filter(|m| m.name != "WStrings")
        .collect::<Vec<_>>();

    let mut resolver = Resolver::new(false);
    resolver.resolve_messages(parsed).expect("resolve messages")
}

fn ctx() -> GenerationContext {
    let mut local = HashSet::new();
    local.insert("test_interface_files".to_string());
    GenerationContext::new(None, local)
}

fn format_tokens(ts: proc_macro2::TokenStream) -> String {
    let file: syn::File = syn::parse2(ts).expect("parse generated TokenStream");
    prettyplease::unparse(&file)
}

fn get_resolved<'a>(msgs: &'a [ResolvedMessage], name: &str) -> &'a ResolvedMessage {
    msgs.iter()
        .find(|m| m.parsed.name == name)
        .unwrap_or_else(|| panic!("ResolvedMessage '{name}' not found"))
}

// ── Message snapshots ─────────────────────────────────────────────────────────

#[test]
fn snapshot_basic_types() {
    let msgs = resolve_corpus();
    let msg = get_resolved(&msgs, "BasicTypes");
    let tokens =
        generate_message_impl_with_cdr(msg, &ctx(), &HashSet::new()).expect("generate BasicTypes");
    insta::assert_snapshot!("basic_types", format_tokens(tokens));
}

#[test]
fn snapshot_strings() {
    let msgs = resolve_corpus();
    let msg = get_resolved(&msgs, "Strings");
    let tokens =
        generate_message_impl_with_cdr(msg, &ctx(), &HashSet::new()).expect("generate Strings");
    insta::assert_snapshot!("strings", format_tokens(tokens));
}

#[test]
fn snapshot_arrays() {
    let msgs = resolve_corpus();
    let msg = get_resolved(&msgs, "Arrays");
    let tokens =
        generate_message_impl_with_cdr(msg, &ctx(), &HashSet::new()).expect("generate Arrays");
    insta::assert_snapshot!("arrays", format_tokens(tokens));
}

#[test]
fn snapshot_nested() {
    let msgs = resolve_corpus();
    let msg = get_resolved(&msgs, "Nested");
    let tokens =
        generate_message_impl_with_cdr(msg, &ctx(), &HashSet::new()).expect("generate Nested");
    insta::assert_snapshot!("nested", format_tokens(tokens));
}

// ── Service snapshot ──────────────────────────────────────────────────────────

#[test]
fn snapshot_service_basic_types() {
    let parsed = hiroz_codegen::discovery::discover_services(&corpus_dir(), "test_interface_files")
        .expect("discover services");

    let mut resolver = Resolver::new(true);
    let resolved = resolver.resolve_services(parsed).expect("resolve services");

    let srv = resolved
        .iter()
        .find(|s| s.parsed.name == "BasicTypes")
        .expect("BasicTypes service not found");

    let tokens = generate_service_impl(srv).expect("generate service");
    insta::assert_snapshot!("service_basic_types", format_tokens(tokens));
}

// ── Action snapshot ───────────────────────────────────────────────────────────

#[test]
fn snapshot_action_fibonacci() {
    let parsed = hiroz_codegen::discovery::discover_actions(&corpus_dir(), "test_interface_files")
        .expect("discover actions");

    let mut resolver = Resolver::new(true);
    let resolved = resolver.resolve_actions(parsed).expect("resolve actions");

    let action = resolved
        .iter()
        .find(|a| a.parsed.name == "Fibonacci")
        .expect("Fibonacci action not found");

    let tokens = generate_action_impl(action).expect("generate action");
    insta::assert_snapshot!("action_fibonacci", format_tokens(tokens));
}
