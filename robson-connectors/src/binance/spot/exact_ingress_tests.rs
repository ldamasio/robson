//! Sizing ingress only. Shared USD-M/order/trade parsers remain unchanged.
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::{json, Value};

use super::{
    market_data::decode_message,
    ohlcv::parse_spot_klines,
    records::{parse_account, parse_commission},
    rules::parse_symbol_rules,
    stand_in, SpotRestReader,
};
use crate::{binance_rest::parse_futures_klines, binance_ws::WsMessage};

const ACCOUNT: &str = include_str!("testdata/account.json");
const COMMISSION: &str = include_str!("testdata/commission.json");
const RULES: &str = include_str!("testdata/exchange_info_btcusdt.json");
const KLINES: &str = include_str!("testdata/klines.json");
const FEED: &str = include_str!("testdata/agg_trade.json");
const LOSSY: &str = "0.00000000000000000000000000001";

fn bad_inputs() -> [Value; 5] {
    [
        json!(LOSSY),
        json!(format!("-{LOSSY}")),
        json!(-1),
        json!(0.1),
        json!("-1"),
    ]
}

#[test]
fn every_commission_component_and_discount_has_exact_nonnegative_ingress() {
    let original: Value = serde_json::from_str(COMMISSION).unwrap();
    for kind in ["standardCommission", "taxCommission", "specialCommission"] {
        for side in ["maker", "taker", "buyer", "seller"] {
            for bad in bad_inputs() {
                let mut body = original.clone();
                body[kind][side] = bad;
                assert!(parse_commission(&body.to_string(), "BTCUSDT").is_err(), "{kind}/{side}");
            }
        }
    }
    for bad in bad_inputs() {
        let mut body = original.clone();
        body["discount"]["discount"] = bad;
        assert!(parse_commission(&body.to_string(), "BTCUSDT").is_err(), "discount");
    }
    assert!(parse_commission(COMMISSION, "BTCUSDT").is_ok());
}

#[test]
fn free_and_locked_cash_are_exact_before_domain_projection() {
    let original: Value = serde_json::from_str(ACCOUNT).unwrap();
    for field in ["free", "locked"] {
        for bad in bad_inputs() {
            let mut body = original.clone();
            body["balances"][0][field] = bad;
            assert!(parse_account(&body.to_string()).is_err(), "{field}");
        }
        let mut body = original.clone();
        body["balances"][0][field] = json!(Decimal::MAX.to_string());
        let account = parse_account(&body.to_string()).unwrap();
        let amount = if field == "free" {
            account.balances[0].free
        } else {
            account.balances[0].locked
        };
        assert_eq!(amount, Decimal::MAX);
    }
}

#[test]
fn every_sizing_filter_decimal_rejects_rounded_or_numeric_input() {
    for (kind, fields) in [
        ("PRICE_FILTER", &["minPrice", "maxPrice", "tickSize"][..]),
        ("LOT_SIZE", &["minQty", "maxQty", "stepSize"][..]),
        ("MARKET_LOT_SIZE", &["minQty", "maxQty", "stepSize"][..]),
        ("NOTIONAL", &["minNotional", "maxNotional"][..]),
    ] {
        for field in fields {
            for bad in bad_inputs() {
                let mut body: Value = serde_json::from_str(RULES).unwrap();
                let filters = body["symbols"][0]["filters"].as_array_mut().unwrap();
                let filter = filters.iter_mut().find(|f| f["filterType"] == kind).unwrap();
                filter[*field] = bad;
                assert!(
                    parse_symbol_rules(&body.to_string(), "BTCUSDT").is_err(),
                    "{kind}/{field}"
                );
            }
        }
    }
}

#[test]
fn spot_kline_prices_and_volume_are_exact_but_legacy_parser_is_preserved() {
    for fixture in [KLINES, include_str!("testdata/klines_btcusdt_1m.json")] {
        assert_eq!(parse_spot_klines(fixture).unwrap(), parse_futures_klines(fixture).unwrap());
    }
    for index in 1..=5 {
        for bad in bad_inputs() {
            let mut rows: Value = serde_json::from_str(KLINES).unwrap();
            rows[0][index] = bad;
            assert!(parse_spot_klines(&rows.to_string()).is_err(), "field {index}");
        }
    }
    let mut rows: Value = serde_json::from_str(KLINES).unwrap();
    rows[0][4] = json!(LOSSY);
    assert!(parse_spot_klines(&rows.to_string()).is_err());
    // This is a preservation assertion, not permission for spot to round.
    assert_eq!(parse_futures_klines(&rows.to_string()).unwrap()[0].close, Decimal::ZERO);
}

#[test]
fn spot_feed_price_rejects_a_nonzero_discarded_digit() {
    let mut frame: Value = serde_json::from_str(FEED).unwrap();
    frame["p"] = json!("1.00000000000000000000000000001");
    let decoded: WsMessage = serde_json::from_str(&frame.to_string()).unwrap();
    assert!(matches!(
        decode_message(decoded),
        robson_exec::TradeFeedMessage::Malformed { .. }
    ));
}

#[tokio::test]
async fn ticker_exact_ingress_preserves_one_physical_read() {
    let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
    for value in [json!(LOSSY), json!(0.1), json!("-1"), json!("1.2500")] {
        let body = json!({"symbol":"BTCUSDT", "price":value}).to_string();
        let (client, seen) = stand_in::serve(move |_| (200, body.clone())).await;
        let price = SpotRestReader::new(client).ticker_price(&symbol).await;
        if value == json!("1.2500") {
            assert_eq!(price.unwrap(), dec!(1.25));
        } else {
            assert!(price.is_err());
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
