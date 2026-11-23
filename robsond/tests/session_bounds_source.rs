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

/// Whether the attribute that starts `rest` gates its item to tests. The
/// whole attribute is read, whitespace removed, as rustfmt may split it
/// over lines.
fn is_test_attribute(rest: &str) -> bool {
    let rest = rest.trim_start();
    if !rest.starts_with("#[cfg(") {
        return false;
    }
    let attribute = squeeze(&rest[..rest.find(']').map_or(rest.len(), |end| end + 1)]);
    attribute.starts_with("#[cfg(test)]") || attribute.starts_with("#[cfg(all(test,")
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
        if start >= cursor && is_test_attribute(&code[start..]) {
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
            // A text that production code embeds with `include_str!` (its SQL)
            // is one string literal of that code: its bound mentions count.
            for (at, _) in trimmed.match_indices("include_str!(\"") {
                let rest = &trimmed[at + "include_str!(\"".len()..];
                let included = here.join(&rest[..rest.find('"').unwrap()]);
                assert!(included.exists(), "{} embeds {}", file.display(), included.display());
                let body = fs::read_to_string(&included)
                    .unwrap_or_else(|e| panic!("{}: {e}", included.display()));
                assert!(!body.contains("\"####"), "{} closes the literal", included.display());
                if seen.insert(included.clone()) {
                    out.push(Source {
                        file: included.strip_prefix(&root).unwrap().to_string_lossy().to_string(),
                        text: format!("r####\"{body}\"####"),
                    });
                }
            }
        }
        out.push(Source {
            file: file.strip_prefix(&root).unwrap().to_string_lossy().to_string(),
            text,
        });
    }
    out
}

/// A `fn` or `impl` item of production code, with its byte span.
#[derive(Debug)]
struct Item {
    is_impl: bool,
    name: String,
    span: std::ops::Range<usize>,
}

/// Whether the keyword at `start` begins an item: only item qualifiers stand
/// between it and the end of the previous item, a block opening or an
/// attribute. `fn` and `impl` in type position (`fn(u8)`, `-> impl Fn`) do
/// not.
fn starts_item(code: &str, start: usize) -> bool {
    let mut before = code[..start].trim_end();
    loop {
        if before.ends_with(')') {
            if let Some(visibility) = before.rfind("pub(") {
                if before[visibility..].find(')') == Some(before.len() - visibility - 1) {
                    before = before[..visibility].trim_end();
                    continue;
                }
            }
        }
        let word_start = before
            .rfind(|c: char| c.is_whitespace() || matches!(c, ';' | '{' | '}' | ']'))
            .map_or(0, |i| i + 1);
        if ["pub", "async", "const", "unsafe", "default", "extern"].contains(&&before[word_start..])
        {
            before = before[..word_start].trim_end();
            continue;
        }
        break;
    }
    before.is_empty() || before.ends_with([';', '{', '}', ']'])
}

fn leading_ident(text: &str) -> Option<String> {
    let name: String =
        text.trim_start().bytes().take_while(|b| is_ident(*b)).map(char::from).collect();
    (!name.is_empty()).then_some(name)
}

/// The self type an `impl` header names: `impl<T> Trait for Type<T>` gives
/// `Type`.
fn impl_type(header: &str) -> Option<String> {
    let mut header = header.trim_start();
    if header.starts_with('<') {
        let mut depth = 0;
        let close = header.char_indices().find_map(|(i, c)| {
            depth += match c {
                '<' => 1,
                '>' => -1,
                _ => 0,
            };
            (depth == 0).then_some(i)
        })?;
        header = &header[close + 1..];
    }
    let header = header.split(['{', ';']).next()?;
    let ty = header.rsplit(" for ").next()?.trim().trim_start_matches('&');
    let path = ty.split(['<', ' ', '\n']).next()?;
    leading_ident(path.rsplit("::").next()?)
}

/// The `fn` and `impl` items of production `code`.
fn items(code: &str) -> Vec<Item> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    for (kind, part) in segments(code) {
        if kind == Kind::Code {
            for keyword in ["fn", "impl"] {
                for (offset, _) in part.match_indices(keyword) {
                    let start = at + offset;
                    let end = start + keyword.len();
                    if (start > 0 && is_ident(bytes[start - 1]))
                        || bytes.get(end).is_some_and(|b| is_ident(*b))
                        || !starts_item(code, start)
                    {
                        continue;
                    }
                    let name = if keyword == "fn" {
                        leading_ident(&code[end..])
                    } else {
                        impl_type(&code[end..])
                    };
                    if let Some(name) = name {
                        out.push(Item {
                            is_impl: keyword == "impl",
                            name,
                            span: start..item_end(code, start),
                        });
                    }
                }
            }
        }
        at += part.len();
    }
    out
}

/// The innermost `impl` (empty outside one) and `fn` (`<module>` outside one)
/// whose span contains byte `at`.
fn owner(items: &[Item], at: usize) -> (String, String) {
    let innermost = |is_impl: bool| {
        items
            .iter()
            .filter(|item| item.is_impl == is_impl && item.span.contains(&at))
            .min_by_key(|item| item.span.len())
            .map(|item| item.name.clone())
    };
    (
        innermost(true).unwrap_or_default(),
        innermost(false).unwrap_or_else(|| "<module>".into()),
    )
}

const CONSTRUCTORS: [&str; 9] = [
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

/// Every pool or connection constructor in production `code`, with its owner.
fn constructors(code: &str) -> Vec<(String, String, &'static str)> {
    if !CONSTRUCTORS.iter().any(|needle| code.contains(needle)) {
        return Vec::new();
    }
    let items = items(code);
    let mut found = Vec::new();
    for needle in CONSTRUCTORS {
        for (at, _) in code.match_indices(needle) {
            let (ty, function) = owner(&items, at);
            found.push((ty, function, needle));
        }
    }
    found
}

const GUCS: [&str; 3] = [
    "lock_timeout",
    "statement_timeout",
    "idle_in_transaction_session_timeout",
];

/// The text in lower case, without whitespace, quotes or backslashes.
fn folded(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '\\'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Every mention of a bound's name in a string literal of production `code`,
/// with what the text before it does with it: a read, a change (and which
/// kind), the bare name (an options key or a constant), or anything else.
fn bound_mentions(code: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for (kind, literal) in segments(code) {
        if kind != Kind::Literal {
            continue;
        }
        // The literal's text, between its quotes (`"…"`, `b"…"`, `r#"…"#`).
        let open = literal.find('"').map_or(1, |i| i + 1);
        let literal = &literal[open..literal.rfind('"').unwrap_or(literal.len()).max(open)];
        // PostgreSQL folds unquoted names to lower case: `SET STATEMENT_TIMEOUT`
        // is the same setting. ASCII folding keeps every byte offset.
        let literal = literal.to_ascii_lowercase();
        let literal = literal.as_str();
        let bytes = literal.as_bytes();
        for guc in GUCS {
            for (at, _) in literal.match_indices(guc) {
                let end = at + guc.len();
                if (at > 0 && is_ident(bytes[at - 1]))
                    || bytes.get(end).is_some_and(|b| is_ident(*b))
                {
                    continue;
                }
                let before = folded(&literal[..at]);
                let after = folded(&literal[end..]);
                let how = if before.ends_with("current_setting(") || before.ends_with("show") {
                    "read"
                } else if before.ends_with("set_config(") {
                    match after.split(')').next().and_then(|args| args.rsplit(',').next()) {
                        Some("true") => "set_config local",
                        Some("false") => "set_config session",
                        _ => "set_config unknown",
                    }
                } else if before.ends_with("reset") {
                    "RESET"
                } else if before.ends_with("setlocal") {
                    "SET LOCAL"
                } else if before.ends_with("setsession") {
                    "SET SESSION"
                } else if before.ends_with("set") {
                    "SET"
                } else if before.ends_with("-c") || before.ends_with("-c%20") {
                    "-c option"
                } else if before.is_empty() && after.is_empty() {
                    "name"
                } else {
                    "other"
                };
                mentions.push(format!("{how} {guc}"));
            }
        }
    }
    mentions.sort();
    mentions
}

#[test]
fn the_guard_sees_what_it_guards() {
    let code = production(
        "impl Other {\n    pub fn new() -> Self { Self }\n}\n\
         impl<T: Clone> JournalBackend for PgJournalBackend<T> {\n\
         \x20   fn helper() {}\n\
         \x20   pub(crate) async fn new(pool: &PgPool) -> Self {\n\
         \x20       let p = PgPoolOptions::new();\n        Self\n    }\n\
         \x20   fn after() { let q = PgPoolOptions::new(); }\n}\n\
         fn takes(f: fn(u8) -> u8, g: impl Fn()) -> impl Fn() { PgPoolOptions::new(); }\n\
         static POOL: () = { PgPoolOptions::new(); };\n",
    );
    let owners: Vec<_> = constructors(&code)
        .into_iter()
        .map(|(ty, function, _)| format!("{ty}::{function}"))
        .collect();
    assert_eq!(owners, [
        "PgJournalBackend::new",
        "PgJournalBackend::after",
        "::takes",
        "::<module>"
    ]);
    let mentions = bound_mentions(&production(
        "q(\"SELECT set_config( 'statement_timeout', '0', false)\");\n\
         q(\"SELECT set_config('lock_timeout', $1, true)\");\n\
         q(\"RESET lock_timeout\"); q(\"SET  LOCAL lock_timeout = '1s'\");\n\
         q(\"set Session statement_timeout = 0\"); q(\"set idle_in_transaction_session_timeout = 0\");\n\
         o([(\"lock_timeout\", \"0\")]); o(\"-c statement_timeout=0\");\n\
         q(\"SELECT current_setting('lock_timeout')\"); q(\"SHOW lock_timeout\");\n\
         m(\"expected lock_timeout={}\"); let lock_timeout_ms = 1; // statement_timeout\n\
         q(\"SET STATEMENT_TIMEOUT = 0\"); q(\"Reset Lock_Timeout\");\n",
    ));
    assert_eq!(mentions, [
        "-c option statement_timeout",
        "RESET lock_timeout",
        "RESET lock_timeout",
        "SET LOCAL lock_timeout",
        "SET SESSION statement_timeout",
        "SET idle_in_transaction_session_timeout",
        "SET statement_timeout",
        "name lock_timeout",
        "other lock_timeout",
        "read lock_timeout",
        "read lock_timeout",
        "set_config local lock_timeout",
        "set_config session statement_timeout",
    ]);
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
         #[cfg(all(\n    test,\n    target_os = \"linux\"\n))]\n\
         mod gone_too;\n\
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
    let allowed: BTreeSet<(&str, &str, &str)> = [
        ("db_session.rs", "", "connect_daemon_pool_with"),
        ("order_journal_store.rs", "PgJournalBackend", "new"),
        ("budget_basis_alarm.rs", "BudgetBasisAlarm", "with_metrics"),
        ("db.rs", "", "run_db_command"),
        // Package 2's dormant Spot owner opens raw connections from the
        // daemon pool's connect options, so a balance daemon's bounds are
        // inherited, and appends its own stricter reviewed limits:
        // the clock store (300 ms lock, 1 s statement, 1 s idle at startup),
        ("venue_clock_store.rs", "ClockSampleStore", "initialize"),
        // the budget bootstrap (1 s statement and idle at startup, 300 ms
        // lock as its first statement, sql/settings.sql),
        ("venue_budget/bootstrap.rs", "", "observe_bootstrap"),
        // and the pause restoration (300 ms lock, 1 s statement, 1 s idle at
        // startup).
        ("venue_budget/restore.rs", "", "existing_pause"),
        // Package 3's dormant Spot owners (no production caller) do the
        // same, so the bounds of the pool they are given are inherited:
        // the N5 claim store (1 s statement and idle at startup, then
        // sql/settings.sql as its first statement: 300 ms lock),
        ("venue_budget/n5_claim/mod.rs", "", "raw_connection"),
        // the cold source-audit page reader (read-only, 1 s statement and
        // idle at startup; its lock bound is the inherited one),
        ("venue_budget/source_audit.rs", "", "observe"),
        // and the cold source-audit run (1 s statement and idle at startup,
        // then sql/settings.sql: 300 ms lock).
        ("venue_budget/source_audit/run.rs", "", "begin"),
        // 8k-2's dormant pre reader (no production caller, checked by
        // package5a_owners_stay_dormant_or_bounded): one read-only
        // autocommit statement on one fresh connection from the connect
        // options its caller passes, under the caller's deadline, closed
        // before it returns; its caller (8k-1's transport) passes the daemon
        // pool's, as package 2's and 3's owners take theirs. Since 8k-3a it
        // is `fetch`, the one owner of 8k-2's pre statement and of the
        // non-trading scope's pre and post statement (PLAN-r2 §6.1).
        ("balance_reconciliation/snapshot.rs", "", "fetch"),
    ]
    .into_iter()
    .collect();
    let mut found = Vec::new();
    for source in production_sources() {
        for (ty, function, needle) in constructors(&source.text) {
            found.push((source.file.clone(), ty, function, needle));
        }
    }
    let stray: Vec<_> = found
        .iter()
        .filter(|(file, ty, function, _)| {
            !allowed.contains(&(file.as_str(), ty.as_str(), function.as_str()))
        })
        .collect();
    assert!(
        stray.is_empty(),
        "pool or connection built outside the bounded owners: {stray:?}"
    );
    let owners: BTreeSet<(&str, &str, &str)> = found
        .iter()
        .map(|(file, ty, function, _)| (file.as_str(), ty.as_str(), function.as_str()))
        .collect();
    assert_eq!(owners, allowed, "every allowed owner still builds its pool");
}

#[test]
fn every_mention_of_a_session_bound_is_a_known_one() {
    let each = |how: &str| GUCS.iter().map(|guc| format!("{how} {guc}")).collect::<Vec<_>>();
    // Package 3 embeds package 2's sql/settings.sql twice more.
    let settings_sql = || {
        [each("set_config local"), vec![
            "other lock_timeout".to_string(),
            "other statement_timeout".to_string(),
        ]]
        .concat()
    };
    let mut expected: BTreeMap<String, Vec<String>> = [
        // The three Startup options, the self-check's read and its message.
        ("db_session.rs", [each("name"), each("read"), each("other")].concat()),
        // The existing transaction-local bounds.
        ("order_journal_balance.rs", each("set_config local")),
        ("order_journal_readiness.rs", each("set_config local")),
        ("venue_ledger_store.rs", each("set_config local")),
        // ADR-0056 step 8l-4: the ledger's bounds, first in the balance
        // suspension clear's transaction.
        ("balance_accounting.rs", each("set_config local")),
        // The alarm's own session statement bound.
        ("budget_basis_alarm.rs", vec!["SET statement_timeout".to_string()]),
        // Package 2's clock store: its three Startup options, and the
        // session statement that confirms them on each new connection.
        ("venue_clock_store.rs", [each("name"), each("set_config session")].concat()),
        // Package 2's budget bootstrap: its two Startup options, and the
        // names it checks in the settings row its first statement returns.
        ("venue_budget/bootstrap.rs", vec![
            "name idle_in_transaction_session_timeout".to_string(),
            "name lock_timeout".to_string(),
            "name statement_timeout".to_string(),
            "name statement_timeout".to_string(),
        ]),
        // That first statement: transaction-local, like the journal's and
        // the ledger's; two of its result columns are named after a bound.
        (
            "venue_budget/sql/settings.sql",
            [each("set_config local"), vec![
                "other lock_timeout".to_string(),
                "other statement_timeout".to_string(),
            ]]
            .concat(),
        ),
        // Package 2's pause restoration: its three Startup options.
        ("venue_budget/restore.rs", each("name")),
        // Package 3's cold snapshot: transaction-local limits (20 ms lock,
        // 40 ms statement, 1 s idle) set after BEGIN on a connection of the
        // pool it is given, so the inherited bounds cover BEGIN itself.
        ("spot_order_set_store/cold_audit.rs", each("set_config local")),
        // Package 3's N5 claim store: its two Startup options, and the
        // names it checks in the settings row its first statement returns.
        ("venue_budget/n5_claim/mod.rs", vec![
            "name idle_in_transaction_session_timeout".to_string(),
            "name lock_timeout".to_string(),
            "name statement_timeout".to_string(),
            "name statement_timeout".to_string(),
        ]),
        // Package 3's cold source-audit page reader: its two Startup options.
        ("venue_budget/source_audit.rs", vec![
            "name idle_in_transaction_session_timeout".to_string(),
            "name statement_timeout".to_string(),
        ]),
        // Package 3's cold source-audit run: like the N5 claim store.
        ("venue_budget/source_audit/run.rs", vec![
            "name idle_in_transaction_session_timeout".to_string(),
            "name lock_timeout".to_string(),
            "name statement_timeout".to_string(),
            "name statement_timeout".to_string(),
        ]),
        // The same sql/settings.sql, embedded by the N5 claim store and by
        // the source-audit run: the guard names each by the path it is
        // reached through.
        ("venue_budget/n5_claim/../sql/settings.sql", settings_sql()),
        ("venue_budget/source_audit/../sql/settings.sql", settings_sql()),
    ]
    .into_iter()
    .map(|(file, mentions)| (file.to_string(), mentions))
    .collect();
    for mentions in expected.values_mut() {
        mentions.sort();
    }
    let observed: BTreeMap<String, Vec<String>> = production_sources()
        .into_iter()
        .map(|s| (s.file, bound_mentions(&s.text)))
        .filter(|(_, mentions)| !mentions.is_empty())
        .collect();
    assert_eq!(observed, expected, "a session bound is read or changed outside the known sites");
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

/// Package 3's dormant connection owners build from the connect options of
/// the pool they are given, so a balance daemon's bounds are inherited, and
/// append stricter Startup options. One of them, the N5 cold read, lives in
/// robson-store, outside the robsond sources the other checks read; robsond's
/// N5 claim store passes it its pool. Its only connection and bound names are
/// checked here.
#[test]
fn package3_connection_owners_inherit_the_pool_bounds() {
    let stricter = r#".clone().options([("statement_timeout","1s"),("idle_in_transaction_session_timeout","1s"),])"#;
    let n5 =
        squeeze(&function(&source("venue_budget/n5_claim/mod.rs"), "async fn raw_connection("));
    assert!(n5.contains(&format!("letoptions=(*pool.connect_options()){stricter};")), "{n5}");
    let run = source("venue_budget/source_audit/run.rs");
    let options = squeeze(&function(&run, "fn options(pool: &PgPool)"));
    assert!(options.contains(&format!("(*pool.connect_options()){stricter}")), "{options}");
    assert!(squeeze(&function(&run, "async fn begin(")).contains("options(pool).connect()"));
    let observe = squeeze(&function(&source("venue_budget/source_audit.rs"), "async fn observe("));
    assert!(
        observe.contains(
            r#"letoptions=(*pool.connect_options()).clone().options([("default_transaction_read_only","on"),("statement_timeout","1s"),("idle_in_transaction_session_timeout","1s"),]);"#
        ),
        "{observe}"
    );

    let store = Path::new(env!("CARGO_MANIFEST_DIR")).join("../robson-store/src/native_n5.rs");
    let code = production(&fs::read_to_string(store).unwrap());
    let built: Vec<String> = constructors(&code)
        .into_iter()
        .map(|(ty, function, needle)| format!("{ty}::{function} {needle}"))
        .collect();
    assert_eq!(built, ["::read .connect()"]);
    let read = squeeze(&function(&code, "pub async fn read("));
    assert!(
        read.contains(&format!("letoptions=(*pool.connect_options()){stricter};")),
        "{read}"
    );
    assert_eq!(bound_mentions(&code), [
        "name idle_in_transaction_session_timeout",
        "name statement_timeout"
    ]);
}

/// Package 3's cold installer and the cold owner's capture, with the rows it
/// retains, run only in tests: neither is production code. Production keeps
/// the owner's anchor read, which the cold snapshot uses.
#[test]
fn package3_test_only_owners_stay_out_of_production() {
    let files: BTreeSet<String> = production_sources().into_iter().map(|s| s.file).collect();
    assert!(
        !files.iter().any(|file| file.starts_with("spot_order_set_store/cold_install")),
        "the cold installer is production code: {files:?}"
    );
    let owner = source("spot_order_set_store/cold_owner.rs");
    assert!(owner.contains("pub(super) async fn read_anchor("), "{owner}");
    for test_only in [
        "fn capture(",
        "ColdOwnerCut",
        "CapturedRows",
        "impl ColdSourceSink",
    ] {
        assert!(!owner.contains(test_only), "{test_only} is production code");
    }
}

/// Package 4's supervised startup is the only producer of the Spot writer a
/// daemon can serve on, and it runs only in tests, like the cold installer
/// and the cold owner's capture it drives. Production compiles the handoff
/// type and the daemon's private assembly that would consume one, but no
/// code that constructs a handoff or builds a daemon from it, and neither
/// the supervised preflight (with its own loopback pool) nor its command.
#[test]
fn package4_supervised_startup_stays_out_of_production() {
    let sources = production_sources();
    let files: BTreeSet<&str> = sources.iter().map(|s| s.file.as_str()).collect();
    assert!(
        !files.contains("spot_supervised_preflight.rs"),
        "the supervised preflight is production code: {files:?}"
    );
    let startup = source("spot_order_set_store/spot_startup.rs");
    assert!(startup.contains("pub(crate) struct SpotReady {"), "{startup}");
    for test_only in [
        "trait SpotSupervisor",
        "fn record_epoch",
        "fn capture",
        "fn install",
        "fn audit_source_for_preflight",
        "fn admit",
        "fn authenticate",
        "INSTALLER",
    ] {
        assert!(!startup.contains(test_only), "{test_only} is production code");
    }
    // Besides its definition and its accessors' impl, no production
    // `SpotReady {` exists: nothing builds one.
    let built: Vec<(&str, usize)> = sources
        .iter()
        .map(|s| {
            let named = s.text.matches("SpotReady {").count();
            let declared = s.text.matches("struct SpotReady {").count()
                + s.text.matches("impl SpotReady {").count();
            (s.file.as_str(), named - declared)
        })
        .filter(|(_, count)| *count > 0)
        .collect();
    assert!(built.is_empty(), "a production SpotReady is built: {built:?}");
    assert_eq!(startup.matches("impl SpotReady {").count(), 1, "{startup}");
    assert!(!source("daemon.rs").contains("fn with_supervised_spot("));
    let main = source("main.rs");
    assert!(
        !main.contains("spot_supervised_preflight") && !main.contains("spot-supervised-preflight"),
        "{main}"
    );
    assert!(!source("venue_budget/source_audit.rs").contains("fn describe_existing_source("));
    assert!(!source("venue_budget/source_audit/run.rs").contains("fn start_existing("));
}

/// Package 5a's owners on a balance daemon, one line each: what 8k-1 B,
/// 8k-2, 8l-1, 8l-2, 8l-4a, 8l-4b, 8p-1, 8p-2, 8p-4, 8m-1 and 8m-3 add to
/// its database paths, and that each dormant one keeps no production caller.
/// Its one connection owner is allowed above, with its reason; its one bound
/// site is known above.
#[test]
fn package5a_owners_stay_dormant_or_bounded() {
    let sources = production_sources();
    let naming = |needle: &str| -> BTreeSet<String> {
        sources
            .iter()
            .filter(|s| s.text.contains(needle))
            .map(|s| s.file.clone())
            .collect()
    };
    let outside = |needle: &str, owner: &str| -> BTreeSet<String> {
        naming(needle).into_iter().filter(|file| !file.starts_with(owner)).collect()
    };
    let none = BTreeSet::<String>::new();
    let only = |file: &str| BTreeSet::from([file.to_string()]);
    // 8k-1 B: only 8k-2's dormant detector records the venue_discrepancy
    // fence; the release endpoint's refusal reads the gate and sends no SQL.
    assert_eq!(outside("VenueDiscrepancy", "balance_reconciliation/"), none);
    // 8k-2: the detector is declared and nothing calls it; its pre reader is
    // the connection owner allowed above.
    // 8l-3b's dormant cash writer takes 8k-2's qualified verdict and its
    // interval (`BaseVerdict`, `ObservationInterval`) as its input.
    // 8l-5a's dormant re-anchor runs O1 and O2 through 8k-3a's capture.
    let detector_users = BTreeSet::from(
        [
            "lib.rs",
            "venue_ledger_store/cash/writer.rs",
            "venue_ledger_store/cash/reanchor.rs",
        ]
        .map(String::from),
    );
    assert_eq!(outside("balance_reconciliation", "balance_reconciliation/"), detector_users);
    assert_eq!(source("lib.rs").matches("mod balance_reconciliation;").count(), 1);
    assert_eq!(outside("read_pre", "balance_reconciliation/"), none);
    // 8l-1: S8-4's tables are migrations only; the production texts that
    // name any of them are 8l-2's read-only barrier statement, 8l-3a's
    // read-only cash statements, record_trades' one write, the
    // unattributed-fill invalidation (PLAN-r4 §6.10), 8l-3b's cash writer
    // (its context read and its checkpoint, discrepancy and sighting writes;
    // PLAN-r4 §6.4), which has no production caller, and 8p-4's read-only
    // leg-facts statement, which names dust_ledger only.
    let tables = [
        "cash_anchors",
        "cash_checkpoints",
        "cash_discrepancies",
        "cash_sightings",
        "cash_reanchors",
        "cash_attested_flows",
        "dust_ledger",
        "accounting_invalidations",
    ];
    // 8l-5b's metric robsond_cash_reanchors_total names a table only inside
    // its own name: metrics.rs holds no statement.
    let metrics = source("metrics.rs").replace("robsond_cash_reanchors_total", "");
    assert!(tables.iter().all(|table| !metrics.contains(table)));
    let named: BTreeSet<String> = tables
        .iter()
        .flat_map(|table| naming(table))
        .filter(|file| file != "metrics.rs")
        .collect();
    let cash_sql = [
        "venue_ledger_store/cash/expected.sql",
        "venue_ledger_store/cash/discrepancy.sql",
        "venue_ledger_store/cash/state.sql",
    ];
    let (barrier, trades) = ("venue_ledger_store/settlement.sql", "venue_ledger_store/trades.rs");
    let writer = [
        "venue_ledger_store/cash/context.sql",
        "venue_ledger_store/cash/writer.rs",
    ];
    // 8l-5b's re-anchor: its context read and its one write statement (the
    // anchor, the re-anchor row, its flows, the checkpoint and the
    // discrepancy's closure; PLAN-r4 §6.12), which has no production caller.
    let reanchor_sql = [
        "venue_ledger_store/cash/reanchor/context.sql",
        "venue_ledger_store/cash/reanchor/write.sql",
    ];
    // 8p-4's leg-facts statement, which has no production caller (below).
    let leg_facts_sql = "venue_ledger_store/leg_facts.sql";
    let expected: BTreeSet<String> = cash_sql
        .iter()
        .chain([&barrier, &trades, &leg_facts_sql])
        .chain(&writer)
        .chain(&reanchor_sql)
        .map(|file| file.to_string())
        .collect();
    assert_eq!(named, expected);
    for file in cash_sql.iter().chain([&barrier, &writer[0], &reanchor_sql[0], &leg_facts_sql]) {
        let text = source(file);
        assert!(["INSERT", "UPDATE", "DELETE"].iter().all(|w| !text.contains(w)), "{file}");
    }
    let leg_facts: Vec<&str> = tables
        .into_iter()
        .filter(|table| naming(table).contains(leg_facts_sql))
        .collect();
    assert_eq!(leg_facts, ["dust_ledger"]);
    assert_eq!(naming("INSERT INTO accounting_invalidations"), only(trades));
    // 8l-3a: E(t) and the cash predicates run on their caller's connection
    // and have no production caller yet (8l-3b, 8l-8, 8p, 8n).
    assert_eq!(outside("read_cash_state", "venue_ledger_store/cash"), none);
    assert_eq!(outside("CASH_STATE", "venue_ledger_store/cash"), none);
    // 8l-3b: the cash writer is declared; 8k-3c's dormant route is its one
    // caller (8k-3d's task calls the route).
    let route = only("balance_reconciliation/route.rs");
    assert_eq!(outside("CashWriter", "venue_ledger_store/cash"), route);
    assert_eq!(naming(".conclude("), route);
    assert_eq!(source("balance_reconciliation/route.rs").matches(".conclude(").count(), 1);
    // 8l-5a: the re-anchor service is built nowhere in production; its
    // endpoint takes it as a router extension, which the 5a composition adds
    // with PS-5b's ACCOUNT reads.
    let reanchor = "venue_ledger_store/cash/reanchor.rs";
    let commit = "venue_ledger_store/cash/reanchor/commit.rs";
    assert_eq!(outside("CashReanchor", "venue_ledger_store/cash"), only("api/cash_reanchor.rs"));
    let service: BTreeSet<String> = [reanchor, commit].map(String::from).into();
    assert_eq!(naming("CashReanchor {"), service);
    // 8p-1: S8-5's tables are migrations; 8p-2's boundary owner is the one
    // production text naming any of them. It inserts the pending snapshot
    // rows and reads the activation basis 8m will write, on the bounds of
    // balance_accounting.rs; the other three wait for 8p-5..9.
    assert_eq!(naming("boundary_snapshots"), only("boundary.rs"));
    assert_eq!(naming("activation_bases"), only("boundary.rs"));
    let later = [
        "boundary_reopenings",
        "boundary_snapshot_revisions",
        "boundary_carried_legs",
    ];
    assert_eq!(later.iter().flat_map(|table| naming(table)).collect::<BTreeSet<_>>(), none);
    let boundary = squeeze(&source("boundary.rs"));
    assert!(boundary.contains("sqlx::query(BOUNDS)"), "{boundary}");
    assert!(!boundary.contains("set_config"), "only the ledger's bounds: {boundary}");
    // 8m-1 and 8m-3: S8-6's runs are read only by the cutover report's
    // statement (no writer yet, 8m-4 and 8m-5), and the report is dormant:
    // only its module declaration names it (8m-4 and 8m-6 call it).
    assert_eq!(naming("venue_cutover"), only("cutover_report.sql"));
    assert_eq!(outside("cutover_report", "cutover_report"), only("lib.rs"));
    // 8l-2: the barrier runs on its caller's connection, and neither it nor
    // the gate's invalidity marks has a production caller yet (8l-3, 8l-7,
    // 8l-8, 8n).
    assert_eq!(outside("read_settlement", "venue_ledger_store/settlement.rs"), none);
    // 8l-3b's context read embeds the barrier (one statement, PLAN-r4 §6.4)
    // and its final qualification reads the marks; the writer is dormant.
    // 8k-3a's dormant capture reads them before pre and after post.
    // 8l-5b's re-anchor transaction embeds it too and reads the marks again
    // under the row (PLAN-r4 §6.12).
    let writer_rs = only("venue_ledger_store/cash/writer.rs");
    let barrier_users: BTreeSet<String> =
        writer_rs.iter().cloned().chain([commit.to_string()]).collect();
    assert_eq!(outside("SETTLEMENT_BARRIER", "venue_ledger_store/settlement.rs"), barrier_users);
    let capture = "balance_reconciliation/capture.rs".to_string();
    let marks = barrier_users.iter().cloned().chain([capture]).collect::<BTreeSet<_>>();
    assert_eq!(naming("invalidity_marks"), marks);
    assert_eq!(naming(".pending_evidence("), none);
    // 8l-4a: its writers run on the journal's and the clear's bounded
    // transactions (balance_accounting.rs, a known bound site above); the
    // generation cell is published in production only by the supervised
    // branch of the daemon's assembly, which needs a SpotReady production
    // never builds (package4_supervised_startup_stays_out_of_production).
    assert_eq!(naming("generation.publish("), only("daemon.rs"));
    let daemon = squeeze(&source("daemon.rs"));
    assert_eq!(daemon.matches("generation.publish(").count(), 1, "{daemon}");
    assert!(daemon.contains("Some(ready)=>{generation.publish(ready.generation().clone());"));
    // 8l-4b: every production projector apply passes the context's step (its
    // bounds first, on the projector's pool): the append path, the stream
    // repair and the projection worker, which takes the manager's context.
    assert_eq!(naming("apply_event_to_projections("), none);
    let steps: BTreeSet<String> = [
        "event_streams.rs",
        "position_manager.rs",
        "projection_worker.rs",
    ]
    .map(String::from)
    .into();
    assert_eq!(outside("projector_step(", "balance_accounting.rs"), steps);
    assert!(
        daemon.contains("letaccounting=self.position_manager.read().await.balance_accounting();")
    );
    // 8p-2's zero reset runs the same step through the projector's
    // owned-connection entry, on a connection it holds as a LedgerConnection,
    // so a late or failed apply retires it raw.
    assert_eq!(naming("apply_month_boundary_reset_on("), only("boundary.rs"));
    let zero_reset = squeeze(&function(&source("boundary.rs"), "async fn zero_reset("));
    assert!(
        zero_reset.contains(
            "letmutheld=LedgerConnection::new(pool.acquire().await?);letfetch=sqlx::query_as::<_,\
             EventEnvelope>(ENVELOPE).bind(event_id);letenvelope=fetch.fetch_one(held.connection())\
             .instrument(io(1)).await?;failed_seq=Some(envelope.seq);\
             apply_month_boundary_reset_on(held.connection(),&envelope,accounting).await?;\
             held.finished=true;"
        ),
        "{zero_reset}"
    );
    // 8p-2: the owner's two callers are the daemon's. Its balance-account
    // poll is routed before the legacy reset (R1), and its startup writes the
    // pending rows first. No production daemon is a balance account yet: the
    // factory refuses Spot and a Spot start needs a supervised handoff.
    assert_eq!(outside("boundary::", "boundary.rs"), only("daemon.rs"));
    assert!(daemon.contains(
        "==robson_exec::venue::AccountKind::BalanceAccount{returnself.boundary_tick(now).await;}"
    ));
    let start_up = squeeze(&function(&source("daemon.rs"), "async fn start_up("));
    assert!(
        start_up.contains(
            "->DaemonResult<()>{#[cfg(feature=\"postgres\")]ifletSome((pool,accounting))=self.\
             boundary_owner().await{letpending=crate::boundary::write_pending(&pool,&accounting,\
             Utc::now());crate::operation_observation::job(crate::operation_observation::Job::\
             BoundaryTick,pending,)"
        ),
        "{start_up}"
    );
    // 8p-4: the leg-facts reader runs its one statement (above) on the
    // connection its caller passes, so it opens none and sets no bound; nothing
    // outside it calls it or names its statement until 8p-5.
    let reader = "venue_ledger_store/leg_facts.rs";
    assert_eq!(outside("read_leg_facts", reader), none);
    assert_eq!(outside("LEG_FACTS", reader), none);
}
