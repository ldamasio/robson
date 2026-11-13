//! The mirrored evidence rules on their own (`spot_evidence`), over inputs
//! a consistent book cannot produce: short, conflicting or invalid trades
//! and fills.

use chrono::DateTime;
use robson_domain::OrderSide;
use robson_exec::{
    ports::{FeeEvidence, FeeGap, TradeFill},
    venue::{OrderPhase, VenueOrderRecord},
};
use rust_decimal::Decimal;

use crate::spot_evidence::{market_evidence, terminal_result, MarketFill};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// A terminal order that executed 1 for a quote of 100.
fn record() -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: "7".into(),
        client_order_id: "c".into(),
        orig_client_order_id: None,
        symbol: "BTCUSDT".into(),
        side: OrderSide::Sell,
        order_type: "STOP_LOSS".into(),
        protection_kind: None,
        status: "FILLED".into(),
        phase: OrderPhase::Terminal,
        orig_qty: d("1"),
        executed_qty: d("1"),
        cumulative_quote_qty: Some(d("100")),
        stop_price: Some(d("90")),
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: Some(1),
        update_time_ms: Some(1),
        working_time_ms: None,
    }
}

fn trade(id: &str, qty: &str, asset: &str) -> TradeFill {
    TradeFill {
        trade_id: id.into(),
        price: d("100"),
        qty: d(qty),
        quote_qty: d("100") * d(qty),
        commission: d("0.1"),
        commission_asset: asset.into(),
        time: DateTime::from_timestamp_millis(1_767_225_600_000),
    }
}

fn fill(id: u64, qty: &str, asset: &str) -> MarketFill {
    MarketFill {
        price: d("100"),
        qty: d(qty),
        commission: d("0.1"),
        commission_asset: asset.into(),
        trade_id: id,
    }
}

#[test]
fn trades_give_fills_only_when_valid_distinct_and_exact() {
    let whole = terminal_result(
        &record(),
        vec![trade("1", "0.4", "USDT"), trade("2", "0.6", "USDT")],
        None,
    );
    assert_eq!(whole.unwrap().fee_evidence(), FeeEvidence::Total);
    let duplicate = vec![
        trade("1", "0.4", "USDT"),
        trade("1", "0.4", "USDT"),
        trade("2", "0.6", "USDT"),
    ];
    assert!(
        terminal_result(&record(), duplicate, None).is_some(),
        "an identical duplicate counts once"
    );
    for (case, trades) in [
        ("short", vec![trade("1", "0.4", "USDT")]),
        ("over", vec![trade("1", "0.4", "USDT"), trade("2", "0.7", "USDT")]),
        ("conflicting id", vec![trade("1", "0.4", "USDT"), trade("1", "0.6", "USDT")]),
        ("no asset", vec![trade("1", "0.4", ""), trade("2", "0.6", "USDT")]),
        ("no time", vec![TradeFill { time: None, ..trade("1", "1", "USDT") }]),
    ] {
        assert!(terminal_result(&record(), trades, None).is_none(), "{case}");
    }
    // Quotes that agree while the quantity does not: only the sum refuses.
    let mut dear = trade("1", "0.5", "USDT");
    (dear.price, dear.quote_qty) = (d("200"), d("100"));
    assert!(
        terminal_result(&record(), vec![dear], None).is_none(),
        "half, at twice the price"
    );
    let mut unquoted = record();
    unquoted.cumulative_quote_qty = None;
    assert!(
        terminal_result(&unquoted, vec![trade("1", "0.4", "USDT")], None).is_none(),
        "no record quote"
    );
    let mut negative = trade("1", "1", "USDT");
    negative.quote_qty = d("-1");
    assert!(terminal_result(&record(), vec![negative], None).is_none(), "negative quote");
    let mut off = record();
    off.cumulative_quote_qty = Some(d("100.01"));
    assert!(
        terminal_result(&off, vec![trade("1", "1", "USDT")], None).is_none(),
        "R3, no tolerance"
    );
    assert!(
        terminal_result(&off, vec![trade("1", "1", "USDT")], Some(2)).is_some(),
        "R3, 0.01 tolerated"
    );
}

#[test]
fn market_fills_are_complete_incomplete_or_unusable() {
    let complete =
        market_evidence(Some(d("100")), Some(&[fill(1, "1", "BTC")]), d("1"), None).unwrap();
    assert_eq!((complete.fee_evidence, complete.fills.len()), (FeeEvidence::Total, 1));
    let short =
        market_evidence(Some(d("100")), Some(&[fill(1, "0.5", "BTC")]), d("1"), None).unwrap();
    assert_eq!(
        (short.fee_evidence, short.fills.len()),
        (FeeEvidence::Unavailable(FeeGap::Incomplete), 1)
    );
    for (case, fills, gap) in [
        ("over", vec![fill(1, "1.5", "BTC")], FeeGap::Conflicting),
        ("invalid", vec![fill(1, "1", "")], FeeGap::Conflicting),
        (
            "conflicting id",
            vec![fill(1, "0.5", "BTC"), fill(1, "0.4", "BTC")],
            FeeGap::Conflicting,
        ),
        ("none", vec![], FeeGap::NoFills),
    ] {
        let evidence = market_evidence(Some(d("100")), Some(&fills), d("1"), None).unwrap();
        assert_eq!(
            (evidence.fee_evidence, evidence.fills.len()),
            (FeeEvidence::Unavailable(gap), 0),
            "{case}"
        );
        assert_eq!(evidence.price, d("100"), "{case}: the record's price stands");
    }
    let mixed = [fill(1, "0.5", "BTC"), fill(2, "0.5", "BNB")];
    let evidence = market_evidence(Some(d("100")), Some(&mixed), d("1"), None).unwrap();
    assert_eq!(
        (evidence.fee_evidence, evidence.fills.len()),
        (FeeEvidence::Unavailable(FeeGap::MixedAssets), 2)
    );
    assert!(
        market_evidence(Some(Decimal::ZERO), Some(&[fill(1, "1", "BTC")]), d("1"), None).is_err()
    );
}
