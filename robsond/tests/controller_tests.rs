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
    collections::{BTreeMap, BTreeSet},
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
    // step sets it. Both places are pinned verbatim: the script's whole
    // default dispatch and the whole CI step. Moving another phase under the
    // switch, or the setting into another step, changes a pinned block and
    // fails here; a deliberate change updates the block in the same commit.
    let workflow = fs::read_to_string(root.join(".github/workflows/robsond.yml")).unwrap();
    assert_eq!(script.matches(DEFAULT_DISPATCH).count(), 1, "{script}");
    assert_eq!(workflow.matches(CI_DATABASE_STEP).count(), 1, "{workflow}");
    assert_eq!(workflow.matches("- name: Run the DATABASE_URL-gated tests").count(), 1);
    // Outside comments the switch appears only in those pinned blocks: twice
    // in the script (the test and its warning) and once in the workflow. Any
    // other read, assignment or export, in the scripts, the justfile or any
    // workflow, fails here.
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
    let mut uses = BTreeMap::new();
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
        let count = switch_uses(&fs::read(path).unwrap());
        if count > 0 {
            uses.insert(name, count);
        }
    }
    assert_eq!(
        uses,
        BTreeMap::from([
            (".github/workflows/robsond.yml".to_string(), 1),
            ("scripts/test-pg.sh".to_string(), 2),
        ]),
        "the switch may be read by phase 3 and set only by CI's database step"
    );
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

/// Lines of a text file that name [`SWITCH`] outside a comment. A file with a
/// NUL byte, or that is not UTF-8, is binary and cannot set a shell variable.
/// A shell or YAML comment starts at a `#` outside quotes that begins the line
/// or follows whitespace.
fn switch_uses(bytes: &[u8]) -> usize {
    if bytes.contains(&0) {
        return 0;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return 0;
    };
    text.lines().map(code_part).filter(|code| code.contains(SWITCH)).count()
}

/// The part of a shell or YAML line before its comment.
fn code_part(line: &str) -> &str {
    let (mut single, mut double, mut escaped) = (false, false, false);
    let mut previous = ' ';
    for (at, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if !single => escaped = true,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && previous.is_whitespace() => return &line[..at],
            _ => {},
        }
        previous = c;
    }
    line
}

#[test]
fn switch_uses_skip_comments_and_binaries_only() {
    assert_eq!(switch_uses(b"# ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"), 0);
    assert_eq!(switch_uses(b"run: x # ROBSON_TEST_PG_SKIP_MEASUREMENTS"), 0);
    assert_eq!(switch_uses(b"export ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"), 1);
    assert_eq!(switch_uses(b"echo \" #\"; export ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"), 1);
    assert_eq!(switch_uses(b"n=${#a[@]} ROBSON_TEST_PG_SKIP_MEASUREMENTS=1"), 1);
    assert_eq!(switch_uses(b"\0ROBSON_TEST_PG_SKIP_MEASUREMENTS"), 0);
    assert_eq!(switch_uses(&[0xff, b'R']), 0);
    assert_eq!(switch_uses(DEFAULT_DISPATCH.as_bytes()), 2);
    assert_eq!(switch_uses(CI_DATABASE_STEP.as_bytes()), 1);
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
