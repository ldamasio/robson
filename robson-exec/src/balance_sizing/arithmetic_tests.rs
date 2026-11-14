use proptest::prelude::*;
use rust_decimal_macros::dec;

use super::*;
use crate::balance_costs::exact::ExactDecimal;

fn native(s: &str) -> Decimal {
    s.parse::<ExactDecimal>().unwrap().value()
}
fn r(s: &str) -> Rational {
    Rational::decimal(native(s)).unwrap()
}
fn rational(n: i128, d: i128) -> Rational {
    Rational::new(BigUint::from(n as u128), BigUint::from(d as u128)).unwrap()
}
fn equivalent(value: &Rational, n: i128, d: i128) {
    // Independent bounded cross-product oracle. Production rational helpers
    // are deliberately not used for the expected arithmetic or comparison.
    assert_eq!(
        &value.numerator * BigUint::from(d as u128),
        &value.denominator * BigUint::from(n as u128)
    );
}

#[test]
fn fraction_extreme_goldens_match() {
    let rows: serde_json::Value =
        serde_json::from_str(include_str!("testdata/arithmetic.json")).unwrap();
    for row in rows.as_array().unwrap() {
        let a = r(row["a"].as_str().unwrap());
        let b = r(row["b"].as_str().unwrap());
        let mut math = Arithmetic::default();
        let result = match row["op"].as_str().unwrap() {
            "add" => math.add(&a, &b),
            "sub" => math.sub(&a, &b),
            "mul" => math.mul(&a, &b),
            "div" => math.div(&a, &b),
            "floor" => math.floor_step(&a, &b),
            "ceil" => math.ceil_step(&a, &b),
            "floor12" => math.floor12(&a),
            "floor8" => math.floor8(&a),
            other => panic!("unexpected oracle operation {other}"),
        }
        .and_then(|v| math.decimal(&v));
        let expected = row["expected"].as_str().unwrap();
        match expected {
            "Negative" => assert_eq!(result, Err(MathError::Negative), "{row}"),
            "ZeroDenominator" => assert_eq!(result, Err(MathError::ZeroDenominator), "{row}"),
            "Unrepresentable" => assert_eq!(result, Err(MathError::Unrepresentable), "{row}"),
            _ => assert_eq!(result, Ok(native(expected)), "{row}"),
        }
    }
}

#[test]
fn floors_keep_the_distinct_twelve_and_eight_place_policies() {
    let x = r("0.1234567890129999999999999999");
    let mut math = Arithmetic::default();
    let twelve = math.floor12(&x).unwrap();
    let eight = math.floor8(&x).unwrap();
    assert_eq!(math.decimal(&twelve), Ok(dec!(0.123456789012)));
    assert_eq!(math.decimal(&eight), Ok(dec!(0.12345678)));
}

#[test]
fn exact_ceiling_does_not_lose_a_low_order_unit() {
    let mut math = Arithmetic::default();
    let x = r("0.1000000000000000000000000001");
    let grid = r("0.1");
    let ceiling = math.ceil_step(&x, &grid).unwrap();
    let floor = math.floor_step(&x, &grid).unwrap();
    assert_eq!(math.decimal(&ceiling), Ok(dec!(0.2)));
    assert_eq!(math.decimal(&floor), Ok(dec!(0.1)));
    let exact = math.ceil_step(&r("0.3"), &grid).unwrap();
    assert_eq!(math.decimal(&exact), Ok(dec!(0.3)));
}

#[test]
fn native_export_never_rounds_a_nonterminating_or_scale29_value() {
    let mut math = Arithmetic::default();
    let third = math.div(&r("1"), &r("3")).unwrap();
    assert_eq!(math.decimal(&third), Err(MathError::Unrepresentable));
    let tiny = math.div(&r("0.0000000000000000000000000001"), &r("10")).unwrap();
    assert_eq!(math.decimal(&tiny), Err(MathError::Unrepresentable));
    let recoverable = math.mul(&tiny, &r("10")).unwrap();
    assert_eq!(math.decimal(&recoverable), Ok(native("0.0000000000000000000000000001")));
}

#[test]
fn oversized_intermediate_can_be_cancelled_before_native_export() {
    let mut math = Arithmetic::default();
    let x = r("900000000000000000000000000");
    let intermediate = math.mul(&x, &r("100")).unwrap();
    assert_eq!(math.decimal(&intermediate), Err(MathError::Unrepresentable));
    let recovered = math.div(&intermediate, &r("100")).unwrap();
    assert_eq!(math.decimal(&recovered), Ok(native("900000000000000000000000000")));
}

#[test]
fn signs_zero_and_denominators_fail_without_panics() {
    assert!(matches!(Rational::decimal(dec!(-1)), Err(MathError::Negative)));
    let mut math = Arithmetic::default();
    assert!(matches!(math.sub(&r("0"), &r("1")), Err(MathError::Negative)));
    assert!(matches!(math.div(&r("1"), &r("0")), Err(MathError::ZeroDenominator)));
    assert!(matches!(math.ceil_step(&r("1"), &r("0")), Err(MathError::ZeroDenominator)));
    assert!(matches!(math.floor_step(&r("1"), &r("0")), Err(MathError::ZeroDenominator)));
    let zero = math.mul(&r("0"), &r("1")).unwrap();
    assert_eq!(math.decimal(&zero), Ok(Decimal::ZERO));
}

#[test]
fn technical_work_limits_are_explicit_and_bounded() {
    let large = BigUint::from(1_u8) << (MAX_BITS - 1) as usize;
    assert_eq!(product(&large, &BigUint::from(2_u8)), Err(MathError::WorkLimit));
    assert_eq!(sum(&large, &large), Err(MathError::WorkLimit));
    let one = BigUint::from(1_u8);
    assert!(matches!(Rational::new(&large << 1, one), Err(MathError::WorkLimit)));
    let mut math = Arithmetic::default();
    for _ in 0..MAX_OPERATIONS {
        assert_eq!(math.compare(&r("1"), &r("1")), Ok(Ordering::Equal));
    }
    assert_eq!(math.operations(), MAX_OPERATIONS);
    assert_eq!(math.compare(&r("1"), &r("1")), Err(MathError::WorkLimit));
    assert_eq!(math.operations(), MAX_OPERATIONS);
}

#[test]
fn largest_operands_divide_under_the_runtime_stack_limit() {
    let n = (BigUint::from(1_u8) << MAX_BITS as usize) - BigUint::from(1_u8);
    let d = (BigUint::from(1_u8) << 2048_usize) + BigUint::from(123_u8);
    let (q, remainder) = quotient_remainder(&n, &d);
    assert!(remainder < d);
    assert_eq!(&q * &d + remainder, n);
    let same = Rational::new(n.clone(), n).unwrap();
    assert_eq!(Arithmetic::default().decimal(&same), Ok(Decimal::ONE));
}

proptest! {
    #[test]
    fn bounded_i128_oracle_matches_all_primitives(
        an in 0_i128..1_000_000, ad in 1_i128..10_000,
        bn in 1_i128..1_000_000, bd in 1_i128..10_000,
    ) {
        let a = rational(an, ad); let b = rational(bn, bd);
        let mut math = Arithmetic::default();
        equivalent(&math.add(&a, &b).unwrap(), an*bd+bn*ad, ad*bd);
        equivalent(&math.mul(&a, &b).unwrap(), an*bn, ad*bd);
        equivalent(&math.div(&a, &b).unwrap(), an*bd, ad*bn);
        prop_assert_eq!(math.compare(&a, &b).unwrap(), (an*bd).cmp(&(bn*ad)));
        if an*bd >= bn*ad {
            equivalent(&math.sub(&a, &b).unwrap(), an*bd-bn*ad, ad*bd);
        } else { prop_assert!(math.sub(&a, &b).is_err()); }
        let grid_n = an*bd; let grid_d = ad*bn;
        equivalent(&math.floor_step(&a, &b).unwrap(), (grid_n/grid_d)*bn, bd);
        equivalent(&math.ceil_step(&a, &b).unwrap(), ((grid_n+grid_d-1)/grid_d)*bn, bd);
        equivalent(&math.floor12(&a).unwrap(), an*1_000_000_000_000/ad, 1_000_000_000_000);
        equivalent(&math.floor8(&a).unwrap(), an*100_000_000/ad, 100_000_000);
    }

    #[test]
    fn native_decimal_roundtrips_every_generated_scale(coefficient in any::<u128>(), scale in 0_u32..29) {
        let coefficient = coefficient & ((1_u128<<96)-1);
        let value = Decimal::from_parts(coefficient as u32, (coefficient>>32) as u32, (coefficient>>64) as u32, false, scale);
        prop_assert_eq!(Arithmetic::default().decimal(&Rational::decimal(value).unwrap()), Ok(value));
    }
}
