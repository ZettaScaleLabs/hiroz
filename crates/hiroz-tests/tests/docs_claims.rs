//! Every sentence of the buffer section of `docs/user-guide/interop.md` maps to a named test, or
//! says `not tested`.
//!
//! The table is `docs_claims.md`, next to this file. Each row is `| sentence | tests | note |`.
//! The test splits the section into sentences, then checks four things: each sentence has a row,
//! each row names a sentence of the section, each named test exists as a `fn` in this directory,
//! and each row says either `not tested` or one or more test names. A sentence that changes in
//! the docs fails this test until its row changes too.

#[allow(dead_code)]
mod common;

use std::{collections::BTreeSet, fs, path::PathBuf};

const NAME: &str = "docs_claims_table_has_no_untested_sentence";
const HEADING: &str = "## Buffer messages from Lyrical publishers";
const NOT_TESTED: &str = "not tested";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The text of the docs section, from its heading to the end of the file.
fn section() -> String {
    let path = crate_dir().join("../../docs/user-guide/interop.md");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("R1ASSERT {NAME}: {path:?}: {e}"));
    let start = text
        .find(HEADING)
        .unwrap_or_else(|| panic!("R1ASSERT {NAME}: heading {HEADING:?} is not in the docs"));
    text[start + HEADING.len()..].to_owned()
}

/// Split prose into sentences. A sentence ends at a full stop outside backticks that is followed
/// by a space or the end of the line. A list marker is not part of a sentence.
fn sentences(section: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        let line = line.strip_prefix("- ").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let (mut current, mut in_code) = (String::new(), false);
        for (i, &c) in chars.iter().enumerate() {
            if c == '`' {
                in_code = !in_code;
            }
            let ends = c == '.' && !in_code && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
            if ends {
                out.push(current.trim().to_owned());
                current.clear();
            } else {
                current.push(c);
            }
        }
        if !current.trim().is_empty() {
            out.push(current.trim().to_owned());
        }
    }
    out
}

/// The rows of the table: sentence and the tests cell.
fn rows(table: &str) -> Vec<(String, String)> {
    table
        .lines()
        .filter(|l| l.trim_start().starts_with('|'))
        .filter(|l| !l.chars().all(|c| matches!(c, '|' | '-' | ' ')))
        .map(|l| {
            let cells: Vec<String> = l
                .trim()
                .trim_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_owned())
                .collect();
            (cells[0].clone(), cells.get(1).cloned().unwrap_or_default())
        })
        .filter(|(s, t)| s != "sentence" && t != "tests")
        .collect()
}

/// Every `fn name(` of the test files in this directory, apart from this file.
fn test_fns() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(crate_dir().join("tests")).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("docs_claims.rs") {
            for line in fs::read_to_string(&path).unwrap().lines() {
                if let Some(rest) = line.trim_start().strip_prefix("fn ") {
                    names.insert(rest.split('(').next().unwrap().trim().to_owned());
                }
            }
        }
    }
    names
}

#[test]
fn docs_claims_table_has_no_untested_sentence() {
    let sentences = sentences(&section());
    r1_assert!(
        sentences.len() >= 10,
        NAME,
        "the section split into {} sentences; the splitter or the docs changed",
        sentences.len()
    );
    let table_path = crate_dir().join("tests/docs_claims.md");
    let table = fs::read_to_string(&table_path).unwrap_or_default();
    let rows = rows(&table);
    r1_assert!(
        !rows.is_empty(),
        NAME,
        "no table at {table_path:?}, or it has no rows"
    );

    let known = test_fns();
    let mut problems = Vec::new();
    for s in &sentences {
        if !rows.iter().any(|(r, _)| r == s) {
            problems.push(format!("no row for sentence: {s}"));
        }
    }
    for (sentence, tests) in &rows {
        if !sentences.contains(sentence) {
            problems.push(format!("row names no sentence of the docs: {sentence}"));
        }
        let tests = tests.replace('`', "");
        if tests.trim().is_empty() {
            problems.push(format!("row has no test and no `{NOT_TESTED}`: {sentence}"));
        } else if tests.trim() != NOT_TESTED {
            for name in tests.split(',').map(str::trim) {
                if !known.contains(name) {
                    problems.push(format!("test {name} does not exist (sentence: {sentence})"));
                }
            }
        }
    }
    r1_assert!(
        problems.is_empty(),
        NAME,
        "{} problems: {}",
        problems.len(),
        problems.join("; ")
    );
}
