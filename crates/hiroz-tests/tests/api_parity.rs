//! Checks the shape of `api-parity.toml`, the per-binding API parity table.
//!
//! The table has one row per entity and option and one cell per binding.
//! A cell is `present` (it names a symbol), `absent` (it gives a reason) or `planned` (it gives a reason and a release).
//! These tests check the shape of the file. Other tests in this file compare it with the code.
//! A `present` Rust cell must name a public method of a builder type, found by a scan of `crates/hiroz/src`.
//! Each identifier of a `present` Python, C or Go symbol must be defined in the source of that binding, outside comments and strings.
//! In Rust source (Python and C) a definition is one of these:
//! - the name after `fn`, `struct`, `enum`, `union` or `trait`;
//! - the name after `type`, `const` or `static`, when it has the form of a declaration (`type X =`, `const X:`);
//! - a parameter `name: Type` in the parentheses that follow `fn <name>`;
//! - a field `name: Type` in the braces of a `struct` or `union`;
//! - for Python, a parameter `name =` in `#[pyo3(signature = (...))]`, and the string of `name = "..."` in a `#[pyclass(...)]` or `#[pyo3(...)]` rename attribute.
//!
//! In Go source a definition is a `func` line (a function or a method) or a `type` line.
//! A use of the identifier, such as a call, a struct literal, a `let` binding, a closure parameter or a format argument, does not count.
//!
//! The check does not tie a method to its type or a parameter to its function: a parameter or field name counts when any function or struct of the binding has it.
//! It does not look at enum variants, fields of enum variants, `impl` blocks, macro-generated items or Go struct fields and parameters, and it does not run the binding.
//! Comments and strings are removed first. Rust block comments nest and Rust raw strings are handled; Go block comments do not nest and Go raw strings use backticks.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

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
        if let Some(symbol) = rust_symbol(row)
            && !rust_symbols.insert(symbol.to_string())
        {
            errors.push(format!(
                "row `{id}`: Rust symbol `{symbol}` has a row already"
            ));
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

// ---------------------------------------------------------------------------
// Comparison with the code
//
// The Rust check reads the source of `crates/hiroz/src`. It does not use rustdoc JSON, so it needs no nightly toolchain and no build of `hiroz`.
// ---------------------------------------------------------------------------

/// Builder types whose public methods need a row, with the entity that each one builds.
const BUILDER_ENTITIES: [(&str, &str); 10] = [
    ("ZPubBuilder", "publisher"),
    ("ZSubBuilder", "subscriber"),
    ("ZCacheBuilder", "cache"),
    ("ZClientBuilder", "service_client"),
    ("ZServerBuilder", "service_server"),
    ("ZActionClientBuilder", "action_client"),
    ("ZActionServerBuilder", "action_server"),
    ("ZNodeBuilder", "node"),
    ("ZLifecycleNodeBuilder", "node"),
    ("ZContextBuilder", "context"),
];

/// Builder types that configure a zenoh session. They build no entity, so they have no rows.
const NON_ENTITY_BUILDERS: [&str; 2] = ["RouterConfigBuilder", "SessionConfigBuilder"];

/// The part of the Rust source that the scan reads.
const RUST_SOURCE: &str = "crates/hiroz/src";

/// The source of each binding, with the file extension to read. Rust symbols are checked by the scan.
const BINDING_SOURCES: [(&str, &str, &str); 3] = [
    ("python", "crates/hiroz-py/src", "rs"),
    ("c", "crates/hiroz/src/ffi", "rs"),
    ("go", "crates/hiroz-go/hiroz", "go"),
];

/// Returns the leading identifier of `text`.
fn ident_prefix(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

/// Returns the self type of an `impl` header line. For `impl<T, S> ZPubBuilder<T, S> {` it returns `ZPubBuilder`.
fn impl_self_type(header: &str) -> Option<String> {
    let mut rest = header.strip_prefix("impl")?;
    if !(rest.starts_with('<') || rest.starts_with(' ')) {
        return None;
    }
    if rest.starts_with('<') {
        let mut depth = 0usize;
        let mut previous = ' ';
        let mut end = None;
        for (index, c) in rest.char_indices() {
            match c {
                '<' => depth += 1,
                // The `>` of `->` does not close a generic list.
                '>' if previous != '-' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index + 1);
                        break;
                    }
                }
                _ => {}
            }
            previous = c;
        }
        rest = &rest[end?..];
    }
    let rest = rest.trim_start();
    let target = match rest.find(" for ") {
        Some(index) => &rest[index + 5..],
        None => rest,
    };
    let name = ident_prefix(target.trim_start());
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Returns the text after `fn ` for a public function line. It accepts `pub fn`, `pub async fn`, `pub const fn` and `pub unsafe fn`.
fn pub_fn_rest(line: &str) -> Option<&str> {
    let mut rest = line.strip_prefix("pub ")?;
    for qualifier in ["const ", "async ", "unsafe "] {
        rest = rest.strip_prefix(qualifier).unwrap_or(rest);
    }
    rest.strip_prefix("fn ")
}

/// Adds `Type::method` to `found` for each `pub fn` in an `impl` block of a type whose name ends in `Builder`.
/// Adds `Type::with_type_info` for each `impl_with_type_info!` call on such a type.
/// It reads `impl` headers at the start of a line, which is where rustfmt puts them.
/// A header that spans lines is joined up to the line with its opening brace.
fn scan_source(text: &str, found: &mut BTreeSet<String>) {
    let mut owner: Option<String> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut next = 0;
    while next < lines.len() {
        let line = lines[next];
        next += 1;
        if let Some(rest) = line.strip_prefix("impl_with_type_info!(") {
            let name = ident_prefix(rest);
            if name.ends_with("Builder") {
                found.insert(format!("{name}::with_type_info"));
            }
            continue;
        }
        if line.starts_with("impl<") || line.starts_with("impl ") {
            let mut header = line.to_string();
            while !header.contains('{') && !header.trim_end().ends_with(';') && next < lines.len() {
                header.push(' ');
                header.push_str(lines[next].trim());
                next += 1;
            }
            owner = impl_self_type(&header).filter(|name| name.ends_with("Builder"));
            continue;
        }
        if line.trim_end() == "}" {
            owner = None;
            continue;
        }
        if let Some(owner) = &owner
            && let Some(rest) = pub_fn_rest(line.trim_start())
        {
            let name = ident_prefix(rest);
            if !name.is_empty() {
                found.insert(format!("{owner}::{name}"));
            }
        }
    }
}

fn collect_files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_files(&path, extension, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(extension)
            && !path.to_string_lossy().ends_with("_test.go")
        {
            out.push(path);
        }
    }
}

/// Reads every file with `extension` under `dir`. Returns the file count and the text of all files.
fn read_tree(dir: &Path, extension: &str) -> (usize, String) {
    let mut files = Vec::new();
    collect_files(dir, extension, &mut files);
    files.sort();
    let mut text = String::new();
    for file in &files {
        let content = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        text.push_str(&content);
        text.push('\n');
    }
    (files.len(), text)
}

/// Scans the Rust source of `hiroz` for the public methods of builder types.
fn scan_builders() -> BTreeSet<String> {
    let dir = workspace_root().join(RUST_SOURCE);
    let mut files = Vec::new();
    collect_files(&dir, "rs", &mut files);
    files.sort();
    let mut found = BTreeSet::new();
    for file in &files {
        let content = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        scan_rust_file(&content, &mut found);
    }
    found
}

/// Scans one Rust file. Comments and string literals become blanks first, so that a
/// declaration inside one is not read as code and a `}` inside a raw string does not end an `impl` block.
fn scan_rust_file(content: &str, found: &mut BTreeSet<String>) {
    scan_source(&blank_source(content, Lang::Rust, false), found);
}

/// Returns each way in which the scanned builder methods and the Rust cells of the rows disagree.
///
/// - A public `with_*` method of an entity builder needs a row that names it, under the right entity.
/// - A row that names a Rust symbol needs a public method of a builder type with that name.
/// - A scan that finds no `with_*` method is a defect, because it would pass for any file.
fn check_builders(scan: &BTreeSet<String>, rows: &[Value]) -> Vec<String> {
    let mut errors = Vec::new();
    if !scan.iter().any(|symbol| symbol.contains("::with_")) {
        errors.push("the scan found no `with_*` builder method".to_string());
    }
    let mut cited: BTreeMap<&str, &str> = BTreeMap::new();
    for row in rows {
        let entity = row.get("entity").and_then(Value::as_str);
        if let (Some(symbol), Some(entity)) = (rust_symbol(row), entity) {
            cited.insert(symbol, entity);
        }
    }
    for symbol in scan {
        let Some((owner, method)) = symbol.split_once("::") else {
            continue;
        };
        if !method.starts_with("with_") || NON_ENTITY_BUILDERS.contains(&owner) {
            continue;
        }
        let Some((_, entity)) = BUILDER_ENTITIES
            .iter()
            .find(|(builder, _)| *builder == owner)
        else {
            errors.push(format!(
                "`{symbol}`: `{owner}` is not in BUILDER_ENTITIES. Add it there, or add it to NON_ENTITY_BUILDERS, and add rows."
            ));
            continue;
        };
        match cited.get(symbol.as_str()) {
            None => errors.push(format!("`{symbol}` has no row in api-parity.toml")),
            Some(row_entity) if row_entity != entity => errors.push(format!(
                "`{symbol}` is under entity `{row_entity}`, but `{owner}` builds `{entity}`"
            )),
            Some(_) => {}
        }
    }
    for symbol in cited.keys() {
        if !scan.contains(*symbol) {
            errors.push(format!(
                "a row names Rust symbol `{symbol}`, which is not a public method of a builder type"
            ));
        }
    }
    errors
}

/// The language of a source file, for the rules of comments and strings.
#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Rust,
    Go,
}

/// Pushes a blank for `c`. A line break stays a line break.
/// A blank has the same length in bytes as `c`, so byte offsets stay stable.
fn push_blank(out: &mut String, c: char, fill: char) {
    if c == '\n' {
        out.push('\n');
    } else {
        for _ in 0..c.len_utf8() {
            out.push(fill);
        }
    }
}

/// Returns the number of `#` of a Rust raw string that starts at `chars[i]`, or `None`.
/// A raw string starts at an `r` that is not the end of an identifier. `br` starts a byte raw string.
fn raw_string_hashes(chars: &[char], i: usize) -> Option<usize> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let before = i.checked_sub(1).map(|p| chars[p]);
    let before2 = i.checked_sub(2).map(|p| chars[p]);
    let after_byte_prefix = before == Some('b') && !before2.is_some_and(is_ident);
    let ends_identifier = before.is_some_and(is_ident) && !after_byte_prefix;
    if chars[i] != 'r' || ends_identifier {
        return None;
    }
    let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
    (chars.get(i + 1 + hashes) == Some(&'"')).then_some(hashes)
}

/// Replaces comments, string literals and character literals with blanks.
/// The result has the same length in bytes as `source`, and keeps the line breaks.
///
/// - Both languages: `//` comments, `"` strings with backslash escapes and character literals such as `'"'`, `'\u{22}'` and `'\x22'`. A lifetime such as `'a` is code.
/// - Rust: `/* */` comments nest, and raw strings such as `r#"..."#` take any number of `#`.
/// - Go: `/* */` comments do not nest, and a backtick string has no escapes.
///
/// A comment becomes spaces. A string keeps its characters as spaces, or, when `mark_strings` is set, as `\u{1}` between two `"`.
/// A marked string keeps its position, so the caller reads its text from `source` at the same offsets.
fn blank_source(source: &str, lang: Lang, mark_strings: bool) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let content = if mark_strings { '\u{1}' } else { ' ' };
    let quote = if mark_strings { '"' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                push_blank(&mut out, chars[i], ' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 1;
            out.push_str("  ");
            i += 2;
            while i < chars.len() && depth > 0 {
                let pair = (chars[i], chars.get(i + 1).copied());
                if lang == Lang::Rust && pair == ('/', Some('*')) {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if pair == ('*', Some('/')) {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                } else {
                    push_blank(&mut out, chars[i], ' ');
                    i += 1;
                }
            }
        } else if let (Lang::Rust, Some(hashes)) = (lang, raw_string_hashes(&chars, i)) {
            out.push_str(&" ".repeat(1 + hashes));
            out.push(quote);
            i += hashes + 2;
            while i < chars.len() {
                let closes = chars[i] == '"'
                    && chars[i + 1..].iter().take_while(|&&h| h == '#').count() >= hashes;
                if closes {
                    out.push(quote);
                    out.push_str(&" ".repeat(hashes));
                    i += hashes + 1;
                    break;
                }
                push_blank(&mut out, chars[i], content);
                i += 1;
            }
        } else if c == '"' || (c == '`' && lang == Lang::Go) {
            out.push(quote);
            i += 1;
            while i < chars.len() && chars[i] != c {
                let escaped = c == '"' && chars[i] == '\\';
                push_blank(&mut out, chars[i], content);
                i += 1;
                if escaped && i < chars.len() {
                    push_blank(&mut out, chars[i], content);
                    i += 1;
                }
            }
            out.push(quote);
            i += 1;
        } else if c == '\'' {
            // A character literal has one character, or an escape that ends at the next `'`. A lifetime has neither.
            let len = match next {
                Some('\\') => (i + 3..chars.len())
                    .take_while(|&j| chars[j] != '\n')
                    .find(|&j| chars[j] == '\'')
                    .map(|j| j + 1 - i),
                Some(n) if n != '\'' && n != '\n' && chars.get(i + 2) == Some(&'\'') => Some(3),
                _ => None,
            };
            match len {
                Some(len) => {
                    for &ch in &chars[i..i + len] {
                        push_blank(&mut out, ch, ' ');
                    }
                    i += len;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Splits source text into identifiers and single punctuation characters.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if !c.is_whitespace() {
            out.push(c.to_string());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Names that a `#[pyclass(name = "...")]` or `#[pyo3(name = "...")]` attribute gives to a Python class or method.
/// The attribute is found in code without comments and strings. Its string argument is read from `raw` at the same offsets.
fn python_attribute_names(raw: &str) -> BTreeSet<String> {
    let marked = blank_source(raw, Lang::Rust, true);
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut names = BTreeSet::new();
    let mut from = 0;
    while let Some(at) = marked[from..].find("#[py") {
        let start = from + at;
        let end = marked[start..]
            .find(']')
            .map_or(marked.len(), |e| start + e);
        from = start + "#[py".len();
        for (offset, _) in marked[start..end].match_indices("name") {
            let at = start + offset;
            let boundary = !marked[..at].chars().next_back().is_some_and(is_ident);
            let rest = marked[at + "name".len()..end].trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let rest = rest.trim_start();
            // `rest` is a suffix of `marked[..end]`, so its offset in `marked` is `end - rest.len()`.
            let quote = end - rest.len();
            if !boundary || !rest.starts_with('"') || raw.as_bytes()[quote] != b'"' {
                continue;
            }
            if let Some(len) = rest[1..].find('"') {
                names.insert(raw[quote + 1..quote + 1 + len].to_string());
            }
        }
    }
    names
}

/// Returns the names that Rust source defines. `tokens` come from text without comments and strings.
///
/// - Items: the name after `fn`, `struct`, `enum`, `union` or `trait`; after `type` when `=`, `<`, `;` or `:` follows; after `const` or `static` when `:` follows.
/// - A parameter: `name:` at the top level of the parentheses that follow `fn <ident>`.
/// - A field: `name:` at the top level of the braces of a `struct` or `union`.
/// - A PyO3 parameter: `name =` inside the parentheses of `signature = (...)` in a `#[pyo3(...)]` attribute.
///
/// A struct literal, a `let` binding, a closure parameter, a format argument and a function pointer type define nothing.
fn rust_definitions(tokens: &[String]) -> BTreeSet<String> {
    let t = |i: usize| tokens.get(i).map_or("", String::as_str);
    let is_ident = |s: &str| s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    // `name :` that is not `name ::`, after one of the `before` tokens.
    let typed = |i: usize, before: &[&str]| {
        is_ident(t(i)) && t(i + 1) == ":" && t(i + 2) != ":" && i > 0 && before.contains(&t(i - 1))
    };
    // Calls `visit` on each token index at depth 1 of the group that opens at `open`. Returns nothing.
    let at_top_level = |open: usize, visit: &mut dyn FnMut(usize)| {
        let mut depth = 0;
        for k in open..tokens.len() {
            match t(k) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ if depth == 1 => visit(k),
                _ => {}
            }
            if depth == 0 {
                break;
            }
        }
    };
    // The index of the first of `opens` after `from`, outside angle brackets, before a `stop` token.
    let find_open = |from: usize, opens: &[&str], stop: &[&str]| {
        let mut angle = 0;
        for j in from..tokens.len() {
            match t(j) {
                s if angle == 0 && opens.contains(&s) => return Some(j),
                s if angle == 0 && stop.contains(&s) => return None,
                "<" => angle += 1,
                ">" if t(j - 1) != "-" => angle -= 1,
                _ => {}
            }
        }
        None
    };
    let mut names = BTreeSet::new();
    for i in 0..tokens.len() {
        let next = t(i + 1);
        match t(i) {
            "fn" if is_ident(next) => {
                names.insert(next.to_string());
                if let Some(open) = find_open(i + 2, &["("], &["{", ";"]) {
                    at_top_level(open, &mut |k| {
                        if typed(k, &["(", ",", "mut"]) {
                            names.insert(t(k).to_string());
                        }
                    });
                }
            }
            "struct" | "union" if is_ident(next) => {
                names.insert(next.to_string());
                if let Some(open) = find_open(i + 2, &["{"], &[";"]) {
                    at_top_level(open, &mut |k| {
                        if typed(k, &["{", ",", "pub", ")", "]"]) {
                            names.insert(t(k).to_string());
                        }
                    });
                }
            }
            "enum" | "trait" if is_ident(next) => {
                names.insert(next.to_string());
            }
            "type" if is_ident(next) && matches!(t(i + 2), "=" | "<" | ";" | ":") => {
                names.insert(next.to_string());
            }
            "const" | "static" => {
                let name = if next == "mut" { t(i + 2) } else { next };
                let after = if next == "mut" { t(i + 3) } else { t(i + 2) };
                if is_ident(name) && after == ":" {
                    names.insert(name.to_string());
                }
            }
            "#" if t(i + 1) == "[" && t(i + 2) == "pyo3" => {
                let mut depth = 0;
                for k in i + 1..tokens.len() {
                    match t(k) {
                        "(" | "[" | "{" => depth += 1,
                        ")" | "]" | "}" => depth -= 1,
                        // Depth 3 is the parameter list of `signature = (...)`. A deeper `name =` is
                        // an argument of a call in a default, such as `format!("{x}", x = 1)` or
                        // `format!{"{x}", x = 1}`. Braces count, so a brace-delimited call is deeper too.
                        _ if depth == 3
                            && is_ident(t(k))
                            && t(k + 1) == "="
                            && t(k + 2) != "="
                            && matches!(t(k - 1), "(" | ",") =>
                        {
                            names.insert(t(k).to_string());
                        }
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// Returns true when Go source declares `name` as a function, a method or a type.
/// `text` has no comments or strings.
fn go_defines(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("type ") {
            return ident_prefix(rest.trim_start()) == name;
        }
        let Some(mut rest) = line.strip_prefix("func ") else {
            return false;
        };
        rest = rest.trim_start();
        if rest.starts_with('(') {
            // Skip the receiver of a method.
            let Some(close) = rest.find(')') else {
                return false;
            };
            rest = rest[close + 1..].trim_start();
        }
        ident_prefix(rest) == name
    })
}

/// Returns each `present` cell of a non-Rust binding that names a symbol the binding does not define.
/// A symbol is a list of identifiers, such as `ZNode.create_publisher(qos)`.
/// Each identifier must be defined in the source of the binding.
/// For Python and C, that is an item, a parameter of a function, a field of a struct or a PyO3 name in Rust source outside comments and strings (see `rust_definitions`).
/// For Go, that is a `func`, a method or a `type` declaration outside comments and strings.
/// A use of the identifier, such as a call, does not count.
/// The check does not tie a method to its type or a parameter to its method.
fn check_binding_symbols(rows: &[Value], sources: &BTreeMap<&str, String>) -> Vec<String> {
    let mut errors = Vec::new();
    for row in rows {
        let id = row.get("id").and_then(Value::as_str).unwrap_or("?");
        for (binding, _, _) in BINDING_SOURCES {
            let Some(cell) = row.get(binding) else {
                continue;
            };
            if cell.get("status").and_then(Value::as_str) != Some("present") {
                continue;
            }
            let Some(symbol) = cell.get("symbol").and_then(Value::as_str) else {
                continue;
            };
            let raw = sources.get(binding).map(String::as_str).unwrap_or("");
            let lang = if binding == "go" {
                Lang::Go
            } else {
                Lang::Rust
            };
            let clean = blank_source(raw, lang, false);
            let mut rust_names = rust_definitions(&tokens(&clean));
            if binding == "python" {
                rust_names.extend(python_attribute_names(raw));
            }
            let identifiers = symbol
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            // A symbol such as `.` has no identifier to look up, so it would pass every source.
            if identifiers.is_empty() {
                errors.push(format!(
                    "row `{id}`, binding `{binding}`: symbol `{symbol}` names no identifier"
                ));
            }
            for identifier in identifiers {
                let defined = if binding == "go" {
                    go_defines(&clean, identifier)
                } else {
                    rust_names.contains(identifier)
                };
                if !defined {
                    errors.push(format!(
                        "row `{id}`, binding `{binding}`: `{identifier}` of symbol `{symbol}` is not defined in the source"
                    ));
                }
            }
        }
    }
    errors
}

fn binding_sources() -> BTreeMap<&'static str, String> {
    let mut sources = BTreeMap::new();
    for (binding, dir, extension) in BINDING_SOURCES {
        let (count, text) = read_tree(&workspace_root().join(dir), extension);
        // A source tree with no files would let every symbol pass.
        assert!(count > 0, "no `.{extension}` files under {dir}");
        sources.insert(binding, text);
    }
    sources
}

fn rows_of_file() -> Vec<Value> {
    parse_rows(&parity_text()).expect("api-parity.toml parses")
}

/// A row that names only a Rust symbol, for the tests that add a row.
fn rust_row(entity: &str, symbol: &str) -> Value {
    let text = format!(
        "[[row]]\nid = \"{entity}.extra\"\nentity = \"{entity}\"\noption = \"extra\"\nrust = {{ status = \"present\", symbol = \"{symbol}\" }}\n"
    );
    parse_rows(&text).expect("row parses").remove(0)
}

#[test]
fn scan_reads_impl_headers_and_macro_calls() {
    let source = r#"
impl<'a, A: ZAction> ZThingBuilder<'a, A> {
    pub fn with_one(mut self) -> Self {
        self
    }
    pub fn plain(self) {}
    pub async fn with_async(self) {}
    pub const fn with_const(self) -> Self {
        self
    }
    pub(crate) fn with_hidden(self) {}
}

impl<T> Builder for ZThingBuilder<T> {
    fn build(self) {}
}

impl<F: Fn() -> u8> ZFnBuilder<F> {
    pub fn with_f(self) {}
}

impl Other {
    pub fn with_two(self) {}
}

impl_with_type_info!(ZThingBuilder<T>);
impl_with_type_info!(NotAThing<T>);
"#;
    let mut found = BTreeSet::new();
    scan_source(source, &mut found);
    let expected: BTreeSet<String> = [
        "ZThingBuilder::with_one",
        "ZThingBuilder::plain",
        "ZThingBuilder::with_async",
        "ZThingBuilder::with_const",
        "ZThingBuilder::with_type_info",
        "ZFnBuilder::with_f",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(found, expected);
}

#[test]
fn scan_finds_the_known_builders() {
    // Positive control: a scan that finds nothing would let every other check pass.
    let scan = scan_builders();
    let with_count = scan.iter().filter(|s| s.contains("::with_")).count();
    assert!(
        with_count >= 40,
        "the scan found only {with_count} `with_*` methods: {scan:?}"
    );
    assert!(scan.contains("ZPubBuilder::with_qos"), "{scan:?}");
    assert!(scan.contains("ZNodeBuilder::with_namespace"), "{scan:?}");
    for (builder, _) in BUILDER_ENTITIES {
        let prefix = format!("{builder}::");
        assert!(
            scan.iter().any(|s| s.starts_with(&prefix)),
            "the scan found no method of `{builder}`"
        );
    }
}

#[test]
fn every_with_method_has_a_row() {
    let errors = check_builders(&scan_builders(), &rows_of_file());
    assert!(
        errors.is_empty(),
        "builder methods and api-parity.toml disagree. Add a row for each new `with_*` method.\n{}",
        errors.join("\n")
    );
}

#[test]
fn deleting_a_row_fails_the_check() {
    let scan = scan_builders();
    let mut rows = rows_of_file();
    // Control: the unmodified file passes, so the failure below comes from the deletion.
    assert!(check_builders(&scan, &rows).is_empty());
    let before = rows.len();
    rows.retain(|row| rust_symbol(row) != Some("ZPubBuilder::with_qos"));
    assert_eq!(rows.len(), before - 1, "exactly one row had that symbol");
    let errors = check_builders(&scan, &rows);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`ZPubBuilder::with_qos` has no row")),
        "{errors:?}"
    );
}

#[test]
fn a_new_with_method_fails_until_it_has_a_row() {
    let mut scan = scan_builders();
    let mut rows = rows_of_file();
    scan.insert("ZSubBuilder::with_new_option".to_string());
    let errors = check_builders(&scan, &rows);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`ZSubBuilder::with_new_option` has no row")),
        "{errors:?}"
    );
    rows.push(rust_row("subscriber", "ZSubBuilder::with_new_option"));
    assert!(check_builders(&scan, &rows).is_empty());
}

#[test]
fn a_row_under_the_wrong_entity_fails() {
    let scan = scan_builders();
    let mut rows = rows_of_file();
    rows.retain(|row| rust_symbol(row) != Some("ZSubBuilder::with_qos"));
    rows.push(rust_row("publisher", "ZSubBuilder::with_qos"));
    let errors = check_builders(&scan, &rows);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("is under entity `publisher`")),
        "{errors:?}"
    );
}

#[test]
fn a_new_builder_type_fails_until_it_is_mapped() {
    let mut scan = scan_builders();
    let rows = rows_of_file();
    scan.insert("ZNewThingBuilder::with_x".to_string());
    // A session configuration builder is not an entity, so it needs no row.
    scan.insert("SessionConfigBuilder::with_x".to_string());
    let errors = check_builders(&scan, &rows);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`ZNewThingBuilder` is not in BUILDER_ENTITIES")),
        "{errors:?}"
    );
    assert!(
        !errors.iter().any(|e| e.contains("SessionConfigBuilder")),
        "{errors:?}"
    );
}

#[test]
fn a_row_for_a_missing_method_fails() {
    let scan = scan_builders();
    let mut rows = rows_of_file();
    rows.push(rust_row("publisher", "ZPubBuilder::with_gone"));
    let errors = check_builders(&scan, &rows);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`ZPubBuilder::with_gone`, which is not a public method")),
        "{errors:?}"
    );
}

#[test]
fn an_empty_scan_fails() {
    let errors = check_builders(&BTreeSet::new(), &rows_of_file());
    assert!(
        errors.iter().any(|e| e.contains("found no `with_*`")),
        "{errors:?}"
    );
}

#[test]
fn present_symbols_exist_in_each_binding() {
    let errors = check_binding_symbols(&rows_of_file(), &binding_sources());
    assert!(
        errors.is_empty(),
        "api-parity.toml names symbols that the bindings do not have:\n{}",
        errors.join("\n")
    );
}

#[test]
fn a_missing_binding_symbol_fails() {
    let sources = binding_sources();
    let rows = parse_rows(
        r#"
[[row]]
id = "publisher.extra"
entity = "publisher"
option = "extra"
rust = { status = "present", symbol = "ZPubBuilder::with_qos" }
python = { status = "present", symbol = "ZNode.no_such_method_xyz" }
c = { status = "present", symbol = "hiroz_publisher_create" }
go = { status = "present", symbol = "PublisherBuilder.Build" }
"#,
    )
    .expect("row parses");
    let errors = check_binding_symbols(&rows, &sources);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("binding `python`") && errors[0].contains("no_such_method_xyz"));
}

#[test]
fn a_symbol_that_is_only_mentioned_fails() {
    // Negative control: a comment, a string or a call is not a definition.
    let mention = |lang_fn: &str| {
        format!(
            "// {lang_fn} Ghost()\n/* {lang_fn} Ghost() */\nconst S: &str = \"{lang_fn} Ghost()\";\nlet s = \"{lang_fn} Ghost()\"\ncaller.Ghost();\nGhost()\n"
        )
    };
    let mut sources = BTreeMap::new();
    sources.insert("python", mention("fn"));
    sources.insert("c", mention("fn"));
    sources.insert("go", mention("func"));
    let rows = parse_rows(
        r#"
[[row]]
id = "publisher.extra"
entity = "publisher"
option = "extra"
python = { status = "present", symbol = "Ghost" }
c = { status = "present", symbol = "Ghost" }
go = { status = "present", symbol = "Ghost" }
"#,
    )
    .expect("row parses");
    assert_eq!(check_binding_symbols(&rows, &sources).len(), 3);
    // Positive control: the same symbol passes when it is defined.
    sources.insert(
        "python",
        "#[pyo3(name = \"Ghost\")]\nfn ghost() {}".to_string(),
    );
    sources.insert("c", "pub unsafe extern \"C\" fn Ghost() {}".to_string());
    sources.insert("go", "func (b *B) Ghost() {}".to_string());
    let errors = check_binding_symbols(&rows, &sources);
    assert!(errors.is_empty(), "{errors:?}");
}

/// Returns true when `source` is accepted as a definition of `Ghost` for a `present` cell of `binding`.
fn ghost_defined(binding: &str, source: &str) -> bool {
    let rows = parse_rows(&format!(
        "[[row]]\nid = \"publisher.extra\"\nentity = \"publisher\"\noption = \"extra\"\n{binding} = {{ status = \"present\", symbol = \"Ghost\" }}\n"
    ))
    .expect("row parses");
    let mut sources = BTreeMap::new();
    sources.insert(binding, source.to_string());
    check_binding_symbols(&rows, &sources).is_empty()
}

#[test]
fn definition_in_a_commented_rename_fails() {
    // Negative controls: the rename attribute is in a comment or a string.
    for source in [
        "// #[pyo3(name = \"Ghost\")]\nfn ghost() {}",
        "/* #[pyo3(name = \"Ghost\")] */\nfn ghost() {}",
        "/* /* */ #[pyo3(name = \"Ghost\")] */\nfn ghost() {}",
        "const S: &str = \"#[pyo3(name = \\\"Ghost\\\")]\";\nfn ghost() {}",
    ] {
        assert!(!ghost_defined("python", source), "accepted: {source}");
    }
    // Positive controls: a live rename passes.
    for source in [
        "#[pyo3(name = \"Ghost\")]\nfn ghost() {}",
        "#[pyclass(name = \"Ghost\", module = \"m\")]\nstruct G;",
        "// note\n#[pyo3(\n    name = \"Ghost\"\n)]\nfn ghost() {}",
    ] {
        assert!(ghost_defined("python", source), "rejected: {source}");
    }
}

#[test]
fn definition_in_a_nested_rust_comment_fails() {
    for binding in ["python", "c"] {
        for source in [
            "/* /* */ fn Ghost() {} */",
            "/* /* /* */ */ fn Ghost() {} */",
        ] {
            assert!(!ghost_defined(binding, source), "accepted: {source}");
        }
        // Positive control: the comment is closed, so the function is code.
        assert!(ghost_defined(binding, "/* /* */ */ fn Ghost() {}"));
    }
}

#[test]
fn go_block_comments_do_not_nest() {
    assert!(!ghost_defined("go", "/* func Ghost() {} */"));
    // The first `*/` ends the comment in Go, so the function is code.
    assert!(ghost_defined("go", "/* /* */ func Ghost() {} */"));
}

#[test]
fn definition_after_a_rust_raw_string_is_read_correctly() {
    for binding in ["python", "c"] {
        // Negative controls: the raw string holds quotes, and a later string holds the text.
        for source in [
            "let a = r#\"q\"#; let b = r#\"x \" y\"#;\nlet s = \"fn Ghost() {}\";",
            "let b = r##\"x \"# y\"##;\nlet s = \"fn Ghost() {}\";",
            "let s = r#\"say \"hi\" fn Ghost() {}\"#;",
        ] {
            assert!(!ghost_defined(binding, source), "accepted: {source}");
        }
        // Positive controls: a real definition after a raw string with a quote.
        for source in [
            "let s = r#\"x \" y\"#;\nfn Ghost() {}",
            "let s = br##\"x \"# y\"##;\nfn Ghost() {}",
        ] {
            assert!(ghost_defined(binding, source), "rejected: {source}");
        }
    }
}

#[test]
fn go_raw_strings_hide_their_content() {
    assert!(!ghost_defined("go", "s := `say \" func Ghost() {}`"));
    assert!(ghost_defined("go", "s := `a \\\"`\nfunc Ghost() {}"));
}

#[test]
fn character_literals_and_lifetimes_do_not_confuse_the_blanker() {
    for binding in ["python", "c"] {
        let literals = "let a = '\\u{22}'; let b = '\\x22'; let c = '\\''; let d = '\"';\n";
        assert!(ghost_defined(
            binding,
            &format!("{literals}fn Ghost<'a>(x: &'a str) -> &'a str {{ x }}")
        ));
        assert!(!ghost_defined(
            binding,
            &format!("{literals}let s = \"fn Ghost() {{}}\";")
        ));
        assert!(!ghost_defined(
            binding,
            "fn f<'a>(x: &'a u8) -> &'a u8 { x }\nconst S: &str = \"fn Ghost() {}\";"
        ));
    }
    let runes = "r := '\\u00e9'\nq := '\\x41'\nz := '\\''\n";
    assert!(ghost_defined("go", &format!("{runes}func Ghost() {{}}")));
    assert!(!ghost_defined(
        "go",
        &format!("{runes}s := \"func Ghost() {{}}\"")
    ));
}

#[test]
fn a_name_that_is_not_a_parameter_or_field_fails() {
    // Negative controls: each form uses `Ghost` without defining it.
    for source in [
        "fn f() { let s = S { Ghost: 1 }; }",
        "fn f() { let Ghost: u8 = 1; }",
        "fn f() { let c = |Ghost: u8| 1; }",
        "fn f() { println!(\"{}\", Ghost = 1); }",
        "fn f(x: *const Ghost) {}",
        "fn f(x: Option<fn(Ghost: u8)>) {}",
        "struct S { a: u8 }\nfn f() { let s = S { a: 1, Ghost: 2 }; }",
    ] {
        for binding in ["python", "c"] {
            assert!(!ghost_defined(binding, source), "accepted: {source}");
        }
    }
}

#[test]
fn a_parameter_or_field_definition_passes() {
    // Positive controls for the rule above.
    for source in [
        "fn f(a: u8, Ghost: u8) {}",
        "fn f<T: Into<String>>(mut Ghost: T) {}",
        "pub extern \"C\" fn f(a: *const u8, Ghost: usize) -> i32 { 0 }",
        "struct S { a: u8, pub Ghost: u8 }",
        "struct S<T> where T: Copy { #[x] pub(crate) Ghost: T }",
        "#[pyo3(signature = (a, Ghost=None))]\nfn f() {}",
        "const Ghost: u8 = 1;",
        "static mut Ghost: u8 = 1;",
        "type Ghost = u8;",
    ] {
        for binding in ["python", "c"] {
            assert!(ghost_defined(binding, source), "rejected: {source}");
        }
    }
}

#[test]
fn multibyte_text_before_a_rename_keeps_offsets_stable() {
    // A comment, a string and a character literal with multibyte characters come before the attribute.
    for prefix in [
        "// é — note\n",
        "/* é — note */\n",
        "const S: &str = \"é — note\";\n",
        "const C: char = 'é';\n",
        "const R: &str = r#\"é — \" note\"#;\n",
    ] {
        let source = format!("{prefix}#[pyo3(name = \"Ghost\")]\nfn ghost() {{}}");
        assert!(ghost_defined("python", &source), "rejected: {source}");
    }
}

#[test]
fn builder_scan_ignores_comments_and_strings() {
    let source = r##"
impl ZRealBuilder {
    pub fn with_real(self) {}
}

/*
impl ZCommentBuilder {
    pub fn with_comment(self) {}
}
*/

const TEXT: &str = r#"
impl ZStringBuilder {
    pub fn with_string(self) {}
}
"#;

impl ZAfterBuilder {
    pub fn with_after(self) {}
}
"##;
    let mut found = BTreeSet::new();
    scan_rust_file(source, &mut found);
    let expected: BTreeSet<String> = ["ZRealBuilder::with_real", "ZAfterBuilder::with_after"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn builder_scan_keeps_methods_after_a_raw_string_with_a_closing_brace() {
    // The `}` at column zero inside the raw string used to clear the owner.
    let source = "impl ZABuilder {\n    const T: &str = r#\"\n}\n\"#;\n    pub fn with_after_string(self) {}\n}\n";
    let mut found = BTreeSet::new();
    scan_rust_file(source, &mut found);
    assert!(found.contains("ZABuilder::with_after_string"), "{found:?}");
}

#[test]
fn a_format_argument_in_a_python_default_is_not_a_definition() {
    // A negative control for the positive one above: `Ghost` is a named argument of `format!`.
    let source = "#[pyo3(signature = (a = format!(\"{Ghost}\", Ghost = 1)))]\nfn f() {}";
    assert!(!ghost_defined("python", source), "accepted: {source}");
}

#[test]
fn a_format_argument_in_a_brace_delimited_macro_is_not_a_definition() {
    // The same negative control with `format!{...}`: braces deepen the group like parentheses.
    let source = "#[pyo3(signature = (a = format!{\"{Ghost}\", Ghost = 1}))]\nfn f() {}";
    assert!(!ghost_defined("python", source), "accepted: {source}");
}

#[test]
fn builder_scan_reads_an_impl_header_that_spans_lines() {
    let source = "impl<\n    T,\n> ZSpanBuilder<T> {\n    pub fn with_span(self) {}\n}\n";
    let mut found = BTreeSet::new();
    scan_rust_file(source, &mut found);
    assert!(found.contains("ZSpanBuilder::with_span"), "{found:?}");
}

#[test]
fn a_symbol_without_an_identifier_fails() {
    // `.` splits into no identifier, so it would otherwise pass every source.
    let rows = parse_rows(
        "[[row]]\nid = \"publisher.extra\"\nentity = \"publisher\"\noption = \"extra\"\npython = { status = \"present\", symbol = \".\" }\n",
    )
    .expect("row parses");
    let mut sources = BTreeMap::new();
    sources.insert("python", "fn f() {}".to_string());
    let errors = check_binding_symbols(&rows, &sources);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("names no identifier"), "{errors:?}");
}
