//! Tests that need a dedicated physical controller are classified, not
//! disabled. Each drives a private frame protocol on its stdin and stdout and
//! isolated PostgreSQL 16 roles over loopback, so it fails under any runner
//! but its controller. Their ignore reason starts with [`REASON`];
//! `scripts/test-pg.sh` skips exactly them by name (`CONTROLLER_TESTS`), and
//! keeps selecting every other ignored test.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const REASON: &str = "#[ignore = \"requires dedicated physical controller";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The function each controller reason annotates: the first `fn` after it.
fn classified(text: &str) -> Vec<String> {
    text.match_indices(REASON)
        .map(|(at, _)| {
            let rest = &text[at..];
            let name = &rest[rest.find("fn ").expect("a function follows the reason") + 3..];
            name[..name.find(['(', '<']).unwrap()].trim().to_string()
        })
        .collect()
}

/// The names in the script's `CONTROLLER_TESTS=( ... )` array.
fn skipped(script: &str) -> BTreeSet<String> {
    let start = script.find("CONTROLLER_TESTS=(").expect("CONTROLLER_TESTS in test-pg.sh");
    let body = &script[start + "CONTROLLER_TESTS=(".len()..];
    body[..body.find(')').unwrap()].split_whitespace().map(String::from).collect()
}

#[test]
fn test_pg_skips_exactly_the_controller_tests() {
    let root = workspace();
    let mut files = Vec::new();
    for entry in fs::read_dir(&root).unwrap() {
        let member = entry.unwrap().path();
        if member.join("Cargo.toml").is_file() {
            for dir in ["src", "tests", "benches", "examples"] {
                rust_files(&member.join(dir), &mut files);
            }
        }
    }
    let mut classified_tests = BTreeSet::new();
    // This file names the reason only with escaped quotes, so it is not read
    // as a classified test.
    for file in &files {
        for name in classified(&fs::read_to_string(file).unwrap()) {
            assert!(classified_tests.insert(name.clone()), "{name} is classified twice");
        }
    }
    assert!(!classified_tests.is_empty(), "the controller reason is no longer found");
    let script = fs::read_to_string(root.join("scripts/test-pg.sh")).unwrap();
    assert_eq!(
        skipped(&script),
        classified_tests,
        "test-pg.sh must skip exactly the tests that need the dedicated controller"
    );
    // Every invocation that selects ignored tests carries the skip.
    let selections = script.matches("\"${CARGO[@]}\" -- --ignored").count();
    assert_eq!(selections, 3, "{script}");
    assert_eq!(script.matches("\"${SKIP_CONTROLLER[@]}\"").count(), selections);
}

#[test]
fn the_classifier_reads_what_it_classifies() {
    let text = "#[sqlx::test(migrations = false)]\n\
                #[ignore = \"requires dedicated physical controller and more\"]\n\
                async fn first_physical(pool: PgPool) {}\n\
                #[ignore = \"requires DATABASE_URL\"]\n\
                async fn ordinary(pool: PgPool) {}\n\
                #[ignore = \"requires dedicated physical controller\"]\n\
                fn second_physical<T>() {}\n";
    assert_eq!(classified(text), ["first_physical", "second_physical"]);
    assert_eq!(
        skipped("CONTROLLER_TESTS=(\n    one\n    two\n)\n"),
        BTreeSet::from(["one".to_string(), "two".to_string()])
    );
}
