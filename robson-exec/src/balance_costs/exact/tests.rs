use proptest::prelude::*;
use rust_decimal_macros::dec;

use super::*;

#[test]
fn exact_decimal_text_keeps_full_coefficient_and_normalizes_only_zeros() {
    for (text, expected) in [
        ("79228162514264337593543950335", Decimal::MAX),
        ("79228162514264337593543950335.000", Decimal::MAX),
        ("00001.230000000000000000000000000000000000", dec!(1.23)),
        ("0.0000000000000000000000000001", Decimal::from_parts(1, 0, 0, false, 28)),
        ("-1.2500", dec!(-1.25)),
    ] {
        assert_eq!(text.parse::<ExactDecimal>().unwrap().value(), expected, "{text}");
    }
}

#[test]
fn exact_decimal_text_rejects_rounding_overflow_and_non_plain_grammar() {
    for text in [
        "0.00000000000000000000000000001",
        "-0.00000000000000000000000000001",
        "1.00000000000000000000000000001",
        "79228162514264337593543950336",
        "7922816251426433759354395033.6",
        "",
        "-",
        "+1",
        " 1",
        "1 ",
        ".1",
        "1.",
        "1.2.3",
        "--1",
        "1e-2",
        "NaN",
        "inf",
        "١",
    ] {
        assert!(text.parse::<ExactDecimal>().is_err(), "accepted {text}");
    }
    assert!("0".repeat(MAX_DECIMAL_TEXT_BYTES + 1).parse::<ExactDecimal>().is_err());
    assert_eq!(
        "0".repeat(MAX_DECIMAL_TEXT_BYTES).parse::<ExactDecimal>().unwrap().value(),
        Decimal::ZERO
    );
}

#[test]
fn exact_decimal_json_never_accepts_numeric_tokens() {
    for json in [
        "0",
        "1",
        "0.1",
        "1e-28",
        "-0.00000000000000000000000000001",
        "true",
        "null",
        "[]",
    ] {
        assert!(serde_json::from_str::<ExactDecimal>(json).is_err(), "accepted {json}");
    }
    assert_eq!(serde_json::from_str::<ExactDecimal>(r#""1.2500""#).unwrap().value(), dec!(1.25));
}

#[test]
fn nonnegative_ingress_refuses_original_negative_values() {
    #[derive(serde::Deserialize)]
    struct Amount {
        #[serde(deserialize_with = "nonnegative")]
        value: Decimal,
    }
    for value in ["-1", "-0.00000000000000000000000000001"] {
        let body = format!(r#"{{"value":"{value}"}}"#);
        assert!(serde_json::from_str::<Amount>(&body).is_err());
    }
    assert_eq!(serde_json::from_str::<Amount>(r#"{"value":"0"}"#).unwrap().value, Decimal::ZERO);
}

proptest! {
    #[test]
    fn every_native_decimal_round_trips_without_losing_a_digit(
        lo in any::<u32>(), mid in any::<u32>(), hi in any::<u32>(),
        scale in 0_u32..=28, negative in any::<bool>(), zeros in 0_usize..=12,
    ) {
        let expected = Decimal::from_parts(lo, mid, hi, negative, scale);
        let mut text = expected.to_string();
        if !text.contains('.') { text.push('.'); }
        text.push_str(&"0".repeat(zeros + 1));
        prop_assert_eq!(text.parse::<ExactDecimal>().unwrap().value(), expected);
    }
}
