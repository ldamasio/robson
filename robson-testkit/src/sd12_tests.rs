//! SD12 (ADR-0056 step 8, 8d plan §5.3, §5.4), lexically: no environment
//! read in this crate outside `daemon_process.rs`, and `robsond`'s sources
//! name neither test venue. The structural proof that no production crate
//! depends on this one is `check-dev-only`'s, in CI.

use std::{fs, path::Path};

/// Every `.rs` file under `dir`, with its text.
fn sources(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push((path.display().to_string(), fs::read_to_string(&path).unwrap()));
        }
    }
    found
}

#[test]
fn no_environment_read_outside_the_daemon_process() {
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = concat!("env", "::", "var");
    let readers: Vec<String> = sources(&crate_src)
        .into_iter()
        .filter(|(path, text)| !path.ends_with("daemon_process.rs") && text.contains(read))
        .map(|(path, _)| path)
        .collect();
    assert!(readers.is_empty(), "environment read in {readers:?}");
}

#[test]
fn robsond_names_no_test_venue() {
    let robsond = Path::new(env!("CARGO_MANIFEST_DIR")).join("../robsond/src");
    let files = sources(&robsond);
    assert!(files.len() > 10, "robsond's sources were read");
    for name in ["BalanceStub", "FakeSpotVenue", "SpotBook"] {
        let naming: Vec<&String> = files
            .iter()
            .filter(|(_, text)| text.contains(name))
            .map(|(path, _)| path)
            .collect();
        assert!(naming.is_empty(), "{name} named in {naming:?}");
    }
}
