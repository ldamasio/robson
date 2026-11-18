use chrono::{NaiveDate, TimeZone, Utc};
use robson_domain::{Price, Symbol};
use rust_decimal::Decimal as D;
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::*;
use crate::{
    balance_costs::{sizing_terms_with_provenance, CostIdentity, CostPolicy},
    balance_sizing::{calculate_balance_candidate, test_support::fixture, SizingInputs},
};

fn input(quote: &str, fee: &str, gap: &str, hour: u32, note: &str) -> SizingInputs {
    let mut i = fixture();
    let id = &i.binding.account;
    i.binding.account = CostIdentity::new(
        id.tenant(),
        id.venue().clone(),
        id.scope().into(),
        id.uid(),
        Symbol::new("SOL".into(), quote.into()).unwrap(),
    )
    .unwrap();
    for o in [
        &mut i.basis.observation,
        &mut i.cash.observation,
        &mut i.price.observation,
        &mut i.chart.observation,
        &mut i.rules.observation,
    ] {
        o.binding = i.binding.clone();
    }
    i.filter_price.as_mut().unwrap().observation.binding = i.binding.clone();
    i.cash.quote_asset = quote.into();
    i.basis.amount = dec!(100000);
    i.cash.free = dec!(105);
    i.rules.lot.step = dec!(0.01); // MARKET_LOT_SIZE remains the coarser0.1.
    i.policy = CostPolicy::parse(gap, Some("200"), None).unwrap();
    i.terms = sizing_terms_with_provenance(
        &i.binding.account,
        fee,
        gap,
        Utc.with_ymd_and_hms(2026, 11, 14, hour, 0, 0).unwrap(),
        note,
    );
    i
}
fn map(c: &SizingCalculation) -> AdmissionRow {
    candidate_admission(
        c,
        Uuid::from_u128(42),
        c.inputs().binding.account.symbol(),
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
    )
    .unwrap()
}

#[test]
fn all_eighteen_fields_copy_original_values_and_selected_grid() {
    let i = input("USDC", "10", "10", 12, "dated handoff alpha");
    let original_hash: String = i.terms.hash().iter().map(|b| format!("{b:02x}")).collect();
    let c = calculate_balance_candidate(&i).unwrap();
    assert_eq!(c.selected_lot_step(), dec!(0.1));
    assert_ne!(c.selected_lot_step(), i.rules.lot.step);
    assert_eq!(c.attempts(), 2);
    assert_eq!(map(&c), AdmissionRow {
        position_id: Uuid::from_u128(42),
        symbol: "SOLUSDC".into(),
        decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        taker_bps: dec!(10),
        gap_bps: dec!(10),
        notional_margin_bps: dec!(200),
        min_notional: dec!(1),
        lot_step: dec!(0.1),
        fee_asset_policy: "base".into(),
        envelope_source: "ROBSON_COSTS_OVERRIDE:v1:dated handoff alpha".into(),
        envelope_as_of: Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap(),
        envelope_hash: original_hash,
        planned_qty: dec!(0.9),
        requested_qty: dec!(1),
        reference_price: dec!(100),
        remainder_reserve: dec!(0.1),
        admission_charge: dec!(60.16996),
        initial_trigger: dec!(50),
    });
    assert_ne!(c.amounts().adverse_fill, dec!(50));
    let row = map(&c);
    assert_eq!(row.envelope_hash.len(), 64);
    assert!(row
        .envelope_hash
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_ne!(c.inputs().terms.hash(), c.inputs().terms.evidence().hash());
}

#[test]
fn position_symbol_and_decided_month_are_checked_without_defaults() {
    let c = calculate_balance_candidate(&input("USDC", "10", "10", 12, "context")).unwrap();
    let symbol = c.inputs().binding.account.symbol();
    let month = c.inputs().basis.month;
    assert_eq!(candidate_admission(&c, Uuid::nil(), symbol, month), Err(HandoffError::Position));
    for other in [
        Symbol::new("SOL".into(), "USDT".into()).unwrap(),
        Symbol::new("ETH".into(), "USDC".into()).unwrap(),
    ] {
        assert_eq!(
            candidate_admission(&c, Uuid::from_u128(42), &other, month),
            Err(HandoffError::Symbol)
        );
    }
    for wrong in [
        NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
    ] {
        assert_eq!(
            candidate_admission(&c, Uuid::from_u128(42), symbol, wrong),
            Err(HandoffError::Month)
        );
    }
}

#[test]
fn two_priced_plans_keep_their_own_provenance_after_unrelated_changes() {
    let mut first = input("USDC", "10", "10", 12, "alpha");
    let a = calculate_balance_candidate(&first).unwrap();
    let row_a = map(&a);
    let mut second = input("USDT", "20", "20", 13, "beta");
    let b = calculate_balance_candidate(&second).unwrap();
    let row_b = map(&b);
    assert_eq!(row_b.symbol, "SOLUSDT");
    assert_eq!((row_b.taker_bps, row_b.gap_bps), (dec!(20), dec!(20)));
    assert_eq!(row_b.envelope_source, "ROBSON_COSTS_OVERRIDE:v1:beta");
    assert_eq!(row_b.envelope_as_of, Utc.with_ymd_and_hms(2026, 11, 14, 13, 0, 0).unwrap());
    assert_ne!(row_a.envelope_hash, row_b.envelope_hash);
    assert_ne!(row_a.admission_charge, row_b.admission_charge);
    first.terms = second.terms.clone();
    first.rules.lot.step = dec!(1);
    first.policy = CostPolicy::parse("99", Some("900"), None).unwrap();
    second.cash.free = D::ZERO;
    second.terms = sizing_terms_with_provenance(
        &second.binding.account,
        "90",
        "99",
        Utc.with_ymd_and_hms(2026, 11, 15, 0, 0, 0).unwrap(),
        "later refresh",
    );
    assert_eq!(map(&a), row_a);
    assert_eq!(map(&b), row_b);
    // AdmissionRow is existing mutable data, not a certificate. Mutating a
    // copy cannot change the immutable source or a newly copied payload.
    let mut mutable_row = row_a.clone();
    mutable_row.requested_qty = D::ZERO;
    assert_eq!(map(&a), row_a);
    assert_ne!(map(&a), mutable_row);
}

#[test]
fn high_scale_fields_survive_mapping_without_rounding() {
    let mut i = fixture();
    // Labeled analyzer observation: second15m support5e-27, reference1e-26.
    i.price.value = dec!(0.00000000000000000000000001);
    i.chart.support = dec!(0.000000000000000000000000005);
    i.chart.plan.basis = Price::new(i.chart.support).unwrap();
    i.chart.plan.trigger = i.chart.plan.basis;
    i.chart.plan.cap_basis_distance = Some(dec!(0.000000000000000000000000005));
    i.chart.plan.executable_span = i.chart.plan.cap_basis_distance;
    i.rules.price.tick = dec!(0.0000000000000000000000000001);
    i.chart.plan.tick_size = Some(i.rules.price.tick);
    i.rules.price.max = D::ZERO;
    i.basis.amount = dec!(1000);
    i.cash.free = dec!(1000);
    i.rules.lot.step = D::ONE;
    i.rules.lot.min = D::ONE;
    i.rules.lot.max = D::MAX;
    i.rules.market_lot = None;
    i.rules.notional.min = dec!(0.0000000000000000000000000001);
    i.rules.notional.max = D::MAX;
    i.rules.notional.apply_min = false;
    i.rules.notional.apply_max = false;
    i.filter_price = None;
    let c = calculate_balance_candidate(&i).unwrap();
    let row = map(&c);
    assert_eq!(row.reference_price, dec!(0.00000000000000000000000001));
    assert_eq!(row.initial_trigger, dec!(0.000000000000000000000000005));
    assert_eq!(row.min_notional, dec!(0.0000000000000000000000000001));
    assert_eq!(row.planned_qty, dec!(1999999999999999999999999999));
    assert_eq!(row.requested_qty, row.planned_qty);
    assert_eq!(row.admission_charge, dec!(10));
    assert_eq!(row.lot_step, D::ONE);
    assert_eq!(row.remainder_reserve, D::ONE);
}
