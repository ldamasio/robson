use proptest::prelude::*;
use rust_decimal_macros::dec;

use super::*;

// Fixture exchange metadata: Q = 1 BTC bought for 100,000 USDT, B̂ = 0.001
// from 10 bps, lot step 0.001, so X = 0.999.
fn input() -> EntryFeeInput {
    EntryFeeInput {
        partition: Some(LegPartition {
            gross: dec!(1.00000000),
            cost: dec!(100000.00),
            base_fee: dec!(0.001),
            base_fee_final: false,
            executable: dec!(0.999),
        }),
        consumed: Decimal::ZERO,
        closed: false,
        lot_step: dec!(0.001),
        entry: Some(EntryOrderFacts {
            phase: OrderPhase::Terminal,
            executed_cum: dec!(1.00000000),
            cumulative_quote: Some(dec!(100000.00)),
            fills_qty: dec!(1.00000000),
        }),
        totals: EntryFillTotals {
            fills: 2,
            qty: dec!(1.00000000),
            quote: dec!(100000.00),
            base_commission: dec!(0.0008),
            non_base_fills: 0,
        },
    }
}

fn with_fee(base_commission: Decimal) -> EntryFeeInput {
    let mut input = input();
    input.totals.base_commission = base_commission;
    input
}

fn finalize(base_fee: Decimal, executable: Decimal, reduced: bool, deficit: bool) -> EntryFeePlan {
    EntryFeePlan::Finalize { base_fee, executable, reduced, deficit }
}

#[test]
fn a_lower_final_fee_keeps_x_and_a_higher_one_lowers_it() {
    assert_eq!(plan_entry_fee(&input()), Ok(finalize(dec!(0.0008), dec!(0.999), false, false)));
    // floor_step(1 − 0.0015, 0.001) = 0.998 < 0.999.
    assert_eq!(
        plan_entry_fee(&with_fee(dec!(0.0015))),
        Ok(finalize(dec!(0.0015), dec!(0.998), true, false))
    );
    // X never rises: floor_step(1 − 0) = 1 > 0.999 keeps 0.999.
    assert_eq!(
        plan_entry_fee(&with_fee(Decimal::ZERO)),
        Ok(finalize(Decimal::ZERO, dec!(0.999), false, false))
    );
    let mut sold = with_fee(dec!(0.0015));
    sold.consumed = dec!(0.999);
    assert_eq!(plan_entry_fee(&sold), Ok(finalize(dec!(0.0015), dec!(0.998), true, true)));
}

#[test]
fn incomplete_or_absent_entries_are_not_ready() {
    let mut none = input();
    none.partition = None;
    assert_eq!(plan_entry_fee(&none), Ok(EntryFeePlan::NotReady(EntryFeePending::NoPartition)));
    for entry in [None, Some(OrderPhase::Live), Some(OrderPhase::Unknown)] {
        let mut waiting = input();
        waiting.entry = entry.map(|phase| EntryOrderFacts { phase, ..input().entry.unwrap() });
        assert_eq!(
            plan_entry_fee(&waiting),
            Ok(EntryFeePlan::NotReady(EntryFeePending::EntryNotTerminal))
        );
    }
    let mut partial = input();
    partial.totals.qty = dec!(0.6);
    partial.entry.as_mut().unwrap().fills_qty = dec!(0.6);
    assert_eq!(
        plan_entry_fee(&partial),
        Ok(EntryFeePlan::NotReady(EntryFeePending::FillsIncomplete))
    );
}

#[test]
fn corroboration_failures_hold_and_never_count_another_asset_as_zero() {
    let mut quote = input();
    quote.totals.quote = dec!(99999.00);
    assert_eq!(
        plan_entry_fee(&quote),
        Ok(EntryFeePlan::Held(EntryFeeHold::QuoteMismatch {
            fills: dec!(99999.00),
            cost: dec!(100000.00)
        }))
    );
    // Even a zero BNB commission holds: it is evidence the fee policy broke.
    let mut bnb = input();
    bnb.totals.non_base_fills = 1;
    assert_eq!(
        plan_entry_fee(&bnb),
        Ok(EntryFeePlan::Held(EntryFeeHold::NonBaseCommission { fills: 1 }))
    );
    assert_eq!(
        plan_entry_fee(&with_fee(dec!(1.0000001))),
        Ok(EntryFeePlan::Held(EntryFeeHold::FeeExceedsGross))
    );
}

#[test]
fn recorded_facts_must_agree_before_any_plan() {
    let mut closed = input();
    closed.closed = true;
    assert!(matches!(plan_entry_fee(&closed), Err(LedgerError::Invariant(_))));
    for (executed, quote) in [
        (dec!(0.9), Some(dec!(100000))),
        (dec!(1), Some(dec!(1))),
        (dec!(1), None),
    ] {
        let mut other = input();
        let entry = other.entry.as_mut().unwrap();
        entry.executed_cum = executed;
        entry.cumulative_quote = quote;
        assert_eq!(plan_entry_fee(&other), Err(LedgerError::IdentityConflict));
    }
    let mut stored = input();
    stored.entry.as_mut().unwrap().fills_qty = dec!(0.9);
    assert!(matches!(plan_entry_fee(&stored), Err(LedgerError::Invariant(_))));
    let mut excess = input();
    excess.totals.qty = dec!(1.1);
    excess.entry.as_mut().unwrap().fills_qty = dec!(1.1);
    assert!(matches!(plan_entry_fee(&excess), Err(LedgerError::Invariant(_))));
    let mut step = input();
    step.lot_step = Decimal::ZERO;
    assert!(matches!(plan_entry_fee(&step), Err(LedgerError::Invariant(_))));
}

#[test]
fn a_final_fee_replays_unchanged_and_is_never_rewritten() {
    let mut done = with_fee(dec!(0.0015));
    let partition = done.partition.as_mut().unwrap();
    partition.base_fee = dec!(0.0015);
    partition.base_fee_final = true;
    partition.executable = dec!(0.998);
    assert_eq!(plan_entry_fee(&done), Ok(EntryFeePlan::Unchanged));
    let mut other = done;
    other.totals.base_commission = dec!(0.0016);
    assert_eq!(plan_entry_fee(&other), Err(LedgerError::IdentityConflict));
    // A final X above floor_step(Q − B) contradicts B. A lower one is only an
    // earlier reduction (X never rises), so it replays unchanged.
    let mut higher = done;
    higher.partition.as_mut().unwrap().executable = dec!(0.999);
    assert_eq!(plan_entry_fee(&higher), Err(LedgerError::IdentityConflict));
    let mut lower = done;
    lower.partition.as_mut().unwrap().executable = dec!(0.997);
    assert_eq!(plan_entry_fee(&lower), Ok(EntryFeePlan::Unchanged));
}

fn scaled(units: i128, scale: u32) -> Decimal {
    Decimal::from_i128_with_scale(units, scale)
}

// Independent integer oracle at scale 12: Q = q·10⁻⁸, B̂ = Q·bps·10⁻⁴,
// step = 10⁻ᵉ, X = floor((Q − B̂)/step)·step, B = Σ commissions and
// X′ = min(X, floor((Q − B)/step)·step).
fn oracle(
    q: i64,
    bps: i64,
    step_exp: u32,
    fees: &[i64],
    consumed: i64,
) -> (EntryFeeInput, i128, i128) {
    let gross = i128::from(q) * 10_000;
    let step = 10i128.pow(12 - step_exp);
    let bound = i128::from(q) * i128::from(bps);
    let executable = (gross - bound) / step * step;
    let base_fee: i128 = fees.iter().map(|fee| i128::from(*fee)).sum();
    let reduced = executable.min((gross - base_fee) / step * step);
    let input = EntryFeeInput {
        partition: Some(LegPartition {
            gross: scaled(gross, 12),
            cost: dec!(1000),
            base_fee: scaled(bound, 12),
            base_fee_final: false,
            executable: scaled(executable, 12),
        }),
        consumed: scaled(i128::from(consumed), 12),
        closed: false,
        lot_step: scaled(step, 12),
        entry: Some(EntryOrderFacts {
            phase: OrderPhase::Terminal,
            executed_cum: scaled(gross, 12),
            cumulative_quote: Some(dec!(1000)),
            fills_qty: scaled(gross, 12),
        }),
        totals: EntryFillTotals {
            fills: fees.len() as i64,
            qty: scaled(gross, 12),
            quote: dec!(1000),
            base_commission: scaled(base_fee, 12),
            non_base_fills: 0,
        },
    };
    (input, base_fee, reduced)
}

proptest! {
    #[test]
    fn the_final_fee_matches_an_integer_oracle_and_x_never_rises(
        q in 1i64..=10_000_000_000,
        bps in 0i64..=100,
        step_exp in 2u32..=5,
        fees in proptest::collection::vec(0i64..=2_000_000_000, 1..5),
        consumed in 0i64..=10_000_000_000_000,
    ) {
        let (input, base_fee, reduced) = oracle(q, bps, step_exp, &fees, consumed);
        let partition = input.partition.unwrap();
        let plan = plan_entry_fee(&input).unwrap();
        if scaled(base_fee, 12) > partition.gross {
            prop_assert_eq!(plan, EntryFeePlan::Held(EntryFeeHold::FeeExceedsGross));
            return Ok(());
        }
        let executable = scaled(reduced, 12);
        prop_assert_eq!(plan, EntryFeePlan::Finalize {
            base_fee: scaled(base_fee, 12),
            executable,
            reduced: executable < partition.executable,
            deficit: input.consumed > executable,
        });
        prop_assert!(executable <= partition.executable);
        // Q = B + D + X′ with D ≥ 0.
        prop_assert!(scaled(base_fee, 12) + executable <= partition.gross);
        let mut replay = input;
        let recorded = replay.partition.as_mut().unwrap();
        recorded.base_fee = scaled(base_fee, 12);
        recorded.base_fee_final = true;
        recorded.executable = executable;
        prop_assert_eq!(plan_entry_fee(&replay), Ok(EntryFeePlan::Unchanged));
    }

    #[test]
    fn any_arrival_order_of_fills_and_terminal_reaches_the_same_final_fee(
        fills in proptest::collection::vec((1i64..=1_000_000, 0i64..=1_000), 1..8),
        rotate in 0usize..8,
        terminal_a in 0usize..=8,
        terminal_b in 0usize..=8,
    ) {
        // Fills arrive in any order and any page split, and the terminal
        // record before, between or after them; only the complete set after
        // a terminal record corroborates, and its totals do not depend on
        // the order.
        let mut order = fills.clone();
        order.rotate_left(rotate % fills.len());
        let mut decided = Vec::new();
        for (arrangement, terminal_at) in [(&fills, terminal_a), (&order, terminal_b)] {
            let terminal_at = terminal_at.min(arrangement.len());
            let events = arrangement.len() + 1;
            let (mut seen, mut fee, mut count, mut terminal) = (0, 0, 0, false);
            for event in 0..events {
                if event == terminal_at {
                    terminal = true;
                } else {
                    let (qty, commission) = arrangement[count];
                    seen += qty;
                    fee += commission;
                    count += 1;
                }
                let plan = plan_entry_fee(&arrival(&fills, seen, fee, count, terminal)).unwrap();
                if event + 1 < events {
                    let pending = if terminal {
                        EntryFeePending::FillsIncomplete
                    } else {
                        EntryFeePending::NoPartition
                    };
                    prop_assert_eq!(plan, EntryFeePlan::NotReady(pending));
                } else {
                    decided.push(plan);
                }
            }
        }
        prop_assert_eq!(decided[0], decided[1]);
        let decided_or_held =
            matches!(decided[0], EntryFeePlan::Finalize { .. } | EntryFeePlan::Held(_));
        prop_assert!(decided_or_held);
    }
}

/// One read after some fills and, maybe, the terminal record. EP-1 records
/// the partition only at the terminal record (step 0.001 = 100,000 units).
fn arrival(
    fills: &[(i64, i64)],
    seen: i64,
    fee: i64,
    count: usize,
    terminal: bool,
) -> EntryFeeInput {
    let gross: i64 = fills.iter().map(|fill| fill.0).sum();
    let mut input = input();
    input.partition = terminal.then(|| LegPartition {
        gross: scaled(i128::from(gross), 8),
        cost: dec!(100000.00),
        base_fee: Decimal::ZERO,
        base_fee_final: false,
        executable: scaled(i128::from(gross) / 100_000 * 100_000, 8),
    });
    input.entry = Some(EntryOrderFacts {
        phase: if terminal {
            OrderPhase::Terminal
        } else {
            OrderPhase::Live
        },
        executed_cum: scaled(i128::from(if terminal { gross } else { seen }), 8),
        cumulative_quote: Some(dec!(100000.00)),
        fills_qty: scaled(i128::from(seen), 8),
    });
    input.totals = EntryFillTotals {
        fills: count as i64,
        qty: scaled(i128::from(seen), 8),
        quote: dec!(100000.00),
        base_commission: scaled(i128::from(fee), 8),
        non_base_fills: 0,
    };
    input
}

#[test]
fn an_unrepresentable_reduction_refuses_and_is_never_rounded() {
    // Corroborated Q = 10 and B = 10⁻²⁸: exact Q − B needs 29 digits.
    let tiny = Decimal::from_i128_with_scale(1, 28);
    let mut input = input();
    input.lot_step = Decimal::ONE;
    let partition = input.partition.as_mut().unwrap();
    partition.gross = Decimal::TEN;
    partition.base_fee = Decimal::ZERO;
    partition.executable = dec!(9);
    let entry = input.entry.as_mut().unwrap();
    entry.executed_cum = Decimal::TEN;
    entry.fills_qty = Decimal::TEN;
    input.totals.qty = Decimal::TEN;
    input.totals.base_commission = tiny;
    assert_eq!(plan_entry_fee(&input), Err(LedgerError::Unrepresentable));
}
