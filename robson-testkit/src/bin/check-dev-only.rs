//! `cargo metadata --offline --locked --format-version 1 | check-dev-only`:
//! exits 0 only when `robson-testkit` is reached by dev-dependency edges
//! alone, and the input is a real workspace's metadata
//! ([`robson_testkit::dependency_check`]). Every violation, or why the input
//! proves nothing, goes to stderr, and the exit is 1.

use std::{io::Read, process::ExitCode};

use robson_testkit::dependency_check;

fn main() -> ExitCode {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("check-dev-only: reading stdin failed: {error}");
        return ExitCode::FAILURE;
    }
    match dependency_check::check(&input) {
        Ok(summary) => {
            println!(
                "check-dev-only: {} is reached only by dev edges, from {}",
                dependency_check::TESTKIT,
                summary.dev_dependents.join(", ")
            );
            ExitCode::SUCCESS
        },
        Err(problems) => {
            for problem in problems {
                eprintln!("check-dev-only: {problem}");
            }
            ExitCode::FAILURE
        },
    }
}
