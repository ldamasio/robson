//! ADR-0056 steps 8a, 8b, 8c-2a, 8c-2b and 8c-3a merge inert (SD12, design
//! note v16 §12.2).
//!
//! Lexical regression checks over the workspace's Rust sources as they are on
//! disk, backed by a review of each part's diff and call paths (a name split
//! across lines, generated code or a file these filters skip can escape them):
//! - nothing outside `binance/spot/` reaches this module, names a type it
//!   defines or names a spot endpoint, so no production path, factory included,
//!   can construct its reader, feed, klines, rules cache or venue;
//! - 8a's readers, 8c-2a's rules projection and rules cache import no
//!   `robson_exec` item and implement no port. 8b's feed and klines import
//!   exactly the existing items of the port each implements (`TradeFeedPort`,
//!   `OhlcvPort`). 8c-2b's venue, `exchange.rs`, and 8c-3a's order path,
//!   `trading.rs`, import exactly the contracts they implement or fill and the
//!   types those carry, by path;
//! - an order leaves only through `trading.rs`: one `post_signed_answer` call,
//!   inside `SubmitAttemptLedger::submit` on a one-attempt ledger, so it takes
//!   the journal's grant (8c-1) and the client's gate. No spot file calls the
//!   non-answer forms (`.post(`, `post_signed(`, `delete_signed(`), names the
//!   gate, or holds an HTTP client; cancelling waits for 8c-3b;
//! - they read no configuration and name no testnet host.
//!
//! `robsond`'s venue tests check the other half: the factory still refuses
//! `binance_spot`, builds no spot venue, and builds no feed on the spot stream
//! base.

use std::{
    fs,
    path::{Path, PathBuf},
};

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn spot_dir() -> PathBuf {
    crate_dir().join("src/binance/spot")
}

/// Every `.rs` file under `dir`, build output and hidden directories aside.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "target" && name != "node_modules" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// The spot module's sources other than its tests.
fn spot_production_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    rust_files(&spot_dir(), &mut files);
    files
        .into_iter()
        .filter(|path| !path.to_string_lossy().ends_with("_tests.rs"))
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect()
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `text` holds `word` with no identifier character on either side.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Whether `text` reaches the spot module by path: `binance::spot`, a
/// `spot::` path, or `spot` inside a `binance::{...}` group.
fn reaches_spot(text: &str) -> bool {
    let path = text
        .match_indices("spot::")
        .any(|(at, _)| !text[..at].chars().next_back().is_some_and(is_ident));
    let group = text.match_indices("binance::{").any(|(at, found)| {
        let rest = &text[at + found.len()..];
        has_word(&rest[..rest.find('}').unwrap_or(rest.len())], "spot")
    });
    has_word(text, "binance::spot") || path || group
}

/// Item names the spot module defines at top level, whatever their
/// visibility: types, constants and functions.
fn spot_item_names() -> Vec<String> {
    let mut names = Vec::new();
    for (_, text) in spot_production_sources() {
        for line in text.lines() {
            let Some(item) = ["pub(super) ", "pub(crate) ", "pub "]
                .iter()
                .find_map(|visibility| line.strip_prefix(visibility))
            else {
                continue;
            };
            for keyword in ["struct ", "enum ", "trait ", "type ", "const ", "fn "] {
                if let Some(rest) = item.strip_prefix(keyword) {
                    let name: String = rest.chars().take_while(|&c| is_ident(c)).collect();
                    names.push(name);
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Names a reader, record or adapter of this module exposes: never used
/// outside it.
fn spot_type_names() -> Vec<String> {
    spot_item_names()
        .into_iter()
        .filter(|name| {
            ["Spot", "BinanceSpot", "SPOT_", "MY_TRADES_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect()
}

/// Whether `text` names a spot endpoint: the spot stream host (not the
/// futures `fstream` one) or the spot klines path.
fn names_spot_endpoint(text: &str) -> bool {
    has_word(text, "stream.binance.com") || text.contains("/api/v3/klines")
}

/// `text` without its comments: prose may name the contracts.
fn code_only(text: &str) -> String {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `robson_exec` items each adapter takes, by path: 8b's feed and klines
/// the existing port each implements, with that port's own types; 8c's venue
/// exactly the venue contracts it implements and the types they carry. No
/// other spot file takes any.
const PORT_ITEMS: [(&str, &[&str]); 4] = [
    ("market_data.rs", &["TradeFeedMessage", "TradeFeedPort", "TradeStream"]),
    ("ohlcv.rs", &["CandleInterval", "ExecError", "OhlcvPort"]),
    ("exchange.rs", &[
        "ExecError",
        "OrderResult",
        "gate::OperationGate",
        "order_journal::OrderJournal",
        "ports::OpenOrderRecord",
        "ports::TradeFill",
        "ports::UserTradeRecord",
        "quantize",
        "venue::AccountKind",
        "venue::AccountOpenOrder",
        "venue::AccountPort",
        "venue::CancelAnswer",
        "venue::Cash",
        "venue::EntryOrReduce",
        "venue::ExecutionCostEnvelope",
        "venue::FundingPort",
        "venue::GovernedInventory",
        "venue::Holding",
        "venue::InventoryObservation",
        "venue::InventoryPort",
        "venue::Mark",
        "venue::MarketKind",
        "venue::OrderLookup",
        "venue::OrderObservation",
        "venue::OrderPhase",
        "venue::OrderRef",
        "venue::PlacedStop",
        "venue::PositionAccountPort",
        "venue::ProtectionKind",
        "venue::ReplaceOutcome",
        "venue::ReplacementProtocol",
        "venue::SettlementPort",
        "venue::SettlementRecord",
        "venue::StopCancelOutcome",
        "venue::StopParams",
        "venue::StopRef",
        "venue::SubmitAttemptLedger",
        "venue::TradePage",
        "venue::TradingPort",
        "venue::Venue",
        "venue::VenueCapabilities",
        "venue::VenueFinalityRule",
        "venue::VenueId",
        "venue::VenueOrderRecord",
    ]),
    ("trading.rs", &[
        "ExecError",
        "OrderResult",
        "ports::FeeEvidence",
        "ports::FeeGap",
        "ports::OrderEvidence",
        "ports::TradeFill",
        "venue::PlacedStop",
        "venue::ProtectionKind",
        "venue::StopRef",
        "venue::SubmitAttemptLedger",
        "venue::VenueOrderRecord",
    ]),
];

/// The paths a `use` group names, nested groups flattened: `{A, b::{C, D}}`
/// is `A`, `b::C` and `b::D`. `group` is what lies inside the outer braces.
fn flatten_group(group: &str, prefix: &str, out: &mut Vec<String>) {
    let (mut depth, mut start) = (0, 0);
    let mut parts = Vec::new();
    for (at, c) in group.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&group[start..at]);
                start = at + 1;
            },
            _ => {},
        }
    }
    parts.push(&group[start..]);
    for part in parts.into_iter().map(str::trim).filter(|part| !part.is_empty()) {
        match (part.find('{'), part.rfind('}')) {
            (Some(open), Some(close)) => {
                let head = part[..open].trim().trim_end_matches("::");
                flatten_group(&part[open + 1..close], &format!("{prefix}{head}::"), out);
            },
            _ => out.push(format!("{prefix}{part}")),
        }
    }
}

/// The paths of `code`'s `use robson_exec::{...};`, when it has one.
fn robson_exec_imports(code: &str) -> Option<Vec<String>> {
    let at = code.find("use robson_exec::{")?;
    let body = &code[at + "use robson_exec::{".len()..];
    let mut depth = 1;
    let close = body.char_indices().find_map(|(at, c)| {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {},
        }
        (depth == 0).then_some(at)
    })?;
    let mut items = Vec::new();
    flatten_group(&body[..close], "", &mut items);
    items.sort();
    Some(items)
}

/// Contracts a spot file may name only when its allow-list holds them.
const CONTRACTS: [&str; 15] = [
    "ExecError",
    "OrderResult",
    "OrderLookup",
    "Venue",
    "TradingPort",
    "AccountPort",
    "InventoryPort",
    "SettlementPort",
    "FundingPort",
    "MarketDataPort",
    "TradeFeedPort",
    "TradeStream",
    "TradeFeedMessage",
    "OhlcvPort",
    "CandleInterval",
];

/// How `code`, the code of the spot production file `file`, crosses the port
/// boundary. `robson_exec` may appear once, in a `use robson_exec::{...};`
/// of exactly the file's allowed items, and only in an allow-listed file; no
/// contract outside the allow-list is named at all.
fn port_contract_violations(file: &str, code: &str) -> Vec<String> {
    let allowed: &[&str] = PORT_ITEMS
        .iter()
        .find(|(name, _)| *name == file)
        .map_or(&[], |(_, items)| items);
    let mut violations = Vec::new();
    let uses = code
        .match_indices("robson_exec")
        .filter(|(at, found)| {
            !code[..*at].chars().next_back().is_some_and(is_ident)
                && !code[at + found.len()..].chars().next().is_some_and(is_ident)
        })
        .count();
    if allowed.is_empty() {
        if uses > 0 {
            violations.push("takes from robson_exec".to_string());
        }
    } else {
        let imported = robson_exec_imports(code);
        let mut expected: Vec<String> = allowed.iter().map(|item| item.to_string()).collect();
        expected.sort();
        if uses != 1 || imported.as_ref() != Some(&expected) {
            violations.push(format!("takes {imported:?} from robson_exec, not {expected:?}"));
        }
    }
    // A contract is allowed by its own name, whatever path the import took.
    let names: Vec<&str> =
        allowed.iter().map(|item| item.rsplit("::").next().unwrap_or(item)).collect();
    for contract in CONTRACTS.iter().filter(|contract| !names.contains(contract)) {
        if has_word(code, contract) {
            violations.push(format!("names {contract}"));
        }
    }
    violations
}

#[test]
fn the_matchers_find_what_they_look_for() {
    assert!(has_word("let r = SpotRestReader::new(c);", "SpotRestReader"));
    assert!(!has_word("BinanceSpotRestReaderX", "SpotRestReader"));
    for reaching in [
        "use robson_connectors::binance::spot::SpotRestReader;",
        "use robson_connectors::binance::spot as s;",
        "use robson_connectors::binance::{spot, usdm};",
        "use robson_connectors::binance::{usdm::X, spot::Y};",
        "let r = spot::SpotRestReader::new(c);",
    ] {
        assert!(reaches_spot(reaching), "{reaching}");
    }
    for not_reaching in [
        "pub mod spot;",
        "let spot = FeedKey::new(VenueId::new(\"binance:spot\"), s);",
        "ROBSON_VENUE=binance_spot",
        "binance_spot::x",
        "use robson_connectors::binance::{usdm, Endpoints};",
    ] {
        assert!(!reaches_spot(not_reaching), "{not_reaching}");
    }

    for spot_endpoint in [
        "wss://stream.binance.com:9443",
        "wss://stream.binance.com:443/ws/btcusdt@aggTrade",
        "https://api.binance.com/api/v3/klines?symbol=BTCUSDT",
        "SpotRead::public(\"/api/v3/klines\", 2)",
    ] {
        assert!(names_spot_endpoint(spot_endpoint), "{spot_endpoint}");
    }
    for elsewhere in [
        "wss://fstream.binance.com/market",
        "wss://stream.binancefuture.com",
        "/fapi/v1/klines",
        "/api/v3/exchangeInfo",
    ] {
        assert!(!names_spot_endpoint(elsewhere), "{elsewhere}");
    }

    let names = spot_type_names();
    for expected in [
        "BinanceSpotTradeFeed",
        "BinanceSpotOhlcv",
        "SPOT_MARKET_WS_ENDPOINT",
        "SpotRestReader",
        "SpotRead",
        "SpotReadError",
        "SpotOrderRef",
        "SpotSymbolRules",
        "SpotAccount",
        "SpotOrderRecord",
        "SpotTrade",
        "SpotTradesQuery",
        "SpotTradesRead",
        "SpotTradeSet",
        "SpotCommission",
        "MY_TRADES_MAX_PAGES",
        "BinanceSpotAdapter",
        "SPOT_FINALITY",
        "SpotTradePage",
        "SpotRulesCache",
        "SpotRulesError",
        "SpotWrite",
        "SPOT_STP_MODE",
        "SpotOrderAnswer",
        "SpotFill",
    ] {
        assert!(names.iter().any(|name| name == expected), "{expected} in {names:?}");
    }
}

/// Nested `use` groups flatten to paths, whatever their layout.
#[test]
fn a_use_group_flattens_to_its_paths() {
    let code = "use robson_exec::{\n    gate::OperationGate,\n    venue::{\n        TradingPort, \
                Venue,\n    },\n    ExecError,\n};\nfn f() {}";
    assert_eq!(
        robson_exec_imports(code),
        Some(vec![
            "ExecError".to_string(),
            "gate::OperationGate".to_string(),
            "venue::TradingPort".to_string(),
            "venue::Venue".to_string(),
        ])
    );
    assert_eq!(robson_exec_imports("use robson_exec::*;"), None);
}

/// 8c-2's venue and 8c-3a's order path each take their exact list: nothing
/// missing, nothing more, no glob, no inline path, and no feed or klines
/// contract. Neither list is any other file's.
#[test]
fn the_venue_and_the_order_path_take_exactly_their_own_items() {
    let group = |items: &[&str]| format!("use robson_exec::{{{}}};", items.join(", "));
    for owner in ["exchange.rs", "trading.rs"] {
        let (_, items) = PORT_ITEMS.iter().find(|(file, _)| *file == owner).unwrap();
        assert_eq!(port_contract_violations(owner, &group(items)), Vec::<String>::new());
        let fewer = &items[1..];
        let more: Vec<&str> = items.iter().copied().chain(["venue::observe_account"]).collect();
        let mut crossing = vec![
            group(fewer),
            group(&more),
            "use robson_exec::*;".to_string(),
            format!("{}\nfn f(_: robson_exec::gate::OperationGate) {{}}", group(items)),
        ];
        for contract in [
            "TradeFeedPort",
            "TradeStream",
            "TradeFeedMessage",
            "OhlcvPort",
            "MarketDataPort",
        ] {
            crossing.push(format!("{}\nfn f(_: &dyn {contract}) {{}}", group(items)));
        }
        for code in crossing {
            assert!(!port_contract_violations(owner, &code).is_empty(), "{owner} admits {code}");
        }
        // Its items are nobody else's: 8a's readers, the projection, the
        // cache, and the other of the two.
        for file in ["rest.rs", "rules.rs", "cache.rs", "records.rs", "trades.rs"]
            .into_iter()
            .chain(["exchange.rs", "trading.rs"].into_iter().filter(|file| *file != owner))
        {
            assert!(!port_contract_violations(file, &group(items)).is_empty(), "{file}");
        }
    }
}

/// SD12 points 2 and 4: no code outside `binance/spot/` reaches the module
/// or names its types, so no production path can construct its reader.
#[test]
fn nothing_outside_the_spot_module_names_it() {
    let workspace = crate_dir().parent().unwrap();
    let spot = spot_dir();
    let mut files = Vec::new();
    rust_files(workspace, &mut files);
    let outside: Vec<PathBuf> = files.into_iter().filter(|path| !path.starts_with(&spot)).collect();
    // The scan sees the factory and the daemon, not only this crate.
    for must in [
        "robsond/src/venue.rs",
        "robsond/src/main.rs",
        "robson-connectors/src/lib.rs",
    ] {
        assert!(outside.iter().any(|path| path.ends_with(must)), "{must} not scanned");
    }

    let names = spot_type_names();
    let mut found = Vec::new();
    for path in &outside {
        let text = fs::read_to_string(path).unwrap();
        if reaches_spot(&text) {
            found.push(format!("{}: reaches binance::spot", path.display()));
        }
        if names_spot_endpoint(&text) {
            found.push(format!("{}: names a spot endpoint", path.display()));
        }
        for name in names.iter().filter(|name| has_word(&text, name)) {
            found.push(format!("{}: names {name}", path.display()));
        }
    }
    assert!(found.is_empty(), "the spot read side is reachable: {found:#?}");
}

/// The port check admits each 8b adapter's own port items, exactly, and
/// nothing wider: no other item, no other file, no inline path.
#[test]
fn the_port_allow_list_admits_only_each_adapter_s_own_items() {
    let feed =
        "use robson_exec::{\n    TradeFeedMessage,\n    TradeFeedPort,\n    TradeStream,\n};"
            .to_string();
    let ohlcv = "use robson_exec::{CandleInterval, ExecError, OhlcvPort};".to_string();
    assert_eq!(port_contract_violations("market_data.rs", &feed), Vec::<String>::new());
    assert_eq!(port_contract_violations("ohlcv.rs", &ohlcv), Vec::<String>::new());
    assert_eq!(port_contract_violations("rest.rs", "use crate::x;"), Vec::<String>::new());

    let mut crossing = vec![
        // 8a's readers take nothing, 8b's items included.
        ("rest.rs", "use robson_exec::ExecError;".to_string()),
        ("rest.rs", "use robson_exec::gate::OperationGate;".to_string()),
        ("rest.rs", feed.clone()),
        ("mod.rs", "pub use robson_exec::{OhlcvPort};".to_string()),
        ("trades.rs", "fn f(_: &dyn OhlcvPort) {}".to_string()),
        // Each adapter its own items, exactly.
        ("market_data.rs", String::new()),
        ("market_data.rs", "use robson_exec::{TradeFeedPort, TradeStream};".to_string()),
        (
            "market_data.rs",
            "use robson_exec::{TradeFeedMessage, TradeFeedPort, TradeStream, OhlcvPort};"
                .to_string(),
        ),
        (
            "market_data.rs",
            "use robson_exec::{TradeFeedMessage, TradeFeedPort, TradeStream, venue::Mark};"
                .to_string(),
        ),
        ("market_data.rs", "use robson_exec::*;".to_string()),
        (
            "market_data.rs",
            format!("{feed}\nfn f(_: robson_exec::gate::OperationGate) {{}}"),
        ),
        ("market_data.rs", ohlcv.clone()),
        ("ohlcv.rs", feed.clone()),
        ("ohlcv.rs", String::new()),
        (
            "ohlcv.rs",
            "use robson_exec::{CandleInterval, ExecError, OhlcvPort, OrderResult};".to_string(),
        ),
        (
            "ohlcv.rs",
            "use robson_exec::{CandleInterval, ExecError, OhlcvPort, venue::Mark};".to_string(),
        ),
        ("ohlcv.rs", "use robson_exec::*;".to_string()),
    ];
    // No contract outside the file's allow-list, by whatever path.
    let others = [
        "OrderResult",
        "OrderLookup",
        "Venue",
        "TradingPort",
        "AccountPort",
        "InventoryPort",
        "SettlementPort",
        "FundingPort",
        "MarketDataPort",
    ];
    for contract in others.iter().chain(&["ExecError", "OhlcvPort", "CandleInterval"]) {
        crossing.push(("market_data.rs", format!("{feed}\nfn f(_: &dyn {contract}) {{}}")));
    }
    for contract in others.iter().chain(&["TradeFeedPort", "TradeStream", "TradeFeedMessage"]) {
        crossing.push(("ohlcv.rs", format!("{ohlcv}\nfn f(_: &dyn {contract}) {{}}")));
    }
    for (file, code) in crossing {
        assert!(!port_contract_violations(file, &code).is_empty(), "{file} admits {code}");
    }
}

/// SD12 point 1: 8a's readers, the rules projection and the cache take nothing
/// from `robson_exec`, where the port contracts and 8c's additions live; 8b's
/// feed and klines take only the existing port each implements, and 8c-2's
/// venue only its exact list. No spot source uses another contract.
#[test]
fn the_spot_module_uses_no_port_contract_beyond_8b_s_ports() {
    let sources = spot_production_sources();
    for (file, _) in PORT_ITEMS {
        assert!(sources.iter().any(|(path, _)| path.ends_with(file)), "{file} not scanned");
    }
    for (path, text) in sources {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let violations = port_contract_violations(&file, &code_only(&text));
        assert!(violations.is_empty(), "{}: {violations:?}", path.display());
    }
}

/// 8b's adapters read public market data only: the klines through the
/// shared client's unsigned GET, the feed through its `aggTrade`
/// subscription. No spot source names a testnet or futures host.
#[test]
fn the_feed_and_the_klines_read_public_market_data_only() {
    let sources = spot_production_sources();
    let code = |file: &str| {
        let (_, text) = sources
            .iter()
            .find(|(path, _)| path.ends_with(file))
            .unwrap_or_else(|| panic!("{file} not found"));
        code_only(text)
    };
    let ohlcv = code("ohlcv.rs");
    assert!(ohlcv.contains("get_public_answer"));
    for signed in ["get_signed", "build_signed_query", "api_key"] {
        assert!(!ohlcv.contains(signed), "ohlcv.rs contains {signed}");
    }
    let feed = code("market_data.rs");
    assert!(feed.contains("subscribe_agg_trade"));
    for other in [
        "subscribe_user_data",
        "subscribe_ticker",
        "subscribe_kline",
        "listen",
    ] {
        assert!(!feed.contains(other), "market_data.rs contains {other}");
    }
    for (path, text) in &sources {
        for host in ["binance.vision", "binancefuture", "fapi.binance", "fstream"] {
            assert!(!text.contains(host), "{} names {host}", path.display());
        }
    }
}

/// `text` without whitespace.
fn squeezed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether the mention of the POST helper that ends `before`, the code of
/// `trading.rs` up to it, sits inside the argument list of one
/// `ledger.submit(...)` call, in a function that takes
/// `ledger: &SubmitAttemptLedger`.
fn inside_ledger_submit(before: &str) -> bool {
    let Some(start) = before
        .match_indices("fn ")
        .filter(|(at, _)| !before[..*at].chars().next_back().is_some_and(is_ident))
        .map(|(at, _)| at)
        .last()
    else {
        return false;
    };
    let function = &before[start..];
    let signature = squeezed(&function[..function.find('{').unwrap_or(function.len())]);
    if !signature.contains("ledger:&SubmitAttemptLedger") {
        return false;
    }
    let body = squeezed(function);
    let Some(submit) = body.rfind("ledger.submit(") else {
        return false;
    };
    let mut depth = 1;
    for c in body[submit + "ledger.submit(".len()..].chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {},
        }
        if depth == 0 {
            return false;
        }
    }
    true
}

/// How `text`, the spot production file `file`, could send a mutation other
/// than through the POST answer helper under the one-attempt ledger: a
/// non-answer form (`.post(`, `post_signed(`, `delete_signed(`, any other
/// verb), the gate or an HTTP client by name, a ledger that is not
/// one-attempt, the helper outside `trading.rs`, or a call of it there
/// outside `ledger.submit(...)`. Cancelling is 8c-3b's: no DELETE helper yet.
fn mutation_violations(file: &str, text: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for form in [
        ".post(",
        ".put(",
        ".delete(",
        ".patch(",
        "post_signed(",
        "delete_signed(",
        "reqwest",
        "SubmitAttemptLedger::new(",
    ] {
        if text.contains(form) {
            violations.push(format!("contains {form}"));
        }
    }
    for name in [
        "post_signed",
        "delete_signed",
        "pass_gate",
        "delete_signed_answer",
    ] {
        if has_word(text, name) {
            violations.push(format!("names {name}"));
        }
    }
    let helper = "post_signed_answer";
    if file != "trading.rs" {
        if has_word(text, helper) {
            violations.push(format!("names {helper} outside trading.rs"));
        }
        return violations;
    }
    let code = code_only(text);
    for (at, _) in code.match_indices(helper) {
        let word = !code[..at].chars().next_back().is_some_and(is_ident)
            && !code[at + helper.len()..].chars().next().is_some_and(is_ident);
        if word && !inside_ledger_submit(&code[..at]) {
            violations.push(format!("calls {helper} outside SubmitAttemptLedger::submit"));
        }
    }
    violations
}

/// The checker refuses each way around the ledger, and admits the one way
/// through it.
#[test]
fn the_mutation_checker_refuses_every_way_around_the_ledger() {
    let through =
        "async fn send(client: &C, ledger: &SubmitAttemptLedger) -> R {\n    ledger\n        \
                   .submit(id, rejection, async {\n            \
                   read(client.post_signed_answer(PATH, params).await)\n        })\n        \
                   .await\n}";
    assert_eq!(mutation_violations("trading.rs", through), Vec::<String>::new());
    let around = [
        // No ledger at all.
        ("trading.rs", "async fn f(client: &C) { client.post_signed_answer(P, p).await }"),
        // After the ledger's call, not inside it.
        (
            "trading.rs",
            "async fn f(client: &C, ledger: &SubmitAttemptLedger) { ledger.submit(i, r, async \
             {}).await; client.post_signed_answer(P, p).await }",
        ),
        (
            "trading.rs",
            "async fn f(client: &C, ledger: &SubmitAttemptLedger) { ledger.submit(i, r, async \
             {}).await.and(client.post_signed_answer(P, p).await) }",
        ),
        // A `submit` on something that is not the ledger parameter.
        (
            "trading.rs",
            "async fn f(client: &C, ledger: &Other) { ledger.submit(i, r, async { \
             client.post_signed_answer(P, p).await }).await }",
        ),
        // The ledger in another function than the call.
        (
            "trading.rs",
            "fn a(ledger: &SubmitAttemptLedger) { ledger.submit(i, r, x) }\nasync fn b(client: \
             &C) { client.post_signed_answer(P, p).await }",
        ),
        // The helper outside trading.rs, even through the ledger.
        (
            "exchange.rs",
            "async fn f(ledger: &SubmitAttemptLedger) { ledger.submit(i, r, async { \
             self.client.post_signed_answer(P, p).await }).await }",
        ),
        // The non-answer forms, the gate, an HTTP client, a plain ledger.
        ("trading.rs", "fn f() { self.client.post(&url) }"),
        ("trading.rs", "fn f() { self.client.post_signed(\"/api/v3/order\", p) }"),
        ("trading.rs", "fn f() { self.client.delete_signed(\"/api/v3/order\", p) }"),
        ("exchange.rs", "fn f() { client.delete_signed_answer(P, p) }"),
        ("trading.rs", "fn f() { client.pass_gate(\"POST\", P) }"),
        ("rest.rs", "use reqwest::Client;"),
        ("exchange.rs", "fn f() -> SubmitAttemptLedger { SubmitAttemptLedger::new(64) }"),
    ];
    for (file, code) in around {
        assert!(!mutation_violations(file, code).is_empty(), "{file} admits {code}");
    }
}

/// An order leaves only through `trading.rs`'s one POST, inside the
/// one-attempt ledger; SD12 point 3 for this module: no configuration key.
#[test]
fn the_spot_module_sends_orders_only_through_the_ledger_and_reads_no_configuration() {
    let sources = spot_production_sources();
    assert!(sources.len() >= 7, "{} production sources", sources.len());
    for (path, text) in &sources {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let violations = mutation_violations(&file, text);
        assert!(violations.is_empty(), "{}: {violations:?}", path.display());
        for configuration in ["env::var", "std::env", "ROBSON_", "dotenv"] {
            assert!(!text.contains(configuration), "{} reads {configuration}", path.display());
        }
    }
    let code = |file: &str| {
        let (_, text) = sources.iter().find(|(path, _)| path.ends_with(file)).unwrap();
        code_only(text)
    };
    assert_eq!(code("trading.rs").matches("post_signed_answer").count(), 1, "one POST call");
    assert!(code("exchange.rs").contains("SubmitAttemptLedger::one_attempt("));
}
