use proptest::prelude::*;
use robson_domain::OrderSide;
use rust_decimal_macros::dec;

use super::*;
use crate::{
    venue::{OrderPhase, VenueId},
    venue_ledger::{LegOrderKind, OrderOwner, SeenAt},
};

fn at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}
fn order() -> StoredOrder {
    StoredOrder {
        key: OrderKey {
            venue: VenueId::new(VenueId::BINANCE_SPOT),
            account_scope: "scope".into(),
            symbol: "BTCUSDT".into(),
            exchange_order_id: "1".parse().unwrap(),
        },
        owner: OrderOwner::Leg {
            intent_id: Uuid::nil(),
            position_id: Uuid::nil(),
            kind: LegOrderKind::Sell,
        },
        client_order_id: "client".into(),
        last_client_order_id: "client".into(),
        side: OrderSide::Sell,
        order_type: "MARKET".into(),
        orig_qty: dec!(10000),
        stop_price: None,
        status: "NEW".into(),
        phase: OrderPhase::Live,
        executed_cum: Decimal::ZERO,
        cumulative_quote: None,
        fills_qty: Decimal::ZERO,
        trades_cursor: 0,
        last_checked_at: at(),
        live_seen: SeenAt { at: at(), clock: None },
        terminal_seen: None,
        expiry_reason: None,
        venue_time_ms: None,
        venue_update_time_ms: None,
        venue_transact_time_ms: None,
        venue_working_time_ms: None,
    }
}
fn fill(id: &str, qty: Decimal) -> TradeFill {
    TradeFill {
        trade_id: id.into(),
        price: dec!(10),
        qty,
        quote_qty: qty * dec!(10),
        commission: dec!(0.01),
        commission_asset: "USDT".into(),
        time: Some(at()),
    }
}
fn page(fill: TradeFill) -> TradePage {
    TradePage { trades: vec![fill], next_from_id: None }
}
fn plan(
    order: &StoredOrder,
    page: &TradePage,
    existing: &[StoredTrade],
) -> Result<TradePlan, LedgerError> {
    plan_trades(order, Uuid::nil(), page, existing, at())
}

#[test]
fn invalid_trade_evidence_is_refused_before_any_plan() {
    let valid = fill("1", dec!(1));
    let mut cases = Vec::new();
    let mut item = valid.clone();
    item.time = None;
    cases.push(item);
    for qty in [Decimal::ZERO, dec!(-1)] {
        let mut item = valid.clone();
        item.qty = qty;
        cases.push(item);
    }
    for price in [Decimal::ZERO, dec!(-1)] {
        let mut item = valid.clone();
        item.price = price;
        cases.push(item);
    }
    for id in ["-1", "+1", "1.0", "", "18446744073709551616"] {
        let mut item = valid.clone();
        item.trade_id = id.into();
        cases.push(item);
    }
    let mut item = valid.clone();
    item.quote_qty = dec!(-1);
    cases.push(item);
    let mut item = valid.clone();
    item.commission = dec!(-1);
    cases.push(item);
    let mut item = valid;
    item.commission_asset.clear();
    cases.push(item);
    for item in cases {
        assert!(matches!(plan(&order(), &page(item), &[]), Err(LedgerError::InvalidEvidence(_))));
    }
}

#[test]
fn duplicate_ids_are_checked_after_canonicalization() {
    let p = TradePage {
        trades: vec![fill("001", dec!(1)), fill("1", dec!(1))],
        next_from_id: None,
    };
    assert!(matches!(plan(&order(), &p, &[]), Err(LedgerError::InvalidEvidence(_))));
    let first = plan(&order(), &page(fill("001", dec!(1))), &[]).unwrap();
    assert_eq!(first.new_fills[0].trade_id.to_string(), "1");
    let replay = plan(&first.order, &page(fill("1", dec!(1))), &first.new_fills).unwrap();
    assert!(!replay.changed);
}

#[test]
fn trade_identity_includes_the_order_and_tenant_but_not_local_recording_time() {
    let p = page(fill("1", dec!(1)));
    let first = plan(&order(), &p, &[]).unwrap();
    let later = at() + chrono::Duration::days(1);
    let replay = plan_trades(&first.order, Uuid::nil(), &p, &first.new_fills, later);
    assert!(replay.is_ok(), "local recording time changed economic identity: {replay:?}");
    let replay = replay.unwrap();
    assert!(!replay.changed);
    assert!(replay.new_fills.is_empty());
    assert_eq!(replay.order, first.order);
    assert_eq!(replay.consumed_delta, Decimal::ZERO);
    for other in [false, true] {
        let mut existing = first.new_fills.clone();
        if other {
            existing[0].tenant_id = Uuid::from_u128(2);
        } else {
            existing[0].order.exchange_order_id = "2".parse().unwrap();
        }
        assert_eq!(plan(&first.order, &p, &existing), Err(LedgerError::TradeConflict));
    }
    assert_eq!(first.new_fills[0].recorded_at, at());
}

#[test]
fn every_changed_economic_field_conflicts() {
    let original = fill("1", dec!(1));
    let p = page(original.clone());
    let first = plan(&order(), &p, &[]).unwrap();
    let mut variants = Vec::new();
    let mut f = original.clone();
    f.price += dec!(1);
    variants.push(f);
    let mut f = original.clone();
    f.qty += dec!(1);
    variants.push(f);
    let mut f = original.clone();
    f.quote_qty += dec!(1);
    variants.push(f);
    let mut f = original.clone();
    f.commission += dec!(1);
    variants.push(f);
    let mut f = original.clone();
    f.commission_asset = "BTC".into();
    variants.push(f);
    let mut f = original;
    f.time = Some(at() + chrono::Duration::seconds(1));
    variants.push(f);
    for f in variants {
        assert_eq!(plan(&first.order, &page(f), &first.new_fills), Err(LedgerError::TradeConflict));
    }
}

#[test]
fn live_trades_raise_execution_and_only_exit_ownership_consumes_inventory() {
    for owner in [
        OrderOwner::Unauthored,
        OrderOwner::Discovered,
        OrderOwner::Leg {
            intent_id: Uuid::nil(),
            position_id: Uuid::nil(),
            kind: LegOrderKind::Entry,
        },
        OrderOwner::Leg {
            intent_id: Uuid::nil(),
            position_id: Uuid::nil(),
            kind: LegOrderKind::Stop,
        },
        order().owner,
    ] {
        let mut original = order();
        original.owner = owner;
        original.executed_cum = dec!(1);
        original.fills_qty = dec!(0.5);
        let result = plan(&original, &page(fill("1", dec!(2))), &[]).unwrap();
        assert_eq!(result.order.executed_cum, dec!(2.5));
        assert_eq!(result.order.fills_qty, dec!(2.5));
        let exit = matches!(original.owner, OrderOwner::Leg {
            kind: LegOrderKind::Stop | LegOrderKind::Sell,
            ..
        });
        assert_eq!(result.consumed_delta, if exit { dec!(1.5) } else { Decimal::ZERO });
    }
}

#[test]
fn terminal_execution_and_original_quantity_bound_the_fills() {
    let mut original = order();
    original.phase = OrderPhase::Terminal;
    original.executed_cum = dec!(1);
    original.terminal_seen = Some(original.live_seen.clone());
    let complete = plan(&original, &page(fill("1", dec!(1))), &[]).unwrap();
    assert!(complete.order.fills_complete());
    assert_eq!(complete.consumed_delta, Decimal::ZERO);
    assert_eq!(plan(&original, &page(fill("1", dec!(2))), &[]), Err(LedgerError::TradeConflict));
    original.phase = OrderPhase::Unknown;
    original.orig_qty = dec!(1);
    assert_eq!(plan(&original, &page(fill("1", dec!(2))), &[]), Err(LedgerError::TradeConflict));
}

#[test]
fn cursor_never_decreases_and_overflow_is_invalid_evidence() {
    let mut original = order();
    original.trades_cursor = 20;
    let lower = plan(&original, &TradePage { trades: vec![], next_from_id: Some(3) }, &[]).unwrap();
    assert!(!lower.changed);
    assert_eq!(lower.order, original);
    let advance =
        plan(&original, &TradePage { trades: vec![], next_from_id: Some(21) }, &[]).unwrap();
    assert!(advance.changed);
    assert_eq!(advance.order.trades_cursor, 21);
    for p in [
        page(fill(&u64::MAX.to_string(), dec!(1))),
        page(fill(&i64::MAX.to_string(), dec!(1))),
        TradePage {
            trades: vec![],
            next_from_id: Some(i64::MAX as u64 + 1),
        },
    ] {
        assert!(matches!(plan(&original, &p, &[]), Err(LedgerError::InvalidEvidence(_))));
    }
    assert_eq!(
        plan(
            &original,
            &TradePage {
                trades: vec![fill("1", dec!(1))],
                next_from_id: Some(30)
            },
            &[]
        )
        .unwrap()
        .order
        .trades_cursor,
        30
    );
}

#[test]
fn sums_refuse_rounding_and_overflow() {
    let mut original = order();
    original.orig_qty = Decimal::MAX;
    original.fills_qty = Decimal::MAX;
    original.executed_cum = Decimal::MAX;
    for qty in [dec!(0.0000000000000000000000000001), Decimal::ONE] {
        assert_eq!(plan(&original, &page(fill("1", qty)), &[]), Err(LedgerError::Unrepresentable));
    }
    let p = TradePage {
        trades: vec![
            TradeFill {
                price: Decimal::ONE,
                qty: Decimal::MAX,
                quote_qty: Decimal::ZERO,
                ..fill("1", Decimal::ONE)
            },
            fill("2", Decimal::ONE),
        ],
        next_from_id: None,
    };
    assert_eq!(plan(&order(), &p, &[]), Err(LedgerError::Unrepresentable));
}

#[test]
fn pages_are_bounded_and_empty_pages_are_noops() {
    let p = TradePage {
        trades: (0..1001).map(|id| fill(&id.to_string(), Decimal::ONE)).collect(),
        next_from_id: None,
    };
    assert!(matches!(plan(&order(), &p, &[]), Err(LedgerError::InvalidEvidence(_))));
    let empty = plan(&order(), &TradePage { trades: vec![], next_from_id: None }, &[]).unwrap();
    assert!(!empty.changed);
    assert_eq!(empty.order, order());
}

proptest! {
    #[test]
    fn shuffled_replayed_pages_conserve_fills_execution_and_contribution(qtys in prop::collection::vec(1u32..100,1..40), reverse in any::<bool>(), scale in 0..=6u32) {
        let mut original=order(); let mut existing=Vec::new(); let mut expected=Decimal::ZERO;
        let mut numbered:Vec<_>=qtys.into_iter().enumerate().collect(); if reverse { numbered.reverse(); }
        for (id,raw) in numbered {
            let qty=Decimal::from_i128_with_scale(i128::from(raw),scale); expected=exact_add(expected,qty).unwrap();
            let p=page(fill(&(id+1).to_string(),qty));
            let planned=plan(&original,&p,&existing.iter().filter(|f: &&StoredTrade|f.trade_id==(id+1).to_string().parse().unwrap()).cloned().collect::<Vec<_>>()).unwrap();
            prop_assert_eq!(planned.order.fills_qty,expected); prop_assert_eq!(planned.order.executed_cum,expected);
            prop_assert_eq!(planned.consumed_delta,qty);
            existing.extend(planned.new_fills.clone()); original=planned.order;
            let replay=plan_trades(&original,Uuid::nil(),&p,&existing.iter().filter(|f|f.trade_id==(id+1).to_string().parse().unwrap()).cloned().collect::<Vec<_>>(),at()+chrono::Duration::seconds(100)).unwrap();
            prop_assert!(!replay.changed); prop_assert_eq!(&replay.order,&original); prop_assert_eq!(replay.consumed_delta,Decimal::ZERO);
        }
    }
}
