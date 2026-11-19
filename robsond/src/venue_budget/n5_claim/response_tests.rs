use robson_domain::Symbol;
use robson_exec::{venue::VenueId, venue_clock::ClockDomain};
use serde_json::Value;

use super::*;

// Wire example for one verified fixture account.
const FEE_RESPONSE: &str = r#"{
    "symbol":"ETHUSDT",
    "standardCommission":{"maker":"0.0010","taker":"0.002","buyer":"0","seller":"0","note":"omit"},
    "specialCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},
    "taxCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},
    "discount":{"enabledForAccount":false,"enabledForSymbol":true,"discountAsset":"BNB","discount":"0.25"},
    "uid":999,
    "unrelated":"omit"
}"#;

fn attempt() -> (RawClockCapture, CostIdentity, TickCapture) {
    let domain = ClockDomain::new(Uuid::from_u128(1), -1, 123).unwrap();
    let handoff = TickCapture::new(domain, 20, 0).unwrap();
    let raw = RawClockCapture {
        epoch: Uuid::from_u128(2),
        wall_seconds: 1_795_123_400,
        wall_nanoseconds: 802_345_999,
        domain,
        before_ns: 20_000_000_000,
        after_ns: 20_000_000_016,
    };
    let expected = CostIdentity::new(
        Uuid::from_u128(3),
        VenueId::new(VenueId::BINANCE_SPOT),
        "verified-fixture".to_owned(),
        4,
        Symbol::from_pair("ETHUSDT").unwrap(),
    )
    .unwrap();
    (raw, expected, handoff)
}

#[test]
fn retains_the_exact_original_bracket_and_uses_its_earliest_tick() {
    let (raw, expected, handoff) = attempt();
    let snapshot = prepare_response(FEE_RESPONSE.as_bytes(), raw, &expected, handoff).unwrap();
    assert_eq!(snapshot.observed, handoff);
    assert_eq!(snapshot.evidence.account(), &expected);
    assert_eq!(snapshot.evidence.as_of().timestamp(), raw.wall_seconds);
    assert_eq!(snapshot.evidence.as_of().timestamp_subsec_nanos(), 802_345_000);
    let stored: Value = serde_json::from_slice(&snapshot.payload).unwrap();
    assert_eq!(stored["format_version"], 1);
    assert_eq!(stored["original"]["epoch"], raw.epoch.to_string());
    assert_eq!(stored["original"]["boot"], raw.domain.boot().to_string());
    assert_eq!(stored["original"]["offset_seconds"], -1);
    assert_eq!(stored["original"]["offset_nanoseconds"], 123);
    assert_eq!(stored["original"]["wall_seconds"], raw.wall_seconds);
    assert_eq!(stored["original"]["wall_nanoseconds"], raw.wall_nanoseconds);
    assert_eq!(stored["original"]["before_ns"], raw.before_ns.to_string());
    assert_eq!(stored["original"]["after_ns"], raw.after_ns.to_string());
    assert_eq!(stored["commission"]["standardCommission"]["maker"], "0.0010");
    assert!(stored["commission"].get("uid").is_none());
    assert!(stored["commission"].get("unrelated").is_none());
    assert!(stored["commission"]["standardCommission"].get("note").is_none());
}

#[test]
fn normalized_evidence_preserves_distinct_retained_fee_spellings() {
    let (raw, expected, handoff) = attempt();
    let first = prepare_response(FEE_RESPONSE.as_bytes(), raw, &expected, handoff).unwrap();
    let changed = FEE_RESPONSE.replace("0.0010", "0.001");
    let second = prepare_response(changed.as_bytes(), raw, &expected, handoff).unwrap();
    assert_eq!(first.evidence, second.evidence);
    assert_ne!(first.payload, second.payload);
}

#[test]
fn malformed_foreign_and_repeated_wire_fields_fail_closed() {
    let (raw, expected, handoff) = attempt();
    for invalid in [
        r#"["ETHUSDT",["0","0","0","0"],["0","0","0","0"],["0","0","0","0"],[false,true,"BNB","0"]]"#.to_owned(),
        FEE_RESPONSE.replace("ETHUSDT", "BTCUSDT"),
        FEE_RESPONSE.replace(r#""maker":"0.0010""#, r#""maker":0.0010"#),
        FEE_RESPONSE.replace(
            r#"{"maker":"0.0010","taker":"0.002","buyer":"0","seller":"0","note":"omit"}"#,
            r#"["0.0010","0.002","0","0"]"#,
        ),
        FEE_RESPONSE.replace(r#""maker":"0.0010""#, r#""maker":"0.0010","maker":"0.0010""#),
        FEE_RESPONSE.replace(r#""symbol":"ETHUSDT""#, r#""symbol":"ETHUSDT","symbol":"ETHUSDT""#),
        FEE_RESPONSE.replace(
            r#""enabledForAccount":false"#,
            r#""enabledForAccount":false,"enabledForAccount":true"#,
        ),
    ] {
        assert!(matches!(
            prepare_response(invalid.as_bytes(), raw, &expected, handoff),
            Err(ClaimResult::InvalidPayload)
        ));
    }
    let policy_mismatch =
        FEE_RESPONSE.replace(r#""enabledForAccount":false"#, r#""enabledForAccount":true"#);
    assert!(matches!(
        prepare_response(policy_mismatch.as_bytes(), raw, &expected, handoff),
        Err(ClaimResult::PolicyMismatch)
    ));
    let mut bounded_body = FEE_RESPONSE.as_bytes().to_vec();
    bounded_body.resize(MAX_RESPONSE_BYTES, b' ');
    assert!(prepare_response(&bounded_body, raw, &expected, handoff).is_ok());
    bounded_body.push(b' ');
    assert!(matches!(
        prepare_response(&bounded_body, raw, &expected, handoff),
        Err(ClaimResult::InvalidPayload)
    ));
}

#[test]
fn malformed_or_incomparable_response_marks_produce_no_snapshot() {
    let (raw, expected, handoff) = attempt();
    for invalid in [
        RawClockCapture { before_ns: -1, ..raw },
        RawClockCapture { before_ns: raw.before_ns - 1, ..raw },
        RawClockCapture { after_ns: raw.before_ns - 1, ..raw },
        RawClockCapture { epoch: Uuid::nil(), ..raw },
        RawClockCapture {
            domain: ClockDomain::new(raw.domain.boot(), 0, 123).unwrap(),
            ..raw
        },
        RawClockCapture { wall_nanoseconds: 1_000_000_000, ..raw },
        RawClockCapture {
            wall_seconds: POSTGRES_MIN_UNIX_SECONDS - 1,
            ..raw
        },
        RawClockCapture { wall_seconds: i64::MAX, ..raw },
    ] {
        assert!(matches!(
            prepare_response(FEE_RESPONSE.as_bytes(), invalid, &expected, handoff),
            Err(ClaimResult::Unknown)
        ));
    }
    assert!(response_time(
        RawClockCapture {
            wall_seconds: POSTGRES_MIN_UNIX_SECONDS,
            ..raw
        },
        handoff
    )
    .is_some());
}

#[test]
fn retained_receipt_decodes_only_the_exact_writer_output() {
    let (raw, expected, handoff) = attempt();
    let snapshot = prepare_response(FEE_RESPONSE.as_bytes(), raw, &expected, handoff).unwrap();
    let decoded = decode_retained(&snapshot.payload, &expected, handoff).unwrap();
    assert_eq!(decoded.original, raw);
    assert_eq!(decoded.observed, snapshot.observed);
    assert_eq!(decoded.evidence, snapshot.evidence);
    let stored = std::str::from_utf8(&snapshot.payload).unwrap();
    let mut padded = snapshot.payload.clone();
    padded.push(b' ');
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(&snapshot.payload).unwrap())
            .unwrap();
    let version = r#""format_version":1"#;
    let before = r#""before_ns":"20000000000""#;
    for invalid in [
        Vec::new(),
        padded,
        pretty,
        vec![b' '; MAX_PAYLOAD_BYTES + 1],
        stored.replace(version, r#""format_version":2"#).into_bytes(),
        stored.replace(version, r#""format_version":1,"format_version":1"#).into_bytes(),
        stored.replace(version, r#""format_version":1,"extra":0"#).into_bytes(),
        stored.replace(r#""epoch""#, r#""\u0065poch""#).into_bytes(),
        stored.replace(before, r#""before_ns":"020000000000""#).into_bytes(),
        stored.replace(before, r#""before_ns":"+20000000000""#).into_bytes(),
        stored.replace(before, r#""before_ns":20000000000"#).into_bytes(),
        stored
            .replace(r#""maker":"0.0010""#, r#""maker":"0.0010","maker":"0.0010""#)
            .into_bytes(),
        stored.replace("ETHUSDT", "BTCUSDT").into_bytes(),
    ] {
        assert!(decode_retained(&invalid, &expected, handoff).is_none());
    }
    let later = TickCapture::new(raw.domain, 21, 0).unwrap();
    assert!(decode_retained(&snapshot.payload, &expected, later).is_none());
}
