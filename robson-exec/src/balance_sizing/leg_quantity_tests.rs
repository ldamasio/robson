use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::*;
use crate::balance_sizing::arithmetic::TRACE;

fn traced<T>(run: impl FnOnce() -> T) -> (T, usize, usize) {
    TRACE.with_borrow_mut(Vec::clear);
    let out = run();
    let trace = TRACE.with_borrow(|t| t.clone());
    let count = |kind: &str| trace.iter().filter(|(k, _)| *k == kind).count();
    (out, count("call"), count("export_div"))
}

#[test]
fn leg_quantity_paths_stay_within_the_planned_work() {
    let (q, step, prior) = (dec!(0.00011), dec!(0.00001), dec!(0.0001));
    let provisional = LegFeeInput::ProvisionalBound { fraction: dec!(0.001) };
    let (out, calls, exports) = traced(|| initial_executable(q, step, provisional));
    assert_eq!(out, Ok((dec!(0.00000011), prior)));
    assert_eq!((calls, exports <= 2 * 29), (5, true));
    let commission = LegFeeInput::FinalCommission(dec!(0.00000011));
    let (out, calls, _) = traced(|| initial_executable(q, step, commission));
    assert_eq!(out, Ok((dec!(0.00000011), prior)));
    assert_eq!(calls, 3);
    let (out, calls, exports) = traced(|| reduced_executable(q, dec!(0.0000101), prior, step));
    assert_eq!(out, Ok(dec!(0.00009)));
    assert_eq!((calls, exports <= 29), (6, true));
    let (out, calls, exports) = traced(|| reduced_executable(q, Decimal::ZERO, prior, step));
    assert_eq!(out, Ok(prior));
    assert_eq!((calls, exports), (5, 0));
}

#[test]
fn unselected_unrepresentable_floor_never_refuses_reduction() {
    // MAX ≡ 8 (mod 11), so floor_step(MAX, 11e-28) needs scale 28 and more
    // than 96 coefficient bits.
    let step = Decimal::from_i128_with_scale(11, 28);
    let zero = LegFeeInput::FinalCommission(Decimal::ZERO);
    assert_eq!(initial_executable(Decimal::MAX, step, zero), Err(FactError::Unrepresentable));
    for prior in [Decimal::ZERO, step] {
        assert_eq!(reduced_executable(Decimal::MAX, Decimal::ZERO, prior, step), Ok(prior));
    }
}
