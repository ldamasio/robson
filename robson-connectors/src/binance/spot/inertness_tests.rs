//! ADR-0056 step 8a merges inert (SD12, design note v8 §12.2).
//!
//! Grep-based, over the workspace's Rust sources as they are on disk:
//! - nothing outside `binance/spot/` reaches this module or names a type it
//!   defines, so no production path, factory included, can construct it;
//! - its production sources import no `robson_exec` contract, so they use
//!   nothing 8c or a later step introduces there, and implement no port;
//! - they send no mutation and read no configuration.
//!
//! `robsond`'s venue tests check the other half: the factory still refuses
//! `binance_spot`.

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

/// Names a reader or record of this module exposes: never used outside it.
fn spot_type_names() -> Vec<String> {
    spot_item_names()
        .into_iter()
        .filter(|name| name.starts_with("Spot") || name.starts_with("MY_TRADES_"))
        .collect()
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

    let names = spot_type_names();
    for expected in [
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
        for name in names.iter().filter(|name| has_word(&text, name)) {
            found.push(format!("{}: names {name}", path.display()));
        }
    }
    assert!(found.is_empty(), "the spot read side is reachable: {found:#?}");
}

/// SD12 point 1, and the scope of 8a: the module's production sources take
/// nothing from `robson_exec`, where the port contracts and 8c's additions
/// live, so they implement no port and use no later contract.
#[test]
fn the_spot_module_uses_no_port_contract() {
    for (path, text) in spot_production_sources() {
        // Code only: prose may name what 8c will map these records onto.
        let text: String = text
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        for contract in [
            "robson_exec",
            "ExecError",
            "OrderResult",
            "OrderLookup",
            "Venue",
        ] {
            assert!(!has_word(&text, contract), "{} uses {contract}", path.display());
        }
        for port in [
            "TradingPort",
            "AccountPort",
            "InventoryPort",
            "SettlementPort",
            "FundingPort",
        ] {
            assert!(!has_word(&text, port), "{} names {port}", path.display());
        }
    }
}

/// No mutation, and SD12 point 3 for this module: no configuration key.
#[test]
fn the_spot_module_sends_no_mutation_and_reads_no_configuration() {
    let sources = spot_production_sources();
    assert!(sources.len() >= 5, "{} production sources", sources.len());
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
