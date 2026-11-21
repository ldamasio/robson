use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::*;
use crate::balance_liability::Remainder;

fn provisional(gross: Decimal, fraction: Decimal, lot_step: Decimal) -> InitialQuantityInput {
    InitialQuantityInput {
        gross,
        lot_step,
        consumed: Decimal::ZERO,
        fee: LegFeeInput::ProvisionalBound { fraction },
    }
}

fn final_fee(base_fee: Decimal, consumed: Decimal) -> FinalFeeInput {
    FinalFeeInput {
        gross: dec!(0.00011),
        base_fee,
        prior_executable: dec!(0.0001),
        lot_step: dec!(0.00001),
        consumed,
    }
}

fn assert_conserved(input_gross: Decimal, out: &LegQuantity) {
    let facts = out.facts();
    assert_eq!(out.base_fee() + facts.dust() + out.executable(), input_gross);
    assert_eq!(facts.net(), input_gross - out.base_fee());
}

#[test]
fn provisional_initial_quantity_matches_recorded_partition() {
    let input = InitialQuantityInput {
        consumed: dec!(0.00003),
        ..provisional(dec!(0.00011), dec!(0.001), dec!(0.00001))
    };
    let out = initial_quantity(input).unwrap();
    assert_eq!(out.base_fee(), dec!(0.00000011));
    assert_eq!(out.executable(), dec!(0.0001));
    assert_eq!(out.facts().dust(), dec!(0.00000989));
    assert_eq!(out.facts().remainder(), Remainder::Quantity(dec!(0.00007)));
    assert_eq!(out.facts().fee_evidence(), FeeEvidence::ProvisionalBound);
    let fee = LegFeeInput::FinalCommission(dec!(0.00000011));
    let commission = initial_quantity(InitialQuantityInput { fee, ..input }).unwrap();
    assert_eq!(commission.executable(), out.executable());
    assert_eq!(commission.facts().dust(), out.facts().dust());
    assert_eq!(commission.facts().fee_evidence(), FeeEvidence::FinalCommission);
    assert_conserved(input.gross, &out);
}

#[test]
fn exact_provisional_product_on_lot_boundary_is_not_rounded() {
    let out = initial_quantity(provisional(dec!(0.002), dec!(0.0005), dec!(0.000001))).unwrap();
    assert_eq!((out.base_fee(), out.executable()), (dec!(0.000001), dec!(0.001999)));
    assert_eq!(out.facts().dust(), Decimal::ZERO);
    // B̂ needs all 28 places; any rounding of B̂ moves X off this boundary.
    let ulp = Decimal::from_i128_with_scale(1, 28);
    let gross = dec!(1.00000000000001);
    let out = initial_quantity(provisional(gross, dec!(0.00000000000001), ulp)).unwrap();
    assert_eq!(out.base_fee(), Decimal::from_i128_with_scale(100_000_000_000_001, 28));
    assert_eq!(out.executable(), Decimal::ONE - ulp);
    assert_eq!(out.facts().dust(), Decimal::ZERO);
    assert_conserved(gross, &out);
}

#[test]
fn venue_scaled_gross_with_zero_dust_is_exact() {
    // Q as the venue prints it; B̂ and X export at fewer places, so zero dust
    // carries Q's scale while B̂ keeps its own.
    let gross = dec!(0.30000000);
    let out = initial_quantity(provisional(gross, dec!(0.0010), dec!(0.00001))).unwrap();
    assert_eq!((out.base_fee(), out.executable()), (dec!(0.0003), dec!(0.2997)));
    assert_eq!(out.facts().dust(), Decimal::ZERO);
    assert_conserved(gross, &out);
}

#[test]
fn unrepresentable_positive_product_is_refused_not_rounded() {
    // Q*f_L needs scale 30; native Decimal multiplication would round it.
    let gross = Decimal::from_i128_with_scale(100_000_000_000_000_000_001, 20);
    let input = provisional(gross, dec!(0.0000000001), Decimal::ONE);
    assert_eq!(initial_quantity(input), Err(FactError::Unrepresentable));
    // Native maximum coefficient: MAX/2 needs 97 bits.
    let half = provisional(Decimal::MAX, dec!(0.5), Decimal::ONE);
    assert_eq!(initial_quantity(half), Err(FactError::Unrepresentable));
    let all = initial_quantity(provisional(Decimal::MAX, Decimal::ONE, Decimal::ONE)).unwrap();
    assert_eq!((all.base_fee(), all.executable()), (Decimal::MAX, Decimal::ZERO));
    let fee = LegFeeInput::FinalCommission(Decimal::ZERO);
    let max = initial_quantity(InitialQuantityInput { fee, ..half }).unwrap();
    assert_eq!(max.executable(), Decimal::MAX);
    assert_eq!(max.facts().dust(), Decimal::ZERO);
}

#[test]
fn invalid_quantity_inputs_and_zero_lot_are_refused() {
    let input = provisional(dec!(0.00011), dec!(0.001), dec!(0.00001));
    let fee = |fraction| LegFeeInput::ProvisionalBound { fraction };
    for bad in [
        InitialQuantityInput { gross: dec!(-0.00011), ..input },
        InitialQuantityInput { lot_step: Decimal::ZERO, ..input },
        InitialQuantityInput { lot_step: dec!(-0.00001), ..input },
        InitialQuantityInput { consumed: dec!(-1), ..input },
        InitialQuantityInput { fee: fee(dec!(-0.001)), ..input },
        InitialQuantityInput { fee: fee(dec!(1.0001)), ..input },
        InitialQuantityInput {
            fee: LegFeeInput::FinalCommission(dec!(-1)),
            ..input
        },
        InitialQuantityInput {
            fee: LegFeeInput::FinalCommission(dec!(0.00012)),
            ..input
        },
    ] {
        assert_eq!(initial_quantity(bad), Err(FactError::InvalidQuantity), "{bad:?}");
    }
    let input = final_fee(dec!(0.00000011), Decimal::ZERO);
    for bad in [
        FinalFeeInput {
            gross: dec!(-0.00011),
            base_fee: Decimal::ZERO,
            ..input
        },
        FinalFeeInput { base_fee: dec!(-1), ..input },
        FinalFeeInput { base_fee: dec!(0.00012), ..input },
        FinalFeeInput {
            prior_executable: dec!(-0.00001),
            ..input
        },
        FinalFeeInput { prior_executable: dec!(0.00012), ..input },
        FinalFeeInput {
            prior_executable: dec!(0.000105),
            ..input
        },
        FinalFeeInput { lot_step: Decimal::ZERO, ..input },
        FinalFeeInput { lot_step: dec!(-0.00001), ..input },
        FinalFeeInput { consumed: dec!(-1), ..input },
    ] {
        assert_eq!(final_fee_quantity(bad), Err(FactError::InvalidQuantity), "{bad:?}");
    }
}

#[test]
fn zero_gross_and_below_step_quantities_are_valid_zero_executable() {
    let zero = initial_quantity(provisional(Decimal::ZERO, dec!(0.001), dec!(0.00001))).unwrap();
    assert_eq!((zero.base_fee(), zero.executable()), (Decimal::ZERO, Decimal::ZERO));
    assert_eq!(zero.facts().remainder(), Remainder::Quantity(Decimal::ZERO));
    let below = initial_quantity(provisional(dec!(0.000009), Decimal::ZERO, dec!(0.00001)));
    let below = below.unwrap();
    assert_eq!(below.executable(), Decimal::ZERO);
    assert_eq!(below.facts().dust(), dec!(0.000009));
    let smallest = initial_quantity(InitialQuantityInput {
        fee: LegFeeInput::FinalCommission(Decimal::ZERO),
        ..provisional(Decimal::from_i128_with_scale(1, 28), Decimal::ZERO, Decimal::ONE)
    });
    assert_eq!(smallest.unwrap().facts().dust(), Decimal::from_i128_with_scale(1, 28));
}

#[test]
fn final_fee_decrease_keeps_prior_executable_and_adds_dust() {
    // floor_step(Q−B)=0.00011 is greater than prior X; X must not rise.
    let out = final_fee_quantity(final_fee(Decimal::ZERO, dec!(0.00003))).unwrap();
    assert_eq!(out.executable(), dec!(0.0001));
    assert_eq!(out.facts().dust(), dec!(0.00001));
    assert_eq!(out.facts().remainder(), Remainder::Quantity(dec!(0.00007)));
    assert_eq!(out.facts().fee_evidence(), FeeEvidence::FinalCommission);
    let same = final_fee_quantity(final_fee(dec!(0.00000011), dec!(0.00003))).unwrap();
    assert_eq!(same.executable(), dec!(0.0001));
    assert_eq!(same.facts().dust(), dec!(0.00000989));
    assert_conserved(dec!(0.00011), &out);
}

#[test]
fn final_fee_increase_reduces_executable_to_the_lot_floor() {
    let out = final_fee_quantity(final_fee(dec!(0.0000101), dec!(0.00003))).unwrap();
    assert_eq!(out.executable(), dec!(0.00009));
    assert_eq!(out.facts().dust(), dec!(0.0000099));
    assert_eq!(out.facts().remainder(), Remainder::Quantity(dec!(0.00006)));
    assert_conserved(dec!(0.00011), &out);
}

#[test]
fn deficit_stays_explicit_after_final_fee_reduction() {
    for consumed in [dec!(0.0001), dec!(0.00011)] {
        let out = final_fee_quantity(final_fee(dec!(0.0000101), consumed)).unwrap();
        assert_eq!(out.executable(), dec!(0.00009));
        let excess = consumed - dec!(0.00009);
        assert_eq!(out.facts().remainder(), Remainder::Deficit { excess });
    }
    let input = InitialQuantityInput {
        consumed: dec!(0.00011),
        ..provisional(dec!(0.00011), dec!(0.001), dec!(0.00001))
    };
    let initial = initial_quantity(input).unwrap();
    assert_eq!(initial.facts().remainder(), Remainder::Deficit { excess: dec!(0.00001) });
}

proptest! {
    // Independent scaled-i128 model at scale 12: Q has <=8 places, f_L 4 and
    // the lot step <=8, so B̂, floors and every component are exact integers.
    #[test]
    fn initial_quantity_matches_scaled_integer_oracle(
        q in 0i64..1_000_000_000_000, q_scale in 0u32..=8,
        fraction in 0i64..=10_000, step in 1i64..1_000_000, step_scale in 0u32..=8,
        final_fee in any::<bool>(), fee_raw in any::<u64>(), consumed_raw in any::<u64>(),
    ) {
        let units = |n: i64, scale: u32| i128::from(n) * 10i128.pow(12 - scale);
        let decimal = |n: i128| Decimal::from_i128_with_scale(n, 12);
        let (q12, step12) = (units(q, q_scale), units(step, step_scale));
        let (fee, b12) = if final_fee {
            let b12 = i128::from(fee_raw) % (q12 + 1);
            (LegFeeInput::FinalCommission(decimal(b12)), b12)
        } else {
            let fraction_decimal = Decimal::new(fraction, 4);
            let b12 = units(q, q_scale) * i128::from(fraction) / 10_000;
            prop_assert_eq!(b12 * 10_000, units(q, q_scale) * i128::from(fraction));
            (LegFeeInput::ProvisionalBound { fraction: fraction_decimal }, b12)
        };
        let x12 = (q12 - b12) / step12 * step12;
        let c12 = i128::from(consumed_raw) % (2 * x12 + 2);
        let out = initial_quantity(InitialQuantityInput {
            gross: Decimal::new(q, q_scale), lot_step: Decimal::new(step, step_scale),
            consumed: decimal(c12), fee,
        }).unwrap();
        prop_assert_eq!(out.base_fee(), decimal(b12));
        prop_assert_eq!(out.executable(), decimal(x12));
        prop_assert_eq!(out.facts().dust(), decimal(q12 - b12 - x12));
        prop_assert!(q12 - b12 - x12 < step12);
        let expected = if c12 <= x12 { Remainder::Quantity(decimal(x12 - c12)) }
            else { Remainder::Deficit { excess: decimal(c12 - x12) } };
        prop_assert_eq!(out.facts().remainder(), expected);
    }

    #[test]
    fn final_fee_quantity_matches_scaled_integer_oracle(
        q in 0i64..1_000_000_000_000, step in 1i64..1_000_000, step_scale in 0u32..=8,
        fee_raw in any::<u64>(), prior_raw in any::<u64>(), consumed_raw in any::<u64>(),
    ) {
        let decimal = |n: i128| Decimal::from_i128_with_scale(n, 12);
        let q12 = i128::from(q) * 10_000;
        let step12 = i128::from(step) * 10i128.pow(12 - step_scale);
        let b12 = i128::from(fee_raw) % (q12 + 1);
        let prior12 = i128::from(prior_raw) % (q12 / step12 + 1) * step12;
        let x12 = prior12.min((q12 - b12) / step12 * step12);
        let c12 = i128::from(consumed_raw) % (2 * x12 + 2);
        let out = final_fee_quantity(FinalFeeInput {
            gross: Decimal::new(q, 8), base_fee: decimal(b12), prior_executable: decimal(prior12),
            lot_step: Decimal::new(step, step_scale), consumed: decimal(c12),
        }).unwrap();
        prop_assert!(out.executable() <= decimal(prior12));
        prop_assert_eq!(out.executable(), decimal(x12));
        prop_assert_eq!(out.facts().dust(), decimal(q12 - b12 - x12));
        prop_assert_eq!(out.base_fee() + out.facts().dust() + out.executable(), decimal(q12));
        let expected = if c12 <= x12 { Remainder::Quantity(decimal(x12 - c12)) }
            else { Remainder::Deficit { excess: decimal(c12 - x12) } };
        prop_assert_eq!(out.facts().remainder(), expected);
    }
}
