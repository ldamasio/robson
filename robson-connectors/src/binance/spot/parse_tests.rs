//! ADR-0056 step 8a: the spot parsers against Binance's documented responses
//! and the live BTCUSDT `exchangeInfo` (`testdata/sources.json` says where
//! each fixture comes from).

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    records::{parse_account, parse_commission, parse_order, parse_order_list_count, parse_orders},
    rules::parse_symbol_rules,
    SpotLotFilter, SpotReadError, SpotSymbolRules, SpotTrade,
};

const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
const ACCOUNT: &str = include_str!("testdata/account.json");
const ORDER: &str = include_str!("testdata/order.json");
const OPEN_ORDERS: &str = include_str!("testdata/open_orders.json");
const OPEN_ORDER_LIST: &str = include_str!("testdata/open_order_list.json");
const MY_TRADES: &str = include_str!("testdata/my_trades.json");
const COMMISSION: &str = include_str!("testdata/commission.json");
const SOURCES: &str = include_str!("testdata/sources.json");

fn value(fixture: &str) -> Value {
    serde_json::from_str(fixture).unwrap()
}

fn btcusdt() -> SpotSymbolRules {
    parse_symbol_rules(EXCHANGE_INFO, "BTCUSDT").unwrap()
}

/// The BTCUSDT entry with `edit` applied to its filter of type `kind`.
fn with_filter(kind: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut info = value(EXCHANGE_INFO);
    let filters = info["symbols"][0]["filters"].as_array_mut().unwrap();
    let filter = filters.iter_mut().find(|f| f["filterType"] == kind).unwrap();
    edit(filter);
    info.to_string()
}

fn without_filter(kind: &str) -> String {
    let mut info = value(EXCHANGE_INFO);
    info["symbols"][0]["filters"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["filterType"] != kind);
    info.to_string()
}

fn market_lot_error(rules: &SpotSymbolRules) -> String {
    match rules.market_lot() {
        Err(SpotReadError::InvalidRules(why)) => why,
        other => panic!("expected InvalidRules, got {other:?}"),
    }
}

#[test]
fn btcusdt_rules_parse_as_published() {
    let rules = btcusdt();
    assert_eq!(rules.symbol, "BTCUSDT");
    assert_eq!(rules.status, "TRADING");
    assert_eq!((rules.base_asset.as_str(), rules.quote_asset.as_str()), ("BTC", "USDT"));
    assert_eq!((rules.base_asset_precision, rules.quote_asset_precision), (8, 8));
    for order_type in ["MARKET", "STOP_LOSS"] {
        assert!(rules.order_types.iter().any(|t| t == order_type), "{order_type}");
    }
    // F5: one permission set of 224 values.
    assert_eq!(rules.permission_sets.len(), 1);
    assert_eq!(rules.permission_sets[0].len(), 224);
    assert!(rules.permission_sets[0].iter().any(|p| p == "SPOT"));

    let price = rules.price_filter.as_ref().unwrap();
    assert_eq!(
        (price.min_price, price.max_price, price.tick_size),
        (dec!(0.01), dec!(1000000), dec!(0.01))
    );
    assert_eq!(
        rules.lot_size,
        Some(SpotLotFilter {
            min_qty: dec!(0.00001),
            max_qty: dec!(9000),
            step_size: dec!(0.00001),
        })
    );
    // F6: MARKET_LOT_SIZE carries zeros for its minimum and step.
    assert_eq!(
        rules.market_lot_size,
        Some(SpotLotFilter {
            min_qty: Decimal::ZERO,
            max_qty: dec!(108.95042220),
            step_size: Decimal::ZERO,
        })
    );
    let notional = rules.notional.as_ref().unwrap();
    assert_eq!(notional.min_notional, dec!(5));
    assert!(notional.apply_min_to_market);
    assert_eq!(notional.max_notional, dec!(9000000));
    assert!(!notional.apply_max_to_market);
    assert_eq!(notional.avg_price_mins, 5);
    assert_eq!(rules.max_num_algo_orders, Some(5));
}

#[test]
fn a_zero_market_lot_minimum_or_step_adds_no_constraint() {
    assert_eq!(btcusdt().market_lot().unwrap(), SpotLotFilter {
        min_qty: dec!(0.00001),
        max_qty: dec!(108.95042220),
        step_size: dec!(0.00001),
    });
}

#[test]
fn a_zero_lot_size_minimum_or_step_adds_no_constraint_either() {
    let body = with_filter("LOT_SIZE", |f| {
        f["minQty"] = json!("0.00000000");
        f["stepSize"] = json!("0.00000000");
    });
    let body = {
        let mut info = value(&body);
        let filters = info["symbols"][0]["filters"].as_array_mut().unwrap();
        let market = filters.iter_mut().find(|f| f["filterType"] == "MARKET_LOT_SIZE").unwrap();
        market["minQty"] = json!("0.00010000");
        market["stepSize"] = json!("0.00010000");
        info.to_string()
    };
    let rules = parse_symbol_rules(&body, "BTCUSDT").unwrap();
    assert_eq!(rules.market_lot().unwrap(), SpotLotFilter {
        min_qty: dec!(0.0001),
        max_qty: dec!(108.95042220),
        step_size: dec!(0.0001),
    });
}

#[test]
fn nested_steps_take_the_coarser_and_others_are_refused() {
    let coarser = with_filter("MARKET_LOT_SIZE", |f| f["stepSize"] = json!("0.00100000"));
    let rules = parse_symbol_rules(&coarser, "BTCUSDT").unwrap();
    assert_eq!(rules.market_lot().unwrap().step_size, dec!(0.001));

    let crossed = with_filter("MARKET_LOT_SIZE", |f| f["stepSize"] = json!("0.00002500"));
    let rules = parse_symbol_rules(&crossed, "BTCUSDT").unwrap();
    assert!(market_lot_error(&rules).contains("do not nest"));
}

/// What the lot cannot decide stays visible for 8c's projection to refuse:
/// no step on either filter, or a zero maximum, taken as written.
#[test]
fn zero_steps_and_a_zero_maximum_are_left_as_written() {
    let no_step = with_filter("LOT_SIZE", |f| f["stepSize"] = json!("0.00000000"));
    let rules = parse_symbol_rules(&no_step, "BTCUSDT").unwrap();
    assert_eq!(rules.market_lot().unwrap().step_size, Decimal::ZERO);

    let no_max = with_filter("MARKET_LOT_SIZE", |f| f["maxQty"] = json!("0.00000000"));
    let rules = parse_symbol_rules(&no_max, "BTCUSDT").unwrap();
    assert_eq!(rules.market_lot().unwrap().max_qty, Decimal::ZERO);
}

#[test]
fn optional_filters_parse_absent_and_the_lot_needs_lot_size() {
    let rules = parse_symbol_rules(&without_filter("LOT_SIZE"), "BTCUSDT").unwrap();
    assert_eq!(rules.lot_size, None);
    assert!(market_lot_error(&rules).contains("no LOT_SIZE"));

    let rules = parse_symbol_rules(&without_filter("PRICE_FILTER"), "BTCUSDT").unwrap();
    assert_eq!(rules.price_filter, None);
    let rules = parse_symbol_rules(&without_filter("NOTIONAL"), "BTCUSDT").unwrap();
    assert_eq!(rules.notional, None);
    let rules = parse_symbol_rules(&without_filter("MAX_NUM_ALGO_ORDERS"), "BTCUSDT").unwrap();
    assert_eq!(rules.max_num_algo_orders, None);
    let rules = parse_symbol_rules(&without_filter("MARKET_LOT_SIZE"), "BTCUSDT").unwrap();
    assert_eq!(rules.market_lot_size, None);
    assert_eq!(rules.market_lot().unwrap(), rules.lot_size.clone().unwrap());
}

#[test]
fn an_ambiguous_or_foreign_exchange_info_is_refused() {
    let mut twice = value(EXCHANGE_INFO);
    let filters = twice["symbols"][0]["filters"].as_array_mut().unwrap();
    filters.push(filters[0].clone());
    assert!(matches!(
        parse_symbol_rules(&twice.to_string(), "BTCUSDT"),
        Err(SpotReadError::Inconsistent(why)) if why.contains("PRICE_FILTER twice")
    ));

    let mut untyped = value(EXCHANGE_INFO);
    untyped["symbols"][0]["filters"]
        .as_array_mut()
        .unwrap()
        .push(json!({"minQty": "1"}));
    assert!(matches!(
        parse_symbol_rules(&untyped.to_string(), "BTCUSDT"),
        Err(SpotReadError::Parse(_))
    ));

    let malformed = with_filter("LOT_SIZE", |f| f["stepSize"] = json!("a lot"));
    assert!(matches!(
        parse_symbol_rules(&malformed, "BTCUSDT"),
        Err(SpotReadError::Parse(_))
    ));

    assert!(matches!(
        parse_symbol_rules(EXCHANGE_INFO, "ETHUSDT"),
        Err(SpotReadError::Inconsistent(why)) if why.contains("no entry for ETHUSDT")
    ));
    let mut listed_twice = value(EXCHANGE_INFO);
    let entry = listed_twice["symbols"][0].clone();
    listed_twice["symbols"].as_array_mut().unwrap().push(entry);
    assert!(matches!(
        parse_symbol_rules(&listed_twice.to_string(), "BTCUSDT"),
        Err(SpotReadError::Inconsistent(why)) if why.contains("twice")
    ));

    let mut no_permission_sets = value(EXCHANGE_INFO);
    no_permission_sets["symbols"][0]
        .as_object_mut()
        .unwrap()
        .remove("permissionSets");
    assert!(matches!(
        parse_symbol_rules(&no_permission_sets.to_string(), "BTCUSDT"),
        Err(SpotReadError::Parse(_))
    ));
}

#[test]
fn the_account_parses_as_documented() {
    let account = parse_account(ACCOUNT).unwrap();
    assert_eq!(account.uid, 354937868);
    assert!(account.can_trade);
    assert_eq!(account.account_type, "SPOT");
    assert_eq!(account.permissions, ["SPOT"]);
    assert_eq!(account.balances.len(), 2);
    let btc = account.balance("BTC").unwrap();
    assert_eq!((btc.free, btc.locked), (dec!(4723846.89208129), Decimal::ZERO));
    assert!(account.balance("USDT").is_none());

    let mut empty = value(ACCOUNT);
    empty["balances"] = json!([]);
    assert!(parse_account(&empty.to_string()).unwrap().balances.is_empty());
}

#[test]
fn an_account_listing_an_asset_twice_or_missing_a_field_is_refused() {
    let mut twice = value(ACCOUNT);
    let first = twice["balances"][0].clone();
    twice["balances"].as_array_mut().unwrap().push(first);
    assert!(matches!(
        parse_account(&twice.to_string()),
        Err(SpotReadError::Inconsistent(why)) if why.contains("BTC twice")
    ));

    for field in ["uid", "canTrade", "accountType", "permissions", "balances"] {
        let mut missing = value(ACCOUNT);
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            matches!(parse_account(&missing.to_string()), Err(SpotReadError::Parse(_))),
            "{field}"
        );
    }
}

#[test]
fn an_order_parses_as_documented() {
    let order = parse_order(ORDER).unwrap();
    assert_eq!((order.symbol.as_str(), order.order_id), ("LTCBTC", 1));
    assert_eq!(order.order_list_id, -1);
    assert_eq!(order.client_order_id, "myOrder1");
    assert_eq!(order.price, dec!(0.1));
    assert_eq!((order.orig_qty, order.executed_qty), (dec!(1.0), dec!(0.0)));
    assert_eq!(order.cummulative_quote_qty, Some(dec!(0.0)));
    assert_eq!(order.status, "NEW");
    assert_eq!(order.time_in_force, "GTC");
    assert_eq!((order.order_type.as_str(), order.side.as_str()), ("LIMIT", "BUY"));
    assert_eq!(order.stop_price, Some(Decimal::ZERO));
    assert_eq!((order.time, order.update_time), (1499827319559, 1499827319559));
    assert_eq!(order.self_trade_prevention_mode, "NONE");
    assert_eq!(order.expiry_reason.as_deref(), Some("INSUFFICIENT_LIQUIDITY"));
}

#[test]
fn a_stop_loss_record_keeps_its_stop_and_cumulative_execution() {
    let mut stop = value(ORDER);
    stop["symbol"] = json!("BTCUSDT");
    stop["type"] = json!("STOP_LOSS");
    stop["side"] = json!("SELL");
    stop["status"] = json!("EXPIRED");
    stop["origQty"] = json!("0.00010000");
    stop["executedQty"] = json!("0.00004000");
    stop["cummulativeQuoteQty"] = json!("2.80000000");
    stop["stopPrice"] = json!("70000.00000000");
    stop["expiryReason"] = json!("INSUFFICIENT_LIQUIDITY");
    let order = parse_order(&stop.to_string()).unwrap();
    assert_eq!(order.stop_price, Some(dec!(70000)));
    assert_eq!((order.orig_qty, order.executed_qty), (dec!(0.0001), dec!(0.00004)));
    assert_eq!(order.cummulative_quote_qty, Some(dec!(2.8)));
}

#[test]
fn conditional_order_fields_are_optional_and_the_rest_are_required() {
    let mut bare = value(ORDER);
    for conditional in [
        "stopPrice",
        "expiryReason",
        "icebergQty",
        "preventedMatchId",
    ] {
        bare.as_object_mut().unwrap().remove(conditional);
    }
    let order = parse_order(&bare.to_string()).unwrap();
    assert_eq!((order.stop_price, order.expiry_reason), (None, None));

    // A missing quantity must fail the read, never read as zero.
    for field in [
        "symbol",
        "orderId",
        "orderListId",
        "clientOrderId",
        "price",
        "origQty",
        "executedQty",
        "cummulativeQuoteQty",
        "status",
        "timeInForce",
        "type",
        "side",
        "time",
        "updateTime",
        "selfTradePreventionMode",
    ] {
        let mut missing = value(ORDER);
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            matches!(parse_order(&missing.to_string()), Err(SpotReadError::Parse(_))),
            "{field}"
        );
    }
}

#[test]
fn a_negative_cumulative_quote_quantity_reads_as_unknown() {
    // "For some historical orders `cummulativeQuoteQty` will be < 0, meaning
    // the data is not available at this time." (rest-api.md L4362)
    let mut historical = value(ORDER);
    historical["cummulativeQuoteQty"] = json!("-1.00000000");
    assert_eq!(parse_order(&historical.to_string()).unwrap().cummulative_quote_qty, None);
}

#[test]
fn open_orders_parse_as_documented_and_may_be_empty() {
    let orders = parse_orders(OPEN_ORDERS).unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0], parse_order(&value(OPEN_ORDERS)[0].to_string()).unwrap());
    assert!(parse_orders("[]").unwrap().is_empty());
    assert!(matches!(parse_orders("{}"), Err(SpotReadError::Parse(_))));
}

#[test]
fn only_the_open_order_list_count_is_read() {
    assert_eq!(parse_order_list_count(OPEN_ORDER_LIST).unwrap(), 1);
    assert_eq!(parse_order_list_count("[]").unwrap(), 0);
    assert!(matches!(parse_order_list_count("[{}]"), Err(SpotReadError::Parse(_))));
}

#[test]
fn commission_rates_parse_as_documented() {
    let commission = parse_commission(COMMISSION, "BTCUSDT").unwrap();
    let standard = &commission.standard;
    assert_eq!(
        (standard.maker, standard.taker, standard.buyer, standard.seller),
        (dec!(0.00000010), dec!(0.00000020), dec!(0.00000030), dec!(0.00000040))
    );
    let special = &commission.special;
    assert_eq!(
        (special.maker, special.taker, special.buyer, special.seller),
        (dec!(0.01), dec!(0.02), dec!(0.03), dec!(0.04))
    );
    let tax = &commission.tax;
    assert_eq!(
        (tax.maker, tax.taker, tax.buyer, tax.seller),
        (dec!(0.00000112), dec!(0.00000114), dec!(0.00000118), dec!(0.00000116))
    );
    let discount = &commission.discount;
    assert!(discount.enabled_for_account && discount.enabled_for_symbol);
    assert_eq!((discount.discount_asset.as_str(), discount.discount), ("BNB", dec!(0.75)));

    assert!(matches!(
        parse_commission(COMMISSION, "ETHUSDT"),
        Err(SpotReadError::Inconsistent(_))
    ));
    for block in [
        "standardCommission",
        "specialCommission",
        "taxCommission",
        "discount",
    ] {
        let mut missing = value(COMMISSION);
        missing.as_object_mut().unwrap().remove(block);
        assert!(
            matches!(
                parse_commission(&missing.to_string(), "BTCUSDT"),
                Err(SpotReadError::Parse(_))
            ),
            "{block}"
        );
    }
}

#[test]
fn a_trade_parses_as_documented() {
    let trades: Vec<SpotTrade> = serde_json::from_str(MY_TRADES).unwrap();
    assert_eq!(trades, [SpotTrade {
        symbol: "BNBBTC".to_string(),
        id: 28457,
        order_id: 100234,
        order_list_id: -1,
        price: dec!(4.00000100),
        qty: dec!(12),
        quote_qty: dec!(48.000012),
        commission: dec!(10.1),
        commission_asset: "BNB".to_string(),
        time: 1499865549590,
        is_buyer: true,
        is_maker: false,
        is_best_match: true,
    }]);
}

/// Every fixture says where it came from and when it was read: 8a's on
/// 2026-11-10, 8b's on 2026-11-11. A live one is a public request, kept
/// unmodified: its bytes have the sha256 recorded.
#[test]
fn every_fixture_states_its_source_and_access_date() {
    let live = [
        "https://api.binance.com/api/v3/exchangeInfo?symbol=BTCUSDT",
        "https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=3",
    ];
    let sources = value(SOURCES);
    let listed = sources["fixtures"].as_object().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/binance/spot/testdata");
    let mut fixtures: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != "sources.json")
        .collect();
    fixtures.sort();
    let mut named: Vec<String> = listed.keys().cloned().collect();
    named.sort();
    assert_eq!(fixtures, named);
    for (name, source) in listed {
        let url = source["source"].as_str().unwrap();
        assert!(
            url.starts_with(
                "https://github.com/binance/binance-spot-api-docs/blob/\
                 995f6eb39dda0ccba328247ed7087fa7e5b9c5dd/"
            ) || live.contains(&url),
            "{name}: {url}"
        );
        let accessed = source["accessed"].as_str().unwrap();
        assert!(
            ["2026-11-10", "2026-11-11", "2026-11-13"].contains(&accessed),
            "{name}: {accessed}"
        );
        let how = source["how"].as_str().unwrap();
        assert!(!how.is_empty(), "{name}");
        if live.contains(&url) {
            let digest = hex::encode(Sha256::digest(std::fs::read(dir.join(name)).unwrap()));
            assert!(how.contains(&format!("sha256 {digest}")), "{name}: {digest}");
        }
    }
}
