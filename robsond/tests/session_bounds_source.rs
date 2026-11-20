//! T8 source guard (PLAN r4 §6), over `robsond/src` production code: the
//! files reached from `lib.rs` and `main.rs`, without comments and without
//! `#[cfg(test)]` items. Every pool and connection is built by a function that
//! applies or inherits the session bounds, only the known sites change the
//! bounds, `main` wires the venue's account kind and runs the self-check before
//! schema readiness, and a position account's pool is exactly the original
//! connect.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Code,
    Literal,
    Comment,
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// End of a string body that starts at `from`, past its closing quote.
fn string_end(bytes: &[u8], mut from: usize) -> usize {
    while from < bytes.len() {
        match bytes[from] {
            b'\\' => from += 2,
            b'"' => return from + 1,
            _ => from += 1,
        }
    }
    bytes.len()
}

/// End of a raw string `r#"…"#` (or `br…`) whose `r` is at `at`, if one starts
/// there.
fn raw_string_end(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let prefix_ok = at == 0
        || !is_ident(bytes[at - 1])
        || (bytes[at - 1] == b'b' && (at < 2 || !is_ident(bytes[at - 2])));
    if bytes[at] != b'r' || !prefix_ok {
        return None;
    }
    let hashes = bytes[at + 1..].iter().take_while(|b| **b == b'#').count();
    if bytes.get(at + 1 + hashes) != Some(&b'"') {
        return None;
    }
    let close = format!("\"{}", "#".repeat(hashes));
    let body = at + 2 + hashes;
    Some(text[body..].find(&close).map_or(bytes.len(), |end| body + end + close.len()))
}

/// End of a char literal at `at`; `None` for a lifetime.
fn char_end(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(at + 1) == Some(&b'\\') {
        return text.get(at + 3..)?.find('\'').map(|end| at + 3 + end + 1);
    }
    let next = text[at + 1..].chars().next()?;
    let after = at + 1 + next.len_utf8();
    (bytes.get(after) == Some(&b'\'')).then_some(after + 1)
}

fn block_comment_end(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0, at);
    while i + 1 < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// The text cut into code, literals and comments.
fn segments(text: &str) -> Vec<(Kind, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let token = if bytes[i..].starts_with(b"//") {
            Some((Kind::Comment, text[i..].find('\n').map_or(bytes.len(), |end| i + end)))
        } else if bytes[i..].starts_with(b"/*") {
            Some((Kind::Comment, block_comment_end(text, i)))
        } else if bytes[i] == b'"' {
            Some((Kind::Literal, string_end(bytes, i + 1)))
        } else if bytes[i] == b'r' {
            raw_string_end(text, i).map(|end| (Kind::Literal, end))
        } else if bytes[i] == b'\'' {
            char_end(text, i).map(|end| (Kind::Literal, end))
        } else {
            None
        };
        match token {
            Some((kind, end)) => {
                if i > start {
                    out.push((Kind::Code, &text[start..i]));
                }
                out.push((kind, &text[i..end]));
                start = end;
                i = end;
            },
            None => i += 1,
        }
    }
    if start < bytes.len() {
        out.push((Kind::Code, &text[start..]));
    }
    out
}

fn is_test_attribute(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("#[cfg(test)]") || line.starts_with("#[cfg(all(test,")
}

/// Byte offset just past the item that starts at `start` (its attributes
/// included): its `;`, or the brace that closes its body.
fn item_end(code: &str, start: usize) -> usize {
    let (mut braces, mut groups, mut at) = (0i32, 0i32, start);
    for (kind, part) in segments(&code[start..]) {
        if kind == Kind::Code {
            for (offset, c) in part.char_indices() {
                match c {
                    '(' | '[' => groups += 1,
                    ')' | ']' => groups -= 1,
                    '{' => braces += 1,
                    '}' => {
                        braces -= 1;
                        if braces == 0 && groups == 0 {
                            let end = at + offset + 1;
                            let rest = &code[end..];
                            let trimmed = rest.trim_start();
                            return if trimmed.starts_with(';') {
                                end + rest.len() - trimmed.len() + 1
                            } else {
                                end
                            };
                        }
                    },
                    ';' if braces == 0 && groups == 0 => return at + offset + 1,
                    _ => {},
                }
            }
        }
        at += part.len();
    }
    code.len()
}

/// Production code: comments and `#[cfg(test)]` items removed.
fn production(text: &str) -> String {
    let code: String = segments(text)
        .into_iter()
        .map(|(kind, part)| match kind {
            Kind::Comment => "\n".repeat(part.matches('\n').count()),
            _ => part.to_string(),
        })
        .collect();
    let mut out = String::new();
    let (mut cursor, mut offset) = (0, 0);
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if start >= cursor && is_test_attribute(line) {
            out.push_str(&code[cursor..start]);
            cursor = item_end(&code, start);
        }
    }
    out.push_str(&code[cursor.min(code.len())..]);
    out
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn module_declaration(line: &str) -> Option<&str> {
    let mut decl = line.trim();
    for visibility in ["pub(crate) ", "pub(super) ", "pub "] {
        decl = decl.strip_prefix(visibility).unwrap_or(decl);
    }
    let name = decl.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    name.bytes().all(is_ident).then_some(name)
}

struct Source {
    file: String,
    text: String,
}

/// Every production file, reached from the crate roots through the `mod`
/// declarations and `include!`s that survive [`production`].
fn production_sources() -> Vec<Source> {
    let root = src();
    let mut queue = vec![
        (root.join("lib.rs"), root.clone()),
        (root.join("main.rs"), root.clone()),
    ];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some((file, module_dir)) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let raw = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let text = production(&raw);
        let here = file.parent().unwrap().to_path_buf();
        let mut path_attribute = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(path) =
                trimmed.strip_prefix("#[path = \"").and_then(|r| r.strip_suffix("\"]"))
            {
                path_attribute = Some(path.to_string());
                continue;
            }
            if trimmed.starts_with("#[") {
                continue;
            }
            if let Some(name) = module_declaration(trimmed) {
                let target = match path_attribute.take() {
                    Some(path) => here.join(path),
                    None => {
                        let flat = module_dir.join(format!("{name}.rs"));
                        if flat.exists() {
                            flat
                        } else {
                            module_dir.join(name).join("mod.rs")
                        }
                    },
                };
                assert!(
                    target.exists(),
                    "{} declares module `{name}` at {}, which does not exist",
                    file.display(),
                    target.display()
                );
                let child_dir = if target.ends_with("mod.rs") {
                    target.parent().unwrap().to_path_buf()
                } else {
                    target.with_extension("")
                };
                queue.push((target, child_dir));
            }
            path_attribute = None;
            for (at, _) in trimmed.match_indices("include!(\"") {
                let rest = &trimmed[at + "include!(\"".len()..];
                let included = here.join(&rest[..rest.find('"').unwrap()]);
                assert!(included.exists(), "{} includes {}", file.display(), included.display());
                queue.push((included, module_dir.clone()));
            }
        }
        out.push(Source {
            file: file.strip_prefix(&root).unwrap().to_string_lossy().to_string(),
            text,
        });
    }
    out
}

/// The function whose body contains byte `at` of production `text`.
fn enclosing_fn(text: &str, at: usize) -> String {
    let mut best = String::from("<none>");
    for line in text[..at].lines() {
        let trimmed = line.trim_start();
        let Some(pos) = trimmed.find("fn ") else {
            continue;
        };
        if trimmed[..pos].split_whitespace().all(|word| {
            [
                "pub",
                "pub(crate)",
                "pub(super)",
                "async",
                "const",
                "unsafe",
            ]
            .contains(&word)
        }) {
            let name: String = trimmed[pos + 3..]
                .bytes()
                .take_while(|b| is_ident(*b))
                .map(char::from)
                .collect();
            if !name.is_empty() {
                best = name;
            }
        }
    }
    best
}

/// The production body of the function whose signature starts with `signature`.
fn function(text: &str, signature: &str) -> String {
    let start = text.find(signature).unwrap_or_else(|| panic!("`{signature}` not found"));
    text[start..item_end(text, start)].to_string()
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn source(file: &str) -> String {
    production_sources()
        .into_iter()
        .find(|s| s.file == file)
        .unwrap_or_else(|| panic!("{file} is not production code"))
        .text
}

#[test]
fn the_guard_reads_production_code_only() {
    let text = production(
        "fn kept() { let s = \"} // {\"; let c = '{'; }\n\
         // PgPool::connect in a comment\n\
         /* PgPool::connect /* nested */ */\n\
         #[cfg(test)]\n\
         mod tests {\n    fn gone() { let s = r#\"}\"#; }\n}\n\
         #[cfg(all(test, feature = \"postgres\"))]\n\
         use gone::Thing;\n\
         fn also_kept<'a>(x: &'a str) {}\n",
    );
    assert!(text.contains("fn kept()") && text.contains("fn also_kept"), "{text}");
    assert!(!text.contains("PgPool") && !text.contains("gone"), "{text}");
    let files: BTreeSet<String> = production_sources().into_iter().map(|s| s.file).collect();
    for file in [
        "main.rs",
        "lib.rs",
        "db.rs",
        "db_session.rs",
        "daemon.rs",
        "api.rs",
    ] {
        assert!(files.contains(file), "{file} missing from {files:?}");
    }
    let test_only = files.iter().filter(|f| f.contains("test") || f.ends_with("worker_venue.rs"));
    assert_eq!(test_only.count(), 0, "test-only files read as production: {files:?}");
}

#[test]
fn every_daemon_pool_and_connection_is_built_where_the_bounds_reach_it() {
    // `connect_daemon_pool` delegates to `connect_daemon_pool_with`, as
    // `BudgetBasisAlarm::new` delegates to `with_metrics`.
    let allowed: BTreeSet<(&str, &str)> = [
        ("db_session.rs", "connect_daemon_pool_with"),
        ("order_journal_store.rs", "new"),
        ("budget_basis_alarm.rs", "with_metrics"),
        ("db.rs", "run_db_command"),
    ]
    .into_iter()
    .collect();
    let constructors = [
        "PgPool::connect",
        "Pool::<Postgres>::connect",
        "PgPoolOptions::new()",
        "PgConnection::connect",
        "PgListener::connect",
        ".connect_with(",
        ".connect_lazy(",
        ".connect_lazy_with(",
        ".connect()",
    ];
    let mut found = Vec::new();
    for source in production_sources() {
        for needle in constructors {
            for (at, _) in source.text.match_indices(needle) {
                found.push((source.file.clone(), enclosing_fn(&source.text, at), needle));
            }
        }
    }
    let stray: Vec<_> = found
        .iter()
        .filter(|(file, function, _)| !allowed.contains(&(file.as_str(), function.as_str())))
        .collect();
    assert!(
        stray.is_empty(),
        "pool or connection built outside the bounded owners: {stray:?}"
    );
    let owners: BTreeSet<(&str, &str)> = found
        .iter()
        .map(|(file, function, _)| (file.as_str(), function.as_str()))
        .collect();
    assert_eq!(owners, allowed, "every allowed owner still builds its pool");
}

const GUCS: [&str; 3] = [
    "lock_timeout",
    "statement_timeout",
    "idle_in_transaction_session_timeout",
];

/// Each change of a bound in `text`: a `set_config` (transaction-local or
/// session), a `SET`, a `-c` Startup option or a Startup options key.
fn bound_changes(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut changes = Vec::new();
    for guc in GUCS {
        for (at, _) in text.match_indices(guc) {
            let end = at + guc.len();
            if (at > 0 && is_ident(bytes[at - 1])) || bytes.get(end).is_some_and(|b| is_ident(*b)) {
                continue;
            }
            let mut window = at.saturating_sub(40);
            while !text.is_char_boundary(window) {
                window -= 1;
            }
            let before = text[window..at].trim_end();
            let upper = before.to_ascii_uppercase();
            let how = if before.to_ascii_lowercase().ends_with("set_config('") {
                let args = &text[end..end + text[end..].find(')').unwrap()];
                if args.trim_end().ends_with("true") {
                    "set_config local"
                } else {
                    "set_config session"
                }
            } else if upper.ends_with("SET")
                || upper.ends_with("SET LOCAL")
                || upper.ends_with("SET SESSION")
            {
                "SET"
            } else if before.ends_with("-c") || before.ends_with("-c%20") {
                "-c option"
            } else if before.ends_with("(\"") {
                "options key"
            } else {
                continue;
            };
            changes.push(format!("{how} {guc}"));
        }
    }
    changes.sort();
    changes
}

#[test]
fn only_the_known_sites_change_the_session_bounds() {
    let local = |file: &str| {
        (
            file.to_string(),
            GUCS.iter().map(|guc| format!("set_config local {guc}")).collect::<Vec<_>>(),
        )
    };
    let mut expected: BTreeMap<String, Vec<String>> = [
        local("order_journal_balance.rs"),
        local("order_journal_readiness.rs"),
        local("venue_ledger_store.rs"),
        ("budget_basis_alarm.rs".to_string(), vec!["SET statement_timeout".to_string()]),
        (
            "db_session.rs".to_string(),
            GUCS.iter().map(|guc| format!("options key {guc}")).collect(),
        ),
    ]
    .into_iter()
    .collect();
    for changes in expected.values_mut() {
        changes.sort();
    }
    let observed: BTreeMap<String, Vec<String>> = production_sources()
        .into_iter()
        .map(|s| (s.file, bound_changes(&s.text)))
        .filter(|(_, changes)| !changes.is_empty())
        .collect();
    assert_eq!(observed, expected, "session bounds changed outside the known sites");
}

#[test]
fn main_wires_the_account_kind_and_checks_the_bounds_before_schema_readiness() {
    let main = source("main.rs");
    assert!(
        squeeze(&main)
            .contains("connect_database(database_url,parts.venue.capabilities().account_kind)"),
        "main must pass the venue's account kind"
    );
    let connect = squeeze(&function(&main, "async fn connect_database("));
    assert!(
        connect.contains(
            "connect_database_with(database_url,account_kind,SessionBounds::BALANCE,SessionBounds::BALANCE,)"
        ),
        "production applies and expects the same bounds: {connect}"
    );
    let body = squeeze(&function(&main, "async fn connect_database_with("));
    assert!(
        body.contains("db_session::connect_daemon_pool_with(database_url,account_kind,applied)"),
        "the retry loop builds the pool through db_session"
    );
    let check = body
        .find(
            "ifaccount_kind==AccountKind::BalanceAccount{ifletErr(error)=db_session::verify_session_bounds(&pool,expected).await{",
        )
        .expect("a balance account runs the self-check");
    let readiness = body
        .find("db::verify_adr0052_schema_readiness(&pool)")
        .expect("schema readiness still runs");
    assert!(check < readiness, "the self-check runs before schema readiness");
}

#[test]
fn a_position_account_pool_is_exactly_the_original_connect() {
    let body = function(&source("db_session.rs"), "pub async fn connect_daemon_pool_with(");
    assert!(
        body.contains("AccountKind::PositionAccount => sqlx::PgPool::connect(database_url).await,"),
        "the position-account branch must stay sqlx::PgPool::connect(database_url): {body}"
    );
    assert_eq!(
        body.matches("PositionAccount").count(),
        1,
        "one position-account branch: {body}"
    );
}
