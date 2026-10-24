//! Checks the shape of `api-parity.toml`, the per-binding API parity table.
//!
//! The table has one row per entity and option and one cell per binding.
//! A cell is `present` (it names a symbol), `absent` (it gives a reason) or `planned` (it gives a reason and a release).
//! These tests check the shape of the file. Other tests in this file compare it with the code.

use std::{collections::BTreeSet, fs, path::PathBuf};

use toml::Value;

const BINDINGS: [&str; 4] = ["rust", "python", "c", "go"];
const ENTITIES: [&str; 9] = [
    "publisher",
    "subscriber",
    "cache",
    "service_client",
    "service_server",
    "action_client",
    "action_server",
    "node",
    "context",
];
const STATES: [&str; 3] = ["present", "absent", "planned"];
const ROW_KEYS: [&str; 3] = ["id", "entity", "option"];
const CELL_KEYS: [&str; 4] = ["status", "symbol", "reason", "release"];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parity_text() -> String {
    let path = workspace_root().join("api-parity.toml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// A release is three dot-separated numbers, such as `0.2.2`.
fn release_is_well_formed(release: &str) -> bool {
    let parts: Vec<&str> = release.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn non_empty_str<'a>(table: &'a toml::value::Table, key: &str) -> Option<&'a str> {
    table
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn check_cell(row_id: &str, binding: &str, cell: Option<&Value>, errors: &mut Vec<String>) {
    let at = format!("row `{row_id}`, binding `{binding}`");
    let Some(table) = cell.and_then(Value::as_table) else {
        errors.push(format!("{at}: missing cell"));
        return;
    };
    for key in table.keys() {
        if !CELL_KEYS.contains(&key.as_str()) {
            errors.push(format!("{at}: unknown key `{key}`"));
        }
    }
    let status = match table.get("status").and_then(Value::as_str) {
        Some(s) if STATES.contains(&s) => s,
        Some(s) => {
            errors.push(format!("{at}: status `{s}` is not one of {STATES:?}"));
            return;
        }
        None => {
            errors.push(format!("{at}: missing status"));
            return;
        }
    };
    let symbol = non_empty_str(table, "symbol");
    let reason = non_empty_str(table, "reason");
    let release = non_empty_str(table, "release");
    match status {
        "present" => {
            if symbol.is_none() {
                errors.push(format!("{at}: a present cell needs a symbol"));
            }
            if reason.is_some() || release.is_some() {
                errors.push(format!("{at}: a present cell has no reason or release"));
            }
        }
        "absent" => {
            if reason.is_none() {
                errors.push(format!("{at}: an absent cell needs a reason"));
            }
            if symbol.is_some() || release.is_some() {
                errors.push(format!("{at}: an absent cell has no symbol or release"));
            }
        }
        _ => {
            if reason.is_none() {
                errors.push(format!("{at}: a planned cell needs a reason"));
            }
            match release {
                None => errors.push(format!("{at}: a planned cell needs a release")),
                Some(r) if !release_is_well_formed(r) => {
                    errors.push(format!(
                        "{at}: release `{r}` is not a version such as 0.2.2"
                    ));
                }
                Some(_) => {}
            }
            if symbol.is_some() {
                errors.push(format!("{at}: a planned cell has no symbol"));
            }
        }
    }
}

/// Parses the file and returns its rows, or the defects that stop the parse.
fn parse_rows(text: &str) -> Result<Vec<Value>, Vec<String>> {
    let doc: Value = text
        .parse()
        .map_err(|e| vec![format!("not valid TOML: {e}")])?;
    match doc.get("row").and_then(Value::as_array) {
        Some(rows) if !rows.is_empty() => Ok(rows.clone()),
        _ => Err(vec!["no `[[row]]` entries".to_string()]),
    }
}

/// Returns every shape defect in the rows. An empty list means the shape is valid.
fn validate_shape(text: &str) -> Vec<String> {
    let rows = match parse_rows(text) {
        Ok(rows) => rows,
        Err(errors) => return errors,
    };
    let mut errors = Vec::new();
    let mut ids = BTreeSet::new();
    let mut rust_symbols = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(table) = row.as_table() else {
            errors.push(format!("row {index}: not a table"));
            continue;
        };
        let Some(id) = non_empty_str(table, "id") else {
            errors.push(format!("row {index}: missing id"));
            continue;
        };
        if !ids.insert(id.to_string()) {
            errors.push(format!("row `{id}`: duplicate id"));
        }
        match non_empty_str(table, "entity") {
            Some(entity) if ENTITIES.contains(&entity) => {}
            Some(entity) => errors.push(format!(
                "row `{id}`: entity `{entity}` is not one of {ENTITIES:?}"
            )),
            None => errors.push(format!("row `{id}`: missing entity")),
        }
        if non_empty_str(table, "option").is_none() {
            errors.push(format!("row `{id}`: missing option"));
        }
        for key in table.keys() {
            if !ROW_KEYS.contains(&key.as_str()) && !BINDINGS.contains(&key.as_str()) {
                errors.push(format!("row `{id}`: unknown key `{key}`"));
            }
        }
        for binding in BINDINGS {
            check_cell(id, binding, table.get(binding), &mut errors);
        }
        if let Some(symbol) = rust_symbol(row) {
            if !rust_symbols.insert(symbol.to_string()) {
                errors.push(format!(
                    "row `{id}`: Rust symbol `{symbol}` has a row already"
                ));
            }
        }
    }
    errors
}

/// The Rust symbol a row names, when the Rust cell is `present`.
fn rust_symbol(row: &Value) -> Option<&str> {
    let cell = row.get("rust")?;
    if cell.get("status")?.as_str()? != "present" {
        return None;
    }
    cell.get("symbol")?.as_str()
}

/// Returns the entities that have no row.
fn missing_entities(rows: &[Value]) -> Vec<&'static str> {
    let present: BTreeSet<&str> = rows
        .iter()
        .filter_map(|row| row.get("entity").and_then(Value::as_str))
        .collect();
    ENTITIES
        .iter()
        .copied()
        .filter(|e| !present.contains(e))
        .collect()
}

#[test]
fn parity_file_has_valid_shape() {
    let text = parity_text();
    let errors = validate_shape(&text);
    assert!(
        errors.is_empty(),
        "api-parity.toml defects:\n{}",
        errors.join("\n")
    );
}

#[test]
fn parity_file_has_a_row_for_every_entity() {
    let rows = parse_rows(&parity_text()).expect("api-parity.toml parses");
    let missing = missing_entities(&rows);
    assert!(missing.is_empty(), "no row for entity: {missing:?}");
}

/// One row with four valid cells. Each negative test below breaks one thing in it.
const VALID_ROW: &str = r#"
[[row]]
id = "publisher.qos"
entity = "publisher"
option = "qos"
rust = { status = "present", symbol = "ZPubBuilder::with_qos" }
python = { status = "absent", reason = "No such option." }
c = { status = "planned", reason = "Not written yet.", release = "0.2.2" }
go = { status = "present", symbol = "PublisherBuilder.WithQoS" }
"#;

fn assert_rejected(text: &str, expected: &str) {
    let errors = validate_shape(text);
    assert!(
        errors.iter().any(|e| e.contains(expected)),
        "expected a defect containing `{expected}`, got {errors:?}"
    );
}

#[test]
fn shape_check_accepts_the_valid_row() {
    // Control for every rejection test below: the base row passes.
    let errors = validate_shape(VALID_ROW);
    assert!(errors.is_empty(), "the base row must be valid: {errors:?}");
}

#[test]
fn shape_check_rejects_an_empty_file() {
    assert_rejected("", "no `[[row]]` entries");
}

#[test]
fn shape_check_rejects_invalid_toml() {
    assert_rejected("[[row]\nid =", "not valid TOML");
}

#[test]
fn shape_check_rejects_absent_without_reason() {
    let text = VALID_ROW.replace(
        r#"{ status = "absent", reason = "No such option." }"#,
        r#"{ status = "absent" }"#,
    );
    assert_rejected(&text, "an absent cell needs a reason");
}

#[test]
fn shape_check_rejects_planned_without_release() {
    let text = VALID_ROW.replace(r#", release = "0.2.2""#, "");
    assert_rejected(&text, "a planned cell needs a release");
}

#[test]
fn shape_check_rejects_planned_with_malformed_release() {
    let text = VALID_ROW.replace(r#"release = "0.2.2""#, r#"release = "soon""#);
    assert_rejected(&text, "is not a version");
}

#[test]
fn shape_check_rejects_planned_without_reason() {
    let text = VALID_ROW.replace(r#"reason = "Not written yet.", "#, "");
    assert_rejected(&text, "a planned cell needs a reason");
}

#[test]
fn shape_check_rejects_present_without_symbol() {
    let text = VALID_ROW.replace(r#", symbol = "PublisherBuilder.WithQoS""#, "");
    assert_rejected(&text, "a present cell needs a symbol");
}

#[test]
fn shape_check_rejects_present_with_reason() {
    let text = VALID_ROW.replace(
        r#"symbol = "PublisherBuilder.WithQoS""#,
        r#"symbol = "PublisherBuilder.WithQoS", reason = "extra""#,
    );
    assert_rejected(&text, "a present cell has no reason or release");
}

#[test]
fn shape_check_rejects_an_unknown_status() {
    let text = VALID_ROW.replace(r#"status = "absent""#, r#"status = "partial""#);
    assert_rejected(&text, "status `partial` is not one of");
}

#[test]
fn shape_check_rejects_a_missing_binding_cell() {
    let text = VALID_ROW.replace(
        "go = { status = \"present\", symbol = \"PublisherBuilder.WithQoS\" }\n",
        "",
    );
    assert_rejected(&text, "binding `go`: missing cell");
}

#[test]
fn shape_check_rejects_an_unknown_entity() {
    let text = VALID_ROW.replace(r#"entity = "publisher""#, r#"entity = "parameters""#);
    assert_rejected(&text, "entity `parameters` is not one of");
}

#[test]
fn shape_check_rejects_a_duplicate_id() {
    let text = format!(
        "{VALID_ROW}{}",
        VALID_ROW.replace("ZPubBuilder::with_qos", "ZPubBuilder::other")
    );
    assert_rejected(&text, "duplicate id");
}

#[test]
fn shape_check_rejects_a_repeated_rust_symbol() {
    let text = format!(
        "{VALID_ROW}{}",
        VALID_ROW.replace("publisher.qos", "publisher.qos2")
    );
    assert_rejected(&text, "has a row already");
}

#[test]
fn entity_check_reports_every_missing_entity() {
    let rows = parse_rows(VALID_ROW).expect("base row parses");
    let missing = missing_entities(&rows);
    assert_eq!(
        missing.len(),
        ENTITIES.len() - 1,
        "only `publisher` has a row"
    );
    assert!(!missing.contains(&"publisher"));
}
