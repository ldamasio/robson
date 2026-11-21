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
    // step sets it. Every place that names the switch is pinned verbatim: the
    // script's whole default dispatch and its note on the switch, and CI's
    // whole database step. With those blocks removed, the switch's name may
    // not occur anywhere under scripts/ or .github/ or in the justfile, as
    // code, comment or data, so no read, assignment, export or move can slip
    // past a parser; a deliberate change updates a pinned block in the same
    // commit.
    let workflow = fs::read_to_string(root.join(".github/workflows/robsond.yml")).unwrap();
    for (text, block) in [
        (&script, DEFAULT_DISPATCH),
        (&script, SCRIPT_NOTE),
        (&workflow, CI_DATABASE_STEP),
    ] {
        assert_eq!(text.matches(block).count(), 1, "a pinned block is not found once:\n{block}");
    }
    assert_eq!(workflow.matches("- name: Run the DATABASE_URL-gated tests").count(), 1);
    let mut files = vec![root.join("justfile")];
    let mut dirs = vec![root.join("scripts"), root.join(".github")];
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
    let mut stray = Vec::new();
    for path in &files {
        let name = path.strip_prefix(&root).unwrap();
        let pins: &[&str] = if name == Path::new("scripts/test-pg.sh") {
            &[DEFAULT_DISPATCH, SCRIPT_NOTE]
        } else if name == Path::new(".github/workflows/robsond.yml") {
            &[CI_DATABASE_STEP]
        } else {
            &[]
        };
        let mut bytes = fs::read(path).unwrap();
        for pin in pins {
            bytes = remove_once(&bytes, pin.as_bytes());
        }
        if occurrences(&bytes) > 0 {
            stray.push(name.display().to_string());
        }
    }
    assert!(stray.is_empty(), "{SWITCH} is named outside its pinned blocks in {stray:?}");
}

const SWITCH: &str = "ROBSON_TEST_PG_SKIP_MEASUREMENTS";

/// `scripts/test-pg.sh`'s whole default dispatch, verbatim.
const DEFAULT_DISPATCH: &str = r##"else
    status=0
    info "Phase 1: every ignored test outside ${SUITE}"
    "${CARGO[@]}" -- --ignored --skip "${SUITE}" "${SKIP_CONTROLLER[@]}" "${SKIP_ISOLATED[@]}" || status=1
    info "Phase 2: ${SUITE} one test at a time"
    "${CARGO[@]}" -- --ignored "${SUITE}" "${SKIP_CONTROLLER[@]}" --test-threads=1 || status=1
    if [[ "${ROBSON_TEST_PG_SKIP_MEASUREMENTS:-0}" == "1" ]]; then
        warn "Phase 3 skipped (ROBSON_TEST_PG_SKIP_MEASUREMENTS=1): ${ISOLATED_TESTS[*]} run on the local gate host."
    else
        for name in "${ISOLATED_TESTS[@]}"; do
            info "Phase 3: ${name} alone"
            "${CARGO[@]}" -- --ignored "${name}" "${SKIP_CONTROLLER[@]}" --test-threads=1 || status=1
        done
    fi
    if [[ ${status} -ne 0 ]]; then
        fail "Postgres integration tests failed."
        exit 1
    fi
fi
"##;

/// `scripts/test-pg.sh`'s header note on the switch, verbatim.
const SCRIPT_NOTE: &str = r##"#   ROBSON_TEST_PG_SKIP_MEASUREMENTS=1 skips phase 3 and says so. CI sets it: a
#   shared runner's speed varies (the 60 s million-row cold audit reached 920
#   and then, alone, 848 of its 1000 pages on two runners, and passed on two
#   others), so the envelope is proven by the local gates, which run phase 3.
"##;

/// CI's database step in `.github/workflows/robsond.yml`, verbatim.
const CI_DATABASE_STEP: &str = r##"      - name: Run the DATABASE_URL-gated tests
        env:
          DATABASE_URL: postgresql://postgres:postgres@localhost:5432/robson_migration_ci
          # Phase 3's wall-clock envelopes depend on the host, and a hosted
          # runner's speed varies; the local gates run them (see test-pg.sh).
          ROBSON_TEST_PG_SKIP_MEASUREMENTS: "1"
        # The canonical script runs everything else with the default
        # concurrency, then the migration38 suite (bounded_migration::public_pg::)
        # one test at a time.
        run: bash scripts/test-pg.sh
"##;

/// How many times [`SWITCH`] occurs in `bytes`, whatever surrounds it.
fn occurrences(bytes: &[u8]) -> usize {
    bytes.windows(SWITCH.len()).filter(|w| *w == SWITCH.as_bytes()).count()
}

/// `bytes` without the first occurrence of `block` (unchanged if absent).
fn remove_once(bytes: &[u8], block: &[u8]) -> Vec<u8> {
    match bytes.windows(block.len()).position(|w| w == block) {
        Some(at) => [&bytes[..at], &bytes[at + block.len()..]].concat(),
        None => bytes.to_vec(),
    }
}

#[test]
fn the_switch_scan_counts_every_mention() {
    assert_eq!(occurrences(b"# ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"), 1);
    assert_eq!(occurrences(b"run: x # ROBSON_TEST_PG_SKIP_MEASUREMENTS"), 1);
    assert_eq!(
        occurrences(b"echo \"$(echo \" #\")\"; export ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"),
        1
    );
    assert_eq!(occurrences(b"\0ROBSON_TEST_PG_SKIP_MEASUREMENTS\xff"), 1);
    assert_eq!(occurrences(DEFAULT_DISPATCH.as_bytes()), 2);
    assert_eq!(occurrences(SCRIPT_NOTE.as_bytes()), 1);
    assert_eq!(occurrences(CI_DATABASE_STEP.as_bytes()), 1);
    assert_eq!(remove_once(b"aXYbXY", b"XY"), b"abXY");
    assert_eq!(remove_once(b"ab", b"XY"), b"ab");
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
