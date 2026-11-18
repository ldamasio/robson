use std::{collections::HashMap, time::Duration};

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use robson_domain::Symbol;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use uuid::Uuid;

use super::{age::sealed, exact::ExactDecimal, *};
use crate::venue::VenueId;

fn exact(text: &str) -> ExactDecimal {
    text.parse().unwrap()
}
fn identity() -> CostIdentity {
    CostIdentity::new(
        Uuid::from_u128(1),
        VenueId::new(VenueId::BINANCE_SPOT),
        "scope-one".into(),
        7,
        Symbol::from_pair("BTCUSDT").unwrap(),
    )
    .unwrap()
}
fn date() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap()
}
fn components() -> CommissionComponents {
    CommissionComponents {
        standard: ["0.000001", "0.000002", "0.000003", "0.000004"].map(exact),
        tax: ["0.000005", "0.000006", "0.000007", "0.000008"].map(exact),
        special: ["0.000009", "0.000010", "0.000011", "0.000012"].map(exact),
        enabled_for_account: false,
        enabled_for_symbol: true,
        discount: exact("0.75"),
        discount_asset: "BNB".into(),
    }
}
fn api() -> CostCandidate {
    CostCandidate::commission(identity(), date(), components()).unwrap()
}
fn override_body() -> serde_json::Value {
    json!({"version":1,"tenant_id":Uuid::from_u128(1),"venue":"binance:spot","scope":"scope-one","uid":7,"symbol":"BTCUSDT","maker_bps":"8","taker_bps":"10","as_of":"2026-11-14T12:00:00Z","note":"dated operator terms"})
}
fn dated() -> CostCandidate {
    parse_dated_override(&override_body().to_string(), &identity()).unwrap()
}

// Only this cfg(test) implementation can supply synthetic age evidence. There
// is no public constructor or production implementation of the sealed trait.
#[derive(Default)]
struct Authority {
    ages: HashMap<[u8; 32], Result<AgeInterval, AgeError>>,
    absence: Option<Result<(), AgeError>>,
}
impl sealed::Sealed for Authority {}
impl CostAgeAuthority for Authority {
    fn age(&self, evidence: &EvidenceIdentity) -> Result<AgeInterval, AgeError> {
        self.ages.get(evidence.hash()).copied().unwrap_or(Err(AgeError::Identity))
    }
    fn confirm_absence(&self, _: &CostIdentity) -> Result<(), AgeError> {
        self.absence.unwrap_or(Err(AgeError::Unresolved))
    }
}
impl Authority {
    fn with(mut self, candidate: &CostCandidate, seconds: u64) -> Self {
        self.ages.insert(
            *candidate.evidence().hash(),
            Ok(AgeInterval {
                minimum: Duration::ZERO,
                maximum: Duration::from_secs(seconds),
            }),
        );
        self
    }
}
fn select(
    authority: &Authority,
    api: VenueCostEvidence<'_>,
    fallback: Option<&CostCandidate>,
) -> Result<CostTerms, CostError> {
    select_costs(
        authority,
        &identity(),
        api,
        fallback.map_or(DatedOverrideEvidence::Absent, DatedOverrideEvidence::Available),
        exact("10"),
    )
}

#[test]
fn all_six_components_worst_side_and_units_are_exact() {
    let api = api();
    assert_eq!((api.maker_bps(), api.taker_bps()), (dec!(0.39), dec!(0.42)));
    let selected =
        select(&Authority::default().with(&api, 1), VenueCostEvidence::Available(&api), None)
            .unwrap();
    assert_eq!(
        (selected.maker_fraction(), selected.taker_fraction()),
        (dec!(0.000039), dec!(0.000042))
    );
    assert_eq!(selected.envelope().funding, None);
    assert_eq!(selected.envelope().fee_asset_policy, crate::venue::FeeAssetPolicy::Base);
}

#[test]
fn discount_flags_refuse_even_zero_or_another_asset_and_never_discount() {
    for account in [false, true] {
        for symbol in [false, true] {
            let mut input = components();
            input.enabled_for_account = account;
            input.enabled_for_symbol = symbol;
            input.discount = exact("0");
            input.discount_asset = "OTHER".into();
            let result = CostCandidate::commission(identity(), date(), input);
            if account && symbol {
                assert!(matches!(result, Err(CostError::FeePolicyMismatch)));
            } else {
                assert_eq!(result.unwrap().taker_bps(), dec!(0.42));
            }
        }
    }
}

#[test]
fn invalid_or_unrepresentable_components_never_panic_or_round() {
    for value in ["-0.000001", "79228162514264337593543950335", "0.01"] {
        let mut input = components();
        input.standard[1] = exact(value);
        assert!(CostCandidate::commission(identity(), date(), input).is_err(), "{value}");
    }
    // This exact bps value would lose its low digit when converted to fraction.
    let mut raw = override_body();
    raw["taker_bps"] = json!("0.0000000000000000000000000001");
    assert!(matches!(
        parse_dated_override(&raw.to_string(), &identity()),
        Err(CostError::Unrepresentable)
    ));
    let mut raw = override_body();
    raw["taker_bps"] = json!("100");
    assert!(matches!(
        parse_dated_override(&raw.to_string(), &identity()),
        Err(CostError::Rate)
    ));
}

#[test]
fn fresh_venue_wins_and_exact_twenty_four_hours_is_stale() {
    let api = api();
    let fallback = dated();
    let fresh = Authority::default().with(&api, 86_399).with(&fallback, 1);
    let terms = select(&fresh, VenueCostEvidence::Available(&api), Some(&fallback)).unwrap();
    assert_eq!(terms.evidence().source(), CostSource::Venue);
    let stale = Authority::default().with(&api, 86_400).with(&fallback, 1);
    let terms = select(&stale, VenueCostEvidence::Available(&api), Some(&fallback)).unwrap();
    assert_eq!(terms.evidence().source(), CostSource::DatedOverride);
    assert_eq!(terms.evidence().as_of(), date());
    assert_eq!(terms.note(), "dated operator terms");
    assert!(matches!(
        select(&stale, VenueCostEvidence::Available(&api), None),
        Err(CostError::Stale)
    ));
    let stale = Authority::default().with(&api, 86_400).with(&fallback, 86_400);
    assert!(matches!(
        select(&stale, VenueCostEvidence::Available(&api), Some(&fallback)),
        Err(CostError::Stale)
    ));
}

#[test]
fn no_override_hides_unknown_future_invalid_or_unresolved_evidence() {
    let api = api();
    let fallback = dated();
    for error in [
        AgeError::Unknown,
        AgeError::Future,
        AgeError::Unresolved,
        AgeError::Identity,
    ] {
        let mut authority = Authority::default().with(&fallback, 0);
        authority.ages.insert(*api.evidence().hash(), Err(error));
        assert!(
            matches!(select(&authority, VenueCostEvidence::Available(&api), Some(&fallback)), Err(CostError::Age(e)) if e == error)
        );
    }
    let authority = Authority::default().with(&fallback, 0);
    assert!(matches!(
        select(&authority, VenueCostEvidence::Unavailable, Some(&fallback)),
        Err(CostError::Age(AgeError::Unresolved))
    ));
    assert!(matches!(
        select(&authority, VenueCostEvidence::Unresolved, Some(&fallback)),
        Err(CostError::Age(AgeError::Unresolved))
    ));
    assert!(matches!(
        select(
            &authority,
            VenueCostEvidence::Invalid(CostError::FeePolicyMismatch),
            Some(&fallback)
        ),
        Err(CostError::FeePolicyMismatch)
    ));
    let authority = Authority { absence: Some(Ok(())), ..authority };
    assert!(select(&authority, VenueCostEvidence::Unavailable, Some(&fallback)).is_ok());
    assert!(matches!(
        select(&authority, VenueCostEvidence::Unavailable, None),
        Err(CostError::Missing)
    ));
}

#[test]
fn positive_receipt_binds_identity_complete_evidence_and_validation_version() {
    let api = api();
    let mut authority = Authority::default().with(&api, 1);
    assert_eq!(api.evidence().validation_version(), COST_VALIDATION_VERSION);
    let other = CostIdentity::new(
        Uuid::from_u128(2),
        identity().venue().clone(),
        "scope-one".into(),
        7,
        identity().symbol().clone(),
    )
    .unwrap();
    assert!(matches!(
        select_costs(
            &authority,
            &other,
            VenueCostEvidence::Available(&api),
            DatedOverrideEvidence::Absent,
            exact("10")
        ),
        Err(CostError::Identity)
    ));
    let mut changed = components();
    changed.discount = exact("0.5");
    let changed = CostCandidate::commission(identity(), date(), changed).unwrap();
    assert_ne!(api.evidence().hash(), changed.evidence().hash());
    assert!(matches!(
        select(&authority, VenueCostEvidence::Available(&changed), None),
        Err(CostError::Age(AgeError::Identity))
    ));
    authority.ages.insert(
        *api.evidence().hash(),
        Ok(AgeInterval {
            minimum: Duration::from_secs(2),
            maximum: Duration::from_secs(1),
        }),
    );
    assert!(matches!(
        select(&authority, VenueCostEvidence::Available(&api), None),
        Err(CostError::Age(AgeError::Unknown))
    ));
}

#[test]
fn canonical_hash_normalizes_values_but_keeps_identity_source_date_gap_and_note() {
    let original = dated();
    let mut body = override_body();
    body["maker_bps"] = json!("8.0000");
    body["taker_bps"] = json!("10.000");
    assert_eq!(
        original.evidence().hash(),
        parse_dated_override(&body.to_string(), &identity()).unwrap().evidence().hash()
    );
    for (field, value) in [
        ("scope", json!("scope-two")),
        ("uid", json!(8)),
        ("symbol", json!("ETHUSDT")),
        ("note", json!("other dated note")),
        ("as_of", json!("2026-11-14T12:00:01Z")),
    ] {
        let mut body = override_body();
        body[field] = value;
        let configured = CostIdentity::new(
            identity().tenant(),
            identity().venue().clone(),
            body["scope"].as_str().unwrap().into(),
            body["uid"].as_u64().unwrap(),
            Symbol::new(if field == "symbol" { "ETH" } else { "BTC" }.into(), "USDT".into())
                .unwrap(),
        )
        .unwrap();
        assert_ne!(
            original.evidence().hash(),
            parse_dated_override(&body.to_string(), &configured).unwrap().evidence().hash(),
            "{field}"
        );
    }
    let authority = Authority {
        absence: Some(Ok(())),
        ..Authority::default().with(&original, 1)
    };
    let terms = select(&authority, VenueCostEvidence::Unavailable, Some(&original)).unwrap();
    let equivalent = select_costs(
        &authority,
        &identity(),
        VenueCostEvidence::Unavailable,
        DatedOverrideEvidence::Available(&original),
        exact("10.00"),
    )
    .unwrap();
    assert_eq!(terms.hash(), equivalent.hash());
    let other = select_costs(
        &authority,
        &identity(),
        VenueCostEvidence::Unavailable,
        DatedOverrideEvidence::Available(&original),
        exact("11"),
    )
    .unwrap();
    assert_ne!(terms.hash(), other.hash());
    assert_eq!(terms.gap_bps(), dec!(10));
}

#[test]
fn injected_config_is_bounded_exact_and_never_reads_legacy_fees() {
    let policy = CostPolicy::parse("10", None, Some("100.000")).unwrap();
    assert_eq!(policy.margin_bps(), dec!(200));
    assert_eq!(policy.max_entry_notional(), Some(dec!(100)));
    for invalid in ["-1", "0.00000000000000000000000000001", "NaN", "1e1"] {
        assert!(CostPolicy::parse(invalid, None, None).is_err());
        assert!(CostPolicy::parse("10", Some(invalid), None).is_err());
        assert!(CostPolicy::parse("10", None, Some(invalid)).is_err());
    }
    assert!(CostPolicy::parse("101", None, None).is_err());
    assert!(CostPolicy::parse("10", Some("10000"), None).is_err());
    assert!(CostPolicy::parse("10", None, Some("0")).is_err());
    for (field, value) in [
        ("maker_bps", json!(8)),
        ("taker_bps", json!(0.1)),
        ("version", json!(2)),
        ("note", json!("x".repeat(257))),
        ("scope", json!("x".repeat(129))),
        ("extra", json!(true)),
    ] {
        let mut body = override_body();
        body[field] = value;
        assert!(parse_dated_override(&body.to_string(), &identity()).is_err(), "{field}");
    }
    assert!(parse_dated_override(&" ".repeat(MAX_OVERRIDE_BYTES + 1), &identity()).is_err());
}

proptest! {
    #[test]
    fn generated_six_term_side_max_matches_integer_oracle(values in prop::array::uniform12(0_u32..10_000)) {
        let mut input = components();
        let rates: Vec<ExactDecimal> = values.iter().map(|n| exact(&Decimal::from_i128_with_scale(i128::from(*n), 8).to_string())).collect();
        input.standard.copy_from_slice(&rates[0..4]); input.tax.copy_from_slice(&rates[4..8]); input.special.copy_from_slice(&rates[8..12]);
        let candidate = CostCandidate::commission(identity(), date(), input).unwrap();
        for (kind, actual) in [(0, candidate.maker_bps()), (1, candidate.taker_bps())] {
            let sum = |side: usize| [0,4,8].iter().map(|offset| u64::from(values[offset+kind])+u64::from(values[offset+side])).sum::<u64>();
            let expected = Decimal::from_i128_with_scale(i128::from(sum(2).max(sum(3))), 4);
            prop_assert_eq!(actual, expected);
        }
    }
}

#[test]
fn invalid_override_does_not_displace_fresh_venue_or_hide_needed_config_error() {
    let api = api();
    let fresh = Authority::default().with(&api, 1);
    assert!(select_costs(
        &fresh,
        &identity(),
        VenueCostEvidence::Available(&api),
        DatedOverrideEvidence::Invalid(CostError::Config),
        exact("10")
    )
    .is_ok());
    let stale = Authority::default().with(&api, 86_400);
    assert!(matches!(
        select_costs(
            &stale,
            &identity(),
            VenueCostEvidence::Available(&api),
            DatedOverrideEvidence::Invalid(CostError::Config),
            exact("10")
        ),
        Err(CostError::Config)
    ));
}

#[test]
fn freshness_uses_the_complete_upper_bound_including_subseconds() {
    let api = api();
    let mut authority = Authority::default();
    let day = Duration::from_secs(86_400);
    for (upper, accepted) in [
        (day - Duration::from_nanos(1), true),
        (day, false),
        (day + Duration::from_nanos(1), false),
    ] {
        authority.ages.insert(
            *api.evidence().hash(),
            Ok(AgeInterval { minimum: Duration::ZERO, maximum: upper }),
        );
        assert_eq!(select(&authority, VenueCostEvidence::Available(&api), None).is_ok(), accepted);
    }
}

#[test]
fn native_scale_twenty_eight_survives_aggregation_and_fraction_round_trip() {
    let mut input = components();
    let unit = exact("0.0000000000000000000000000001");
    input.standard = [unit; 4];
    input.tax = [unit; 4];
    input.special = [unit; 4];
    let candidate = CostCandidate::commission(identity(), date(), input).unwrap();
    assert_eq!(candidate.taker_bps(), Decimal::from_parts(6, 0, 0, false, 24));
    let authority = Authority::default().with(&candidate, 1);
    let terms = select(&authority, VenueCostEvidence::Available(&candidate), None).unwrap();
    assert_eq!(terms.taker_fraction(), Decimal::from_parts(6, 0, 0, false, 28));
    let mut body = override_body();
    body["taker_bps"] = json!("0.000000000000000000000001");
    assert_eq!(
        parse_dated_override(&body.to_string(), &identity()).unwrap().taker_bps(),
        Decimal::from_parts(1, 0, 0, false, 24)
    );
}

#[test]
fn overflow_is_an_exact_arithmetic_refusal_not_a_saturated_rate() {
    let mut input = components();
    input.standard[1] = exact("79228162514264337593543950335");
    assert!(matches!(
        CostCandidate::commission(identity(), date(), input),
        Err(CostError::Unrepresentable)
    ));
}

#[test]
fn override_body_limit_rejects_an_otherwise_valid_oversize_document() {
    let body = override_body().to_string();
    let at_limit = format!("{body}{}", " ".repeat(MAX_OVERRIDE_BYTES - body.len()));
    assert!(parse_dated_override(&at_limit, &identity()).is_ok());
    assert!(matches!(
        parse_dated_override(&(at_limit + " "), &identity()),
        Err(CostError::Config)
    ));
}

#[test]
fn configured_override_supports_quotes_without_suffix_guessing() {
    // These are configured canonical symbols, not a new tradability/metadata
    // decision. No fixed quote list is consulted by the override interface.
    for (base, quote) in [("SOL", "USDC"), ("BTC", "USDC"), ("BTC", "EUR")] {
        let symbol = Symbol::new(base.into(), quote.into()).unwrap();
        let configured = CostIdentity::new(
            identity().tenant(),
            identity().venue().clone(),
            identity().scope().into(),
            identity().uid(),
            symbol.clone(),
        )
        .unwrap();
        let mut body = override_body();
        body["symbol"] = json!(symbol.as_pair());
        let candidate = parse_dated_override(&body.to_string(), &configured).unwrap();
        assert_eq!(candidate.evidence().account().symbol(), &symbol);
        let authority = Authority {
            absence: Some(Ok(())),
            ..Authority::default().with(&candidate, 1)
        };
        let terms = select_costs(
            &authority,
            &configured,
            VenueCostEvidence::Unavailable,
            DatedOverrideEvidence::Available(&candidate),
            exact("10"),
        )
        .unwrap();
        assert_eq!(terms.evidence().account(), &configured);
    }
}

#[test]
fn configured_override_rejects_every_explicit_identity_mismatch() {
    for (field, value) in [
        ("tenant_id", json!(Uuid::from_u128(2))),
        ("venue", json!("binance:usdm")),
        ("scope", json!("other-scope")),
        ("uid", json!(8)),
        ("symbol", json!("ETHUSDT")),
    ] {
        let mut body = override_body();
        body[field] = value;
        assert!(
            matches!(
                parse_dated_override(&body.to_string(), &identity()),
                Err(CostError::Identity)
            ),
            "{field}"
        );
    }
}

// Cross-module pure sizing fixtures use the same sealed test-only authority;
// no public constructor or production proof bypass is added.
pub(crate) fn sizing_terms(id: &CostIdentity, taker: &str, gap: &str) -> CostTerms {
    let body = json!({"version":1,"tenant_id":id.tenant(),"venue":id.venue().as_str(),"scope":id.scope(),"uid":id.uid(),"symbol":id.symbol().as_pair(),"maker_bps":"0","taker_bps":taker,"as_of":date(),"note":"sizing fixture"});
    let candidate = parse_dated_override(&body.to_string(), id).unwrap();
    let authority = Authority {
        absence: Some(Ok(())),
        ..Authority::default()
    }
    .with(&candidate, 1);
    select_costs(
        &authority,
        id,
        VenueCostEvidence::Unavailable,
        DatedOverrideEvidence::Available(&candidate),
        exact(gap),
    )
    .unwrap()
}
