//! The drift guard of the stub's mirror of the spot adapter (8d-1b, Codex
//! round 2): a SHA-256 of each adapter function `spot_evidence.rs` and
//! `balance_stub.rs` mirror, over its source text from its `fn` line to its
//! closing brace at the same indentation. A change to any of them, a
//! whitespace one included, fails here until the mirror is re-certified
//! against it and its hash updated. Source text only: no production code is
//! called or changed.

use sha2::{Digest, Sha256};

const TRADING: &str = include_str!("../../robson-connectors/src/binance/spot/trading.rs");
const EXCHANGE: &str = include_str!("../../robson-connectors/src/binance/spot/exchange.rs");

/// The adapter functions mirrored, and the hashes they were certified at.
const PINNED: [(&str, &str, &str); 11] = [
    (
        "trading.rs",
        "valid_client_order_id",
        "d2d5c51a4fc0629ccf8ba76b987cbb275c2946c004d561bdf5e65ee93237db0a",
    ),
    (
        "trading.rs",
        "valid_symbol",
        "c2cb781372738a297dc47f7b1edcc440a0a7cb1e0cbd1b74851a5f33840709fb",
    ),
    (
        "trading.rs",
        "malformed",
        "b863d01dfc6649e9b61d576f50175f7fc5164dfe9c1a58467a067cf8cc496ab8",
    ),
    (
        "trading.rs",
        "valid_fill",
        "996c6231a6a4e33740f21c9e5b635cbd5325f950049ee5f282765dfb76c984c6",
    ),
    (
        "trading.rs",
        "valid_trade",
        "966274b02908e8591673efd0c52ec76ee0df57c2c0cf4b903c540fa1dd01fba8",
    ),
    (
        "trading.rs",
        "fill_set",
        "bbe71a6b3cd043c8aa0af0330420f4777c23aba2f0069e82ad09ddacade73099",
    ),
    (
        "trading.rs",
        "quotes_agree",
        "f4d42424720b8f1e7621ddae67674d08d7425432fbd640a9c4d36971b3f90a77",
    ),
    (
        "trading.rs",
        "commission_total",
        "ae3342c79a98c03009dcaf1d33e96b1f447c79ac0ce65a6b6b9ec7aeefa92b57",
    ),
    (
        "trading.rs",
        "market_evidence",
        "b75cd6019242c8e3006db507034a86b78fb70fb614ca56190ef9a562657485bc",
    ),
    (
        "trading.rs",
        "terminal_result",
        "c370a8d4fb64084c8e39a50d7036084a9d3b5877df063d2d06909d70634fa9f4",
    ),
    // The composite paging loop.
    (
        "exchange.rs",
        "terminal_fills",
        "1dbb00a9177f4de8ca6e8ffc195c78c698998a233062b6a5cbcf76e5a685150b",
    ),
];

/// Whether `line` declares the function `name`, whatever its visibility.
fn declares(line: &str, name: &str) -> bool {
    let rest = line.trim_start();
    let rest = ["pub(super) ", "pub(crate) ", "pub "]
        .iter()
        .find_map(|visibility| rest.strip_prefix(visibility))
        .unwrap_or(rest);
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    rest.strip_prefix("fn ")
        .and_then(|rest| rest.strip_prefix(name))
        .is_some_and(|rest| rest.starts_with('(') || rest.starts_with('<'))
}

/// The function's source, from its `fn` line to the brace that closes it.
fn item(source: &str, name: &str) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| declares(lines[i], name)).collect();
    assert_eq!(starts.len(), 1, "{name} is declared {} times", starts.len());
    let start = starts[0];
    let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];
    let close = format!("{indent}}}");
    let end = (start..lines.len()).find(|&i| lines[i] == close).expect("a closing brace");
    lines[start..=end].join("\n")
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn the_mirrored_adapter_functions_are_as_certified() {
    let mut changed = Vec::new();
    for (file, name, pinned) in PINNED {
        let source = if file == "exchange.rs" {
            EXCHANGE
        } else {
            TRADING
        };
        let now = sha256_hex(&item(source, name));
        if now != pinned {
            changed.push(format!("{file} `{name}`: certified at {pinned}, now {now}"));
        }
    }
    assert!(
        changed.is_empty(),
        "the spot adapter changed where BalanceStub mirrors it: re-certify the mirror \
         (spot_evidence.rs, balance_stub.rs) against each function below, then update its \
         pinned hash in spot_mirror_tests.rs:\n{}",
        changed.join("\n")
    );
}

/// The guard's own parts: the extraction takes exactly one function, whole,
/// and the hash is SHA-256 (FIPS 180-2's "abc" vector).
#[test]
fn the_guard_extracts_whole_functions_and_hashes_sha256() {
    let source = "fn other() {}\n\n/// Doc.\npub(super) fn valid_fill(a: u8) -> bool {\n    a > 0\n}\nfn valid_fill_more() {}\n";
    assert_eq!(
        item(source, "valid_fill"),
        "pub(super) fn valid_fill(a: u8) -> bool {\n    a > 0\n}"
    );
    let fills = item(EXCHANGE, "terminal_fills");
    assert!(fills.starts_with("    async fn terminal_fills(") && fills.ends_with("\n    }"));
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
