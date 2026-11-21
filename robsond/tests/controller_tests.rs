//! Tests that need a dedicated physical controller are classified, not
//! disabled. Each drives a private frame protocol on its stdin and stdout and
//! isolated PostgreSQL 16 roles over loopback, so it fails under any runner
//! but its controller. Their ignore reason starts with [`REASON`];
//! `scripts/test-pg.sh` skips exactly them by name (`CONTROLLER_TESTS`), and
//! keeps selecting every other ignored test.
//!
//! Measurement runs whose ignore reason starts with [`ISOLATED_REASON`] assert
//! a wall-clock envelope of their host, so the script keeps them out of the
//! concurrent first phase and runs each alone in phase 3 (`ISOLATED_TESTS`).
//! Only CI's database step skips phase 3 (`ROBSON_TEST_PG_SKIP_MEASUREMENTS`);
//! the local gates run it.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const REASON: &str = "#[ignore = \"requires dedicated physical controller";
const ISOLATED_REASON: &str = "#[ignore = \"requires isolated PG";

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

/// The function each `reason` annotates: the first `fn` after it.
fn classified(text: &str, reason: &str) -> Vec<String> {
    text.match_indices(reason)
        .map(|(at, _)| {
            let rest = &text[at..];
            let name = &rest[rest.find("fn ").expect("a function follows the reason") + 3..];
            name[..name.find(['(', '<']).unwrap()].trim().to_string()
        })
        .collect()
}

/// The names in the script's `<array>=( ... )` array.
fn skipped(script: &str, array: &str) -> BTreeSet<String> {
    let open = format!("{array}=(");
    let start = script.find(&open).unwrap_or_else(|| panic!("{array} in test-pg.sh"));
    let body = &script[start + open.len()..];
    body[..body.find(')').unwrap()].split_whitespace().map(String::from).collect()
}

/// Every test in the workspace that `reason` annotates, each once.
fn classified_in_workspace(root: &Path, reason: &str) -> BTreeSet<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let member = entry.unwrap().path();
        if member.join("Cargo.toml").is_file() {
            for dir in ["src", "tests", "benches", "examples"] {
                rust_files(&member.join(dir), &mut files);
            }
        }
    }
    let mut tests = BTreeSet::new();
    // This file names the reasons only with escaped quotes, so it is not read
    // as a classified test.
    for file in &files {
        for name in classified(&fs::read_to_string(file).unwrap(), reason) {
            assert!(tests.insert(name.clone()), "{name} is classified twice");
        }
    }
    tests
}

#[test]
fn test_pg_skips_exactly_the_controller_tests() {
    let root = workspace();
    let classified_tests = classified_in_workspace(&root, REASON);
    assert!(!classified_tests.is_empty(), "the controller reason is no longer found");
    let script = fs::read_to_string(root.join("scripts/test-pg.sh")).unwrap();
    assert_eq!(
        skipped(&script, "CONTROLLER_TESTS"),
        classified_tests,
        "test-pg.sh must skip exactly the tests that need the dedicated controller"
    );
    // Every invocation that selects ignored tests carries the skip.
    let selections = script.matches("\"${CARGO[@]}\" -- --ignored").count();
    assert_eq!(selections, 4, "{script}");
    assert_eq!(script.matches("\"${SKIP_CONTROLLER[@]}\"").count(), selections);
}

#[test]
fn test_pg_runs_exactly_the_isolated_tests_alone() {
    let root = workspace();
    let isolated = classified_in_workspace(&root, ISOLATED_REASON);
    assert!(!isolated.is_empty(), "the isolated reason is no longer found");
    let script = fs::read_to_string(root.join("scripts/test-pg.sh")).unwrap();
    assert_eq!(
        skipped(&script, "ISOLATED_TESTS"),
        isolated,
        "test-pg.sh must isolate exactly the tests that measure an envelope"
    );
    // Phase 1, the only concurrent selection, skips them; phase 3 runs each
    // by name with one thread.
    assert_eq!(script.matches("\"${SKIP_ISOLATED[@]}\"").count(), 1, "{script}");
    let phase1 = script.lines().find(|l| l.contains("--skip \"${SUITE}\"")).unwrap();
    assert!(phase1.contains("\"${SKIP_ISOLATED[@]}\""), "{phase1}");
    assert!(script.contains("for name in \"${ISOLATED_TESTS[@]}\"; do"));
    assert!(script.contains("-- --ignored \"${name}\" \"${SKIP_CONTROLLER[@]}\" --test-threads=1"));
    // Phase 3 is skipped only on the explicit switch, and only CI's database
    // step sets it: every other caller of the script still runs the envelopes.
    const SWITCH: &str = "ROBSON_TEST_PG_SKIP_MEASUREMENTS";
    let gate = format!(
        "    if [[ \"${{{SWITCH}:-0}}\" == \"1\" ]]; then\n        warn \"Phase 3 skipped \
         ({SWITCH}=1): ${{ISOLATED_TESTS[*]}} run on the local gate host.\"\n    else\n        \
         for name in \"${{ISOLATED_TESTS[@]}}\"; do\n"
    );
    assert!(script.contains(&gate), "the switch must gate phase 3 only:\n{script}");
    // Outside comments the switch appears exactly where something may read or
    // set it: the two gate lines of test-pg.sh and the one setting in CI's
    // database step. Any other read, assignment or export, in the script, the
    // other scripts, the justfile or any workflow, fails here.
    let mut uses = Vec::new();
    let mut files = vec![root.join("justfile")];
    let mut dirs = vec![root.join("scripts"), root.join(".github/workflows")];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
        // Bytecode caches and other binaries cannot set a shell variable.
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut step = String::new();
        for line in text.lines().map(str::trim) {
            if let Some(title) = line.strip_prefix("- name:") {
                step = title.trim().to_string();
            }
            if line.contains(SWITCH) && !line.starts_with('#') {
                let place = if name.ends_with(".yml") {
                    step.clone()
                } else {
                    String::new()
                };
                uses.push((name.clone(), place, line.to_string()));
            }
        }
    }
    uses.sort();
    let gate_lines: Vec<&str> =
        gate.lines().map(str::trim).filter(|l| l.contains(SWITCH)).collect();
    let mut expected = vec![(
        ".github/workflows/robsond.yml".to_string(),
        "Run the DATABASE_URL-gated tests".to_string(),
        format!("{SWITCH}: \"1\""),
    )];
    for line in gate_lines {
        expected.push(("scripts/test-pg.sh".to_string(), String::new(), line.to_string()));
    }
    expected.sort();
    assert_eq!(
        uses, expected,
        "the switch may be read by phase 3 and set only by CI's database step"
    );
}

#[test]
fn the_classifier_reads_what_it_classifies() {
    let text = "#[sqlx::test(migrations = false)]\n\
                #[ignore = \"requires dedicated physical controller and more\"]\n\
                async fn first_physical(pool: PgPool) {}\n\
                #[ignore = \"requires DATABASE_URL\"]\n\
                async fn ordinary(pool: PgPool) {}\n\
                #[ignore = \"requires dedicated physical controller\"]\n\
                fn second_physical<T>() {}\n\
                #[ignore = \"requires isolated PG and a measurement\"]\n\
                async fn measured(pool: PgPool) {}\n";
    assert_eq!(classified(text, REASON), ["first_physical", "second_physical"]);
    assert_eq!(classified(text, ISOLATED_REASON), ["measured"]);
    assert_eq!(
        skipped("CONTROLLER_TESTS=(\n    one\n    two\n)\n", "CONTROLLER_TESTS"),
        BTreeSet::from(["one".to_string(), "two".to_string()])
    );
    assert_eq!(
        skipped("ISOLATED_TESTS=(\n    three\n)\n", "ISOLATED_TESTS"),
        BTreeSet::from(["three".to_string()])
    );
}
