use chrono::{TimeZone, Utc};
use robson_domain::Symbol;
use robson_exec::{balance_sizing::PreparedBinding, venue::VenueId};

use super::{super::rules::parse_symbol_rules, *};
fn setup() -> (SpotSymbolRules, CostIdentity, ObservationRef) {
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let rules =
        parse_symbol_rules(include_str!("testdata/exchange_info_btcusdt.json"), &symbol.as_pair())
            .unwrap();
    let account = CostIdentity::new(
        "00000000-0000-0000-0000-000000000001".parse().unwrap(),
        VenueId::new(VenueId::BINANCE_SPOT),
        "scope".into(),
        7,
        symbol,
    )
    .unwrap();
    let observation = ObservationRef {
        binding: PreparedBinding {
            account: account.clone(),
            preparation_id: "00000000-0000-0000-0000-000000000002".parse().unwrap(),
            accounting_version: 3,
        },
        observation_id: "00000000-0000-0000-0000-000000000004".parse().unwrap(),
        source_generation: 9,
        original_as_of: Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap(),
    };
    (rules, account, observation)
}
#[test]
fn projection_preserves_every_full_filter_and_bound_reference() {
    let (r, id, o) = setup();
    let got = r.sizing_inputs(&id, o.clone()).unwrap();
    let p = r.price_filter.unwrap();
    let l = r.lot_size.unwrap();
    let n = r.notional.unwrap();
    assert_eq!(
        (got.price.min, got.price.max, got.price.tick),
        (p.min_price, p.max_price, p.tick_size)
    );
    assert_eq!((got.lot.min, got.lot.max, got.lot.step), (l.min_qty, l.max_qty, l.step_size));
    assert_eq!(
        (
            got.notional.min,
            got.notional.max,
            got.notional.apply_min,
            got.notional.apply_max,
            got.notional.average_minutes
        ),
        (
            n.min_notional,
            n.max_notional,
            n.apply_min_to_market,
            n.apply_max_to_market,
            n.avg_price_mins
        )
    );
    let m = r.market_lot_size.unwrap();
    let gm = got.market_lot.unwrap();
    assert_eq!((gm.min, gm.max, gm.step), (m.min_qty, m.max_qty, m.step_size));
    assert_eq!(got.observation, o);
}
#[test]
fn projection_rejects_incomplete_unsupported_unbounded_or_wrong_account() {
    for field in 0..9 {
        let (mut r, id, mut o) = setup();
        match field {
            0 => r.status = "HALT".into(),
            1 => r.order_types.retain(|s| s != "STOP_LOSS"),
            2 => r.order_types.push("FUTURE_UNKNOWN".into()),
            3 => {
                r.order_types = vec!["LIMIT".into(); 33];
                r.order_types[0] = "MARKET".into();
                r.order_types[1] = "STOP_LOSS".into();
            },
            4 => r.price_filter = None,
            5 => r.notional = None,
            6 => r.quote_asset = "USDC".into(),
            7 => o.observation_id = "00000000-0000-0000-0000-000000000000".parse().unwrap(),
            _ => r.symbol = "OTHER".into(),
        };
        assert!(r.sizing_inputs(&id, o).is_err(), "field{field}");
    }
}

#[test]
fn a_spot_observation_cannot_be_rebound_to_a_different_venue() {
    let (r, id, mut o) = setup();
    let other = CostIdentity::new(
        id.tenant(),
        VenueId::new(VenueId::BINANCE_USDM),
        id.scope().into(),
        id.uid(),
        id.symbol().clone(),
    )
    .unwrap();
    o.binding.account = other.clone();
    assert!(matches!(r.sizing_inputs(&other, o), Err(SizingError::Identity)));
}
