//! ADR-0056 step 6: leg evidence carried by the event log, without a migration.
//!
//! The rules snapshot and the residual are additive on the wire: every payload
//! that does not carry them serializes byte for byte as before, which the step
//! 4 fixtures also pin. An older reader ignores the new `rules_snapshot` key.

use chrono::{TimeZone, Utc};
use robson_domain::{
    Event, Position, PositionState, Price, Quantity, ReductionKind, Side, Symbol,
    SymbolTradingRules,
};
use rust_decimal_macros::dec;
use serde_json::{json, Value};
use uuid::Uuid;

const POSITION_ID: &str = "01940000-0000-7000-8000-000000000001";
const SIGNAL_ID: &str = "01940000-0000-7000-8000-000000000002";

fn btcusdt_rules() -> SymbolTradingRules {
    SymbolTradingRules::new(
        Symbol::from_pair("BTCUSDT").unwrap(),
        dec!(0.10),
        dec!(556.80),
        dec!(0.001),
        dec!(0.001),
        dec!(120),
        dec!(50),
        2,
        3,
    )
    .unwrap()
}

fn signal_event(rules_snapshot: Option<SymbolTradingRules>) -> Event {
    Event::EntrySignalReceived {
        position_id: POSITION_ID.parse().unwrap(),
        signal_id: SIGNAL_ID.parse().unwrap(),
        entry_price: Price::new(dec!(77500)).unwrap(),
        stop_loss: Price::new(dec!(76000)).unwrap(),
        quantity: Quantity::new(dec!(0.105)).unwrap(),
        initial_executable_stop: Some(Price::new(dec!(75900)).unwrap()),
        executable_span: Some(dec!(1600)),
        cap_basis_distance: Some(dec!(1500)),
        tick_size: Some(dec!(0.10)),
        rules_snapshot,
        timestamp: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    }
}

#[test]
fn a_signal_without_a_snapshot_serializes_as_it_always_did() {
    let wire = serde_json::to_string(&signal_event(None)).unwrap();
    assert!(!wire.contains("rules_snapshot"));

    let golden = concat!(
        r#"{"type":"entry_signal_received","position_id":"01940000-0000-7000-8000-000000000001","#,
        r#""signal_id":"01940000-0000-7000-8000-000000000002","entry_price":"77500","#,
        r#""stop_loss":"76000","quantity":"0.105","initial_executable_stop":"75900","#,
        r#""executable_span":"1600","cap_basis_distance":"1500","tick_size":"0.10","#,
        r#""timestamp":"2026-11-06T12:00:00Z"}"#
    );
    assert_eq!(wire, golden);
    assert_eq!(serde_json::from_str::<Event>(golden).unwrap(), signal_event(None));
}

#[test]
fn a_snapshot_roundtrips_through_the_event_payload() {
    let event = signal_event(Some(btcusdt_rules()));
    let persisted = serde_json::to_value(&event).unwrap();
    assert_eq!(persisted["rules_snapshot"]["step_size"], "0.001");
    assert_eq!(persisted["rules_snapshot"]["max_qty"], "120");
    assert_eq!(serde_json::from_value::<Event>(persisted).unwrap(), event);
}

#[test]
fn a_corrupt_snapshot_is_refused_rather_than_trusted() {
    let mut persisted = serde_json::to_value(signal_event(Some(btcusdt_rules()))).unwrap();
    persisted["rules_snapshot"]["step_size"] = json!("0");
    assert!(serde_json::from_value::<Event>(persisted).is_err());
}

#[test]
fn a_position_without_leg_evidence_keeps_its_wire_shape() {
    let position = Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
    let wire: Value = serde_json::to_value(&position).unwrap();
    assert!(wire.get("rules_snapshot").is_none());
    assert!(wire.get("residual_quantity").is_none());
    assert!(wire.get("residual_unknown").is_none());
}

#[test]
fn the_residual_event_has_the_registered_wire_shape() {
    let event = Event::ResidualInventoryRecorded {
        position_id: POSITION_ID.parse().unwrap(),
        order_kind: ReductionKind::Exit,
        requested_quantity: dec!(0.105),
        accepted_quantity: dec!(0.1),
        residual_quantity: dec!(0.005),
        exchange_order_id: Some("7".to_string()),
        fill_price: Some(Price::new(dec!(75900)).unwrap()),
        fee: Some(dec!(3.03)),
        step_size: Some(dec!(0.01)),
        min_qty: Some(dec!(0.01)),
        timestamp: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    };
    let persisted = serde_json::to_value(&event).unwrap();
    assert_eq!(event.event_type(), "residual_inventory_recorded");
    assert_eq!(persisted["type"], "residual_inventory_recorded");
    assert_eq!(persisted["order_kind"], "exit");
    assert_eq!(persisted["residual_quantity"], "0.005");

    // Top-level keys are exactly what bronze-event-registry.yaml registers: the
    // exporter aborts a run on any other key set.
    let mut keys: Vec<&str> = persisted.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, [
        "accepted_quantity",
        "exchange_order_id",
        "fee",
        "fill_price",
        "min_qty",
        "order_kind",
        "position_id",
        "requested_quantity",
        "residual_quantity",
        "step_size",
        "timestamp",
        "type",
    ]);
    assert_eq!(serde_json::from_value::<Event>(persisted).unwrap(), event);

    let registry = include_str!("../../docs/data-contracts/bronze-event-registry.yaml");
    assert!(registry.contains("residual_inventory_recorded:"));
    assert!(registry.contains("rules_snapshot: object"));
}

fn residual_event(residual_quantity: rust_decimal::Decimal) -> Event {
    Event::ResidualInventoryRecorded {
        position_id: POSITION_ID.parse().unwrap(),
        order_kind: ReductionKind::Protection,
        requested_quantity: dec!(0.105),
        accepted_quantity: dec!(0.105) - residual_quantity,
        residual_quantity,
        exchange_order_id: None,
        fill_price: None,
        fee: None,
        step_size: None,
        min_qty: None,
        timestamp: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    }
}

fn accepted_stop_event() -> Event {
    Event::InsuranceStopReplaced {
        position_id: POSITION_ID.parse().unwrap(),
        previous_order_id: "ins-1".to_string(),
        order_id: "ins-2".to_string(),
        stop_price: Price::new(dec!(76000)).unwrap(),
        timestamp: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    }
}

fn active_leg() -> Position {
    let mut position =
        Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
    position.id = POSITION_ID.parse().unwrap();
    position.quantity = Quantity::new(dec!(0.105)).unwrap();
    position.state = PositionState::Active {
        current_price: Price::new(dec!(77500)).unwrap(),
        trailing_stop: Price::new(dec!(76000)).unwrap(),
        favorable_extreme: Price::new(dec!(77500)).unwrap(),
        extreme_at: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
        insurance_stop_id: Some("ins-1".to_string()),
        invalidation_guard_level: None,
        last_emitted_stop: None,
    };
    position
}

/// What a restart reads back from the event log, with the semantics of replay.
#[test]
fn leg_evidence_restores_the_first_snapshot_and_the_latest_residual() {
    let mut position = active_leg();
    let first = btcusdt_rules();
    let later = SymbolTradingRules::new(
        Symbol::from_pair("BTCUSDT").unwrap(),
        dec!(0.10),
        dec!(556.80),
        dec!(0.01),
        dec!(0.01),
        dec!(120),
        dec!(50),
        2,
        3,
    )
    .unwrap();

    // The first signal carries no snapshot: the first one that does wins.
    position.restore_leg_evidence(&[
        signal_event(None),
        signal_event(Some(first.clone())),
        signal_event(Some(later)),
        residual_event(dec!(0.005)),
        residual_event(dec!(0.004)),
    ]);
    assert_eq!(position.rules_snapshot, Some(first));
    assert_eq!(position.residual_quantity, Some(dec!(0.004)));
    assert!(position.has_unresolved_residual());

    // An accepted stop does not clear it, however many of them follow: the
    // repeated replacements of a rounded stop write no new residual.
    position.restore_leg_evidence(&[accepted_stop_event(), accepted_stop_event()]);
    assert_eq!(position.residual_quantity, Some(dec!(0.004)));
    assert_eq!(position.quantity.as_decimal(), dec!(0.105), "evidence moves no quantity");
}

/// An exit that executed closes its leg, so its record is evidence and nothing
/// more. An exit with nothing executable leaves its leg open, and that stands.
#[test]
fn only_what_leaves_a_leg_open_stands_as_its_residual() {
    let exit = |accepted, residual, fill_price: Option<Price>| Event::ResidualInventoryRecorded {
        position_id: POSITION_ID.parse().unwrap(),
        order_kind: ReductionKind::Exit,
        requested_quantity: dec!(0.105),
        accepted_quantity: accepted,
        residual_quantity: residual,
        exchange_order_id: fill_price.map(|_| "7".to_string()),
        fill_price,
        fee: None,
        step_size: None,
        min_qty: None,
        timestamp: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    };
    let filled = exit(dec!(0.1), dec!(0.005), Some(Price::new(dec!(75900)).unwrap()));
    assert_eq!(filled.standing_residual(), None);
    assert_eq!(exit(dec!(0), dec!(0.105), None).standing_residual(), Some(dec!(0.105)));
    assert_eq!(residual_event(dec!(0.005)).standing_residual(), Some(dec!(0.005)));
    assert_eq!(accepted_stop_event().standing_residual(), None);

    let mut leg = active_leg();
    leg.restore_leg_evidence([&filled]);
    assert!(!leg.has_unresolved_residual());
}

/// A poison row must not crash-loop the daemon. An unreadable residual row
/// blocks admissions until the leg closes; rules can stay degraded.
#[test]
fn an_unreadable_residual_row_blocks_admissions_on_an_open_leg() {
    let poison = |event_type: &str| json!({ "type": event_type, "garbage": true });
    let event_type = "residual_inventory_recorded";

    let mut leg = active_leg();
    assert!(leg.restore_leg_evidence_record(event_type, poison(event_type)).is_err());
    assert!(leg.residual_unknown);
    assert_eq!(leg.residual_quantity, None);
    assert!(leg.has_unresolved_residual());

    // Nothing later in the stream clears it, not even a readable residual.
    leg.restore_leg_evidence_record(
        event_type,
        serde_json::to_value(residual_event(dec!(0.005))).unwrap(),
    )
    .unwrap();
    leg.restore_leg_evidence_record(
        "insurance_stop_replaced",
        serde_json::to_value(accepted_stop_event()).unwrap(),
    )
    .unwrap();
    assert!(leg.residual_unknown);
    assert_eq!(leg.residual_quantity, Some(dec!(0.005)));

    // Rules can stay degraded.
    let mut leg = active_leg();
    let event_type = "entry_signal_received";
    leg.restore_leg_evidence_record(event_type, poison(event_type)).unwrap_err();
    assert!(!leg.has_unresolved_residual());

    // A row that reads as another event, or as another leg's, counts for nothing it
    // claims. A residual row then blocks; a rules row is no snapshot.
    let event_type = "residual_inventory_recorded";
    let mut leg = active_leg();
    let other_variant = serde_json::to_value(accepted_stop_event()).unwrap();
    assert!(leg.restore_leg_evidence_record(event_type, other_variant.clone()).is_err());
    assert!(leg.residual_unknown);
    assert_eq!(leg.residual_quantity, None);

    let mut leg = active_leg();
    leg.id = Uuid::now_v7();
    let other_leg = serde_json::to_value(residual_event(dec!(0.005))).unwrap();
    assert!(leg.restore_leg_evidence_record(event_type, other_leg).is_err());
    assert!(leg.residual_unknown);
    assert_eq!(leg.residual_quantity, None);

    let event_type = "entry_signal_received";
    let mut leg = active_leg();
    assert!(leg.restore_leg_evidence_record(event_type, other_variant).is_err());
    assert_eq!(leg.rules_snapshot, None);
    assert!(!leg.has_unresolved_residual());

    let mut leg = active_leg();
    leg.id = Uuid::now_v7();
    let other_leg = serde_json::to_value(signal_event(Some(btcusdt_rules()))).unwrap();
    assert!(leg.restore_leg_evidence_record(event_type, other_leg).is_err());
    assert_eq!(leg.rules_snapshot, None);
    assert!(!leg.has_unresolved_residual());

    // The row it says it is does count.
    let mut leg = active_leg();
    let own = serde_json::to_value(signal_event(Some(btcusdt_rules()))).unwrap();
    leg.restore_leg_evidence_record(event_type, own).unwrap();
    assert_eq!(leg.rules_snapshot, Some(btcusdt_rules()));

    // Only an open leg has anything to block.
    let mut armed =
        Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
    let event_type = "residual_inventory_recorded";
    armed.restore_leg_evidence_record(event_type, poison(event_type)).unwrap_err();
    assert!(!armed.has_unresolved_residual());
}
