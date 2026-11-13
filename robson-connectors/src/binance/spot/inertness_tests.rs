//! ADR-0056 steps 8a, 8b and 8c-2a merge inert (SD12, design note v16 §12.2).
//!
//! Lexical regression checks over the workspace's Rust sources as they are on
//! disk, backed by a review of each part's diff and call paths:
//! - nothing outside `binance/spot/` reaches this module, names a type it
//!   defines or names a spot endpoint, so no production path, factory included,
//!   can construct its reader, feed, klines or rules cache;
//! - 8a's readers, 8c-2a's rules projection and rules cache import no
//!   `robson_exec` contract and implement no port. 8b's feed and klines import
//!   exactly the existing items of the port each implements (`TradeFeedPort`,
//!   `OhlcvPort`) and nothing else, so no spot source uses anything 8c-2b or a
//!   later step introduces there;
//! - they send no mutation, read no configuration and name no testnet host.
//!
//! `robsond`'s venue tests check the other half: the factory still refuses
//! `binance_spot`, and builds no feed on the spot stream base.

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

/// The `robson_exec` items each 8b adapter takes: the existing port it
/// implements, with that port's own types. No other spot file takes any.
const PORT_ITEMS: [(&str, &[&str]); 2] = [
    ("market_data.rs", &["TradeFeedMessage", "TradeFeedPort", "TradeStream"]),
    ("ohlcv.rs", &["CandleInterval", "ExecError", "OhlcvPort"]),
];

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
        let imported = code.find("use robson_exec::{").map(|at| {
            let rest = &code[at + "use robson_exec::{".len()..];
            let mut items: Vec<&str> = rest[..rest.find("};").unwrap_or(rest.len())]
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .collect();
            items.sort_unstable();
            items
        });
        let mut expected = allowed.to_vec();
        expected.sort_unstable();
        if uses != 1 || imported.as_ref() != Some(&expected) {
            violations.push(format!("takes {imported:?} from robson_exec, not {expected:?}"));
        }
    }
    for contract in CONTRACTS.iter().filter(|contract| !allowed.contains(contract)) {
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
        "SpotTradePage",
        "SpotRulesCache",
        "SpotRulesError",
    ] {
        assert!(names.iter().any(|name| name == expected), "{expected} in {names:?}");
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

/// SD12 point 1: 8a's readers take nothing from `robson_exec`, where the
/// port contracts and 8c's additions live, and 8b's feed and klines take only
/// the existing port each implements. No spot source uses a later contract.
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

/// No mutation, and SD12 point 3 for this module: no configuration key.
#[test]
fn the_spot_module_sends_no_mutation_and_reads_no_configuration() {
    let sources = spot_production_sources();
    assert!(sources.len() >= 7, "{} production sources", sources.len());
    for (path, text) in sources {
        for mutation in [
            "post_signed",
            "delete_signed",
            "pass_gate",
            ".post(",
            ".put(",
            ".delete(",
            ".patch(",
        ] {
            assert!(!text.contains(mutation), "{} contains {mutation}", path.display());
        }
        for configuration in ["env::var", "std::env", "ROBSON_", "dotenv"] {
            assert!(!text.contains(configuration), "{} reads {configuration}", path.display());
        }
    }
}
