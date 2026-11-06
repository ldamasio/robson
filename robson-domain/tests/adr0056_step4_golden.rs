//! ADR-0056 step 4 stability contract (§4, §10).
//!
//! Every file under `fixtures/adr0056_step4/` was written by the serializer and
//! the sizing code of commit 93a3bb76, before the naming cleanup, and must stay
//! byte-identical. These tests never name the renamed Rust items, so a rename
//! cannot compile-fix them into agreement with itself.

use std::str::FromStr;

use robson_domain::{
    build_executable_stop_plan, size_entry, CredentialId, Event, Exchange, ExecutableSpanSource,
    Position, Price, RiskConfig, Side, StopPlanInputs, StopPolicy, Symbol, SymbolTradingRules,
    TechnicalStopDistance,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::Value;

const WIRE_KEY: &str = "binance_position_id";
const RUST_NAME: &str = "exchange_position_id";
const VENUE_POSITION: &str = "8389765512345";

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/adr0056_step4/", $name)).trim_end()
    };
}

fn event_roundtrip(input: &str) -> String {
    serde_json::to_string(&serde_json::from_str::<Event>(input).unwrap()).unwrap()
}

fn position_roundtrip(input: &str) -> String {
    serde_json::to_string(&serde_json::from_str::<Position>(input).unwrap()).unwrap()
}

// =============================================================================
// Event payload
// =============================================================================

#[test]
fn event_canonical_serialization_is_stable() {
    for golden in [
        fixture!("event_entry_filled_null.json"),
        fixture!("event_entry_filled_value.json"),
    ] {
        assert_eq!(event_roundtrip(golden), golden);

        // The event log persists `to_value` and replay reads `from_value`.
        let persisted: Value = serde_json::from_str(golden).unwrap();
        let replayed: Event = serde_json::from_value(persisted.clone()).unwrap();
        assert_eq!(serde_json::to_value(&replayed).unwrap(), persisted);
        assert_eq!(serde_json::to_string(&replayed).unwrap(), golden);
    }
}

#[test]
fn event_missing_field_reserializes_as_null() {
    let input = fixture!("event_entry_filled_missing.input.json");
    assert!(!input.contains(WIRE_KEY));

    let output = event_roundtrip(input);
    assert_eq!(output, fixture!("event_entry_filled_missing.expected.json"));
    assert_eq!(serde_json::from_str::<Value>(&output).unwrap()[WIRE_KEY], Value::Null);
}

#[test]
fn event_historical_payload_deserializes_and_keeps_the_id() {
    let output = event_roundtrip(fixture!("event_entry_filled_historical.input.json"));
    assert_eq!(output, fixture!("event_entry_filled_historical.expected.json"));
    assert_eq!(serde_json::from_str::<Value>(&output).unwrap()[WIRE_KEY], VENUE_POSITION);
}

#[test]
fn event_wire_carries_only_the_legacy_key() {
    let output = event_roundtrip(fixture!("event_entry_filled_value.json"));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value[WIRE_KEY], VENUE_POSITION);
    assert!(value.get(RUST_NAME).is_none());

    // The Rust name is not an accepted wire alias.
    let renamed = fixture!("event_entry_filled_value.json").replace(WIRE_KEY, RUST_NAME);
    let value: Value = serde_json::from_str(&event_roundtrip(&renamed)).unwrap();
    assert_eq!(value[WIRE_KEY], Value::Null);
    assert!(value.get(RUST_NAME).is_none());
}

// =============================================================================
// Position (independent serde derive)
// =============================================================================

#[test]
fn position_canonical_serialization_is_stable() {
    for golden in [
        fixture!("position_armed_null.json"),
        fixture!("position_active_value.json"),
    ] {
        assert_eq!(position_roundtrip(golden), golden);
    }
}

#[test]
fn position_missing_field_reserializes_as_null() {
    let input = fixture!("position_missing.input.json");
    assert!(!input.contains(WIRE_KEY));

    let output = position_roundtrip(input);
    assert_eq!(output, fixture!("position_missing.expected.json"));
    assert_eq!(serde_json::from_str::<Value>(&output).unwrap()[WIRE_KEY], Value::Null);
}

#[test]
fn position_historical_payload_deserializes_and_keeps_the_id() {
    let output = position_roundtrip(fixture!("position_historical.input.json"));
    assert_eq!(output, fixture!("position_historical.expected.json"));
    assert_eq!(serde_json::from_str::<Value>(&output).unwrap()[WIRE_KEY], VENUE_POSITION);
}

#[test]
fn position_wire_carries_only_the_legacy_key() {
    let output = position_roundtrip(fixture!("position_active_value.json"));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value[WIRE_KEY], VENUE_POSITION);
    assert!(value.get(RUST_NAME).is_none());

    // The Rust name is not an accepted wire alias.
    let renamed = fixture!("position_active_value.json").replace(WIRE_KEY, RUST_NAME);
    let value: Value = serde_json::from_str(&position_roundtrip(&renamed)).unwrap();
    assert_eq!(value[WIRE_KEY], Value::Null);
    assert!(value.get(RUST_NAME).is_none());
}

// =============================================================================
// Credential identity
// =============================================================================

#[test]
fn exchange_encodings_are_stable() {
    for (exchange, encoded) in [
        (Exchange::Binance, "binance"),
        (Exchange::BinanceTestnet, "binance_testnet"),
    ] {
        assert_eq!(exchange.as_str(), encoded);
        assert_eq!(Exchange::from_str(encoded), Some(exchange));

        let quoted = format!("\"{encoded}\"");
        assert_eq!(serde_json::to_string(&exchange).unwrap(), quoted);
        assert_eq!(serde_json::from_str::<Exchange>(&quoted).unwrap(), exchange);

        let aad = String::from_utf8(CredentialId::single_user(exchange).aad()).unwrap();
        assert_eq!(aad, format!("default|local|default|{encoded}"));
    }

    let check =
        include_str!("../../migrations/20240101000004_exchange_credentials_multi_tenant.sql");
    assert!(check.contains("CHECK (exchange IN ('binance', 'binance_testnet'))"));
}

// =============================================================================
// Sizing numbers
// =============================================================================

fn decimal(value: &Value) -> Decimal {
    Decimal::from_str(value.as_str().unwrap()).unwrap()
}

fn btcusdt_rules() -> SymbolTradingRules {
    SymbolTradingRules::new(
        Symbol::from_pair("BTCUSDT").unwrap(),
        dec!(0.10),
        dec!(556.80),
        dec!(0.001),
        dec!(0.001),
        dec!(1000),
        dec!(100),
        2,
        3,
    )
    .unwrap()
}

#[test]
fn size_entry_numbers_are_stable() {
    let cases: Vec<Value> = serde_json::from_str(fixture!("size_entry.json")).unwrap();
    assert_eq!(cases.len(), 9);

    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let side = if case["side"] == "long" {
            Side::Long
        } else {
            Side::Short
        };
        let executable_span = case["executable_span"].as_bool().unwrap();

        let mut config = RiskConfig::new(decimal(&case["capital"])).unwrap();
        if !case["headroom_bps"].is_null() {
            config = config.with_margin_headroom(decimal(&case["headroom_bps"])).unwrap();
        }
        let entry = Price::new(decimal(&case["entry"])).unwrap();
        let stop = Price::new(decimal(&case["stop"])).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let rules = executable_span.then(btcusdt_rules);
        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: if executable_span {
                StopPolicy::ExecutableSpan
            } else {
                StopPolicy::LegacyUncapped
            },
            side,
            technical_stop: stop,
            guard: None,
            entry_reference: executable_span.then_some(entry),
            technical_span: executable_span.then(|| tech_stop.span()),
            stop_buffer_bps: if executable_span {
                dec!(10)
            } else {
                Decimal::ZERO
            },
            executable_span_source: ExecutableSpanSource::Admission,
            rules: rules.as_ref(),
        })
        .unwrap();
        let available = (!case["available"].is_null()).then(|| decimal(&case["available"]));

        match size_entry(&config, &entry, &tech_stop, &plan, available, rules.as_ref()) {
            Ok(sized) => {
                // Compared as text so the decimal scale is pinned too.
                assert_eq!(sized.quantity.as_decimal().to_string(), case["quantity"], "{name}");
                assert_eq!(
                    sized.worst_case_loss_per_unit.to_string(),
                    case["worst_case_loss_per_unit"],
                    "{name}"
                );
                assert_eq!(sized.planned_risk.to_string(), case["planned_risk"], "{name}");
            },
            Err(error) => assert_eq!(error.to_string(), case["error"], "{name}"),
        }
    }
}
