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
    let mut expected: BTreeMap<String, Vec<String>> = [
        // The three Startup options, the self-check's read and its message.
        ("db_session.rs", [each("name"), each("read"), each("other")].concat()),
        // The existing transaction-local bounds.
        ("order_journal_balance.rs", each("set_config local")),
        ("order_journal_readiness.rs", each("set_config local")),
        ("venue_ledger_store.rs", each("set_config local")),
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
