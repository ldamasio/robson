//! 8p-4's source guard on the leg-facts statement (8p PLAN-r3 §6.1, §6.4):
//! the venue's own order times are never selected (v16 §C4, R9-4).

use super::LEG_FACTS;

/// The statement without its `--` comments, lowercased.
fn code() -> String {
    let lines = LEG_FACTS.lines().map(|line| line.split("--").next().unwrap_or_default());
    lines.collect::<Vec<_>>().join("\n").to_ascii_lowercase()
}

#[test]
fn leg_facts_never_select_venue_times() {
    let code = code();
    // No venue time column (`venue_time_ms`, `venue_update_time_ms`, ...)
    // nor any other milliseconds column is named, ...
    assert!(!code.contains("_ms"), "a venue time is selected:\n{code}");
    // ... and no whole row is selected or converted, which would carry them.
    for whole_row in ["*", "to_json", "row_to_json", "jsonb_populate", "hstore"] {
        assert!(!code.contains(whole_row), "{whole_row} selects whole rows:\n{code}");
    }
    // Every JSON value is built field by field.
    let objects = code.matches("jsonb_build_object(").count();
    assert!(objects >= 8, "{objects} objects:\n{code}");
}

#[test]
fn the_guard_sees_a_venue_time() {
    let mutated =
        LEG_FACTS.replacen("o.live_seen_at,", "o.live_seen_at, o.venue_update_time_ms,", 1);
    assert_ne!(mutated, LEG_FACTS);
    assert!(mutated.to_ascii_lowercase().contains("_ms"));
    // The header's mention of the guard is a comment the guard ignores.
    assert!(LEG_FACTS.contains("venue's own order times") && !code().contains("venue's own"));
}

/// The reader's JSON decodes only into what 8p-3's types hold: canonical venue
/// ids, and no key they do not know at any depth, the lookup and the venue
/// times included.
#[test]
fn leg_facts_decode_only_canonical_ids_and_known_keys() {
    use robson_exec::{boundary_carry::CandidateLeg, venue_ledger::VenueIntId};
    use serde_json::{json, Value};

    let id = |text: &str| serde_json::from_value::<VenueIntId>(json!(text));
    assert_eq!(id("123").unwrap().to_string(), "123");
    for bad in ["000123", "0123", "+123", "12a", "", "18446744073709551616"] {
        assert!(id(bad).is_err(), "{bad}");
    }
    let time = || json!({"at": "2026-11-30T23:59:50Z", "verdict": "valid"});
    let leg = json!({
        "position_id": "00000000-0000-4000-8000-000000000001",
        "cohort_month": "2026-11-01",
        "executable": "0.9",
        "terms": {"gap_bps": "10", "taker_bps": "10", "remainder_reserve": "0.0001", "admission_charge": "1"},
        "dust": {"write_off": "0", "close_at": time(), "last_fill_at": "2026-11-30T00:00:00Z"},
        "summarized_sold_before_b": "0",
        "orders": [{
            "exchange_order_id": "7", "kind": "stop", "status": "NEW", "phase": "Live", "orig_qty": "0.9",
            "stop_price": "90", "live_seen": time(), "terminal_seen": null, "first_attempt": time(),
            "fills": [{"trade_time": "2026-11-30T00:00:00Z", "qty": "0.1", "price": "90", "commission": "0"}],
        }],
        "cancels": [{
            "position_id": "00000000-0000-4000-8000-000000000001", "cancels_order_id": "7",
            "venue_outcome": "success", "cancel_class": "cancelled", "first_attempt": time(),
        }],
    });
    let decoded = serde_json::from_value::<CandidateLeg>(leg.clone()).unwrap();
    assert_eq!(decoded.orders[0].fills[0].qty.to_string(), "0.1");
    let paths = [
        "",
        "/terms",
        "/dust",
        "/dust/close_at",
        "/orders/0",
        "/orders/0/live_seen",
        "/orders/0/first_attempt",
        "/orders/0/fills/0",
        "/cancels/0",
        "/cancels/0/first_attempt",
    ];
    for path in paths {
        for key in ["venue_time_ms", "lookup", "venue_record_times"] {
            let mut extra = leg.clone();
            let object = extra.pointer_mut(path).and_then(Value::as_object_mut).unwrap();
            object.insert(key.into(), json!(null));
            let refused = serde_json::from_value::<CandidateLeg>(extra);
            assert!(refused.is_err(), "{key} at {path:?}: {refused:?}");
        }
    }
    // A summary no ledger amount could hold arrives as null and refuses the leg.
    let mut unrepresentable = leg;
    unrepresentable["summarized_sold_before_b"] = Value::Null;
    assert!(serde_json::from_value::<CandidateLeg>(unrepresentable).is_err());
}
