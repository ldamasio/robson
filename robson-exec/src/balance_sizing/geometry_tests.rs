use robson_domain::{Price, Side, StopDistanceBounds, StopPolicy};
use rust_decimal::Decimal as D;
use rust_decimal_macros::dec;

use super::{
    arithmetic::{Arithmetic, TRACE},
    bridge::native,
    geometry,
    inputs::*,
    test_support::fixture,
};
use crate::{balance_costs::CostPolicy, ports::CandleInterval};

fn check(i: &SizingInputs) -> Result<D, SizingError> {
    let config = geometry::validate(i)?;
    let mut a = Arithmetic::default();
    geometry::lot(&mut a, i)?;
    let result = geometry::geometry(&mut a, i, &config)?;
    native(&mut a, &result)
}
#[test]
fn one_preparation_frame_has_exact_32_call_geometry_prefix() {
    TRACE.with_borrow_mut(Vec::clear);
    let i = fixture();
    let config = geometry::validate(&i).unwrap();
    let mut a = Arithmetic::default();
    let lot = geometry::lot(&mut a, &i).unwrap();
    let result = geometry::geometry(&mut a, &i, &config).unwrap();
    assert_eq!(a.operations(), 32);
    assert_eq!(lot.step, dec!(0.1));
    let trace = TRACE.with_borrow(|t| t.clone());
    assert_eq!(trace.iter().filter(|(k, _)| *k == "call").count(), 32);
    assert!(trace.iter().filter(|(k, _)| *k == "bits").all(|(_, bits)| *bits <= 492));
    assert!(trace.iter().filter(|(k, _)| *k == "export_div").count() <= 4 * 29);
    assert_eq!(native(&mut a, &result).unwrap(), dec!(50));
    assert_eq!(a.operations(), 33, "caller continues same frame");
    assert_eq!(config.capital(), dec!(2000));
    assert_eq!(config.taker_fee_rate(), D::ZERO);
    assert_eq!(config.margin_headroom_bps(), i.risk.margin_headroom_bps());
}
#[test]
fn geometry_options_and_nonbinding_cap_identity_are_all_checked() {
    let i = fixture();
    assert_eq!(check(&i).unwrap(), dec!(50));
    for field in 0..11 {
        let mut bad = i.clone();
        match field {
            0 => bad.chart.plan.cap_basis_distance = Some(dec!(49)),
            1 => bad.chart.plan.cap_basis_distance = None,
            2 => bad.chart.plan.executable_span = None,
            3 => bad.chart.plan.tick_size = None,
            4 => bad.chart.plan.effective_buffer = dec!(0.1),
            5 => bad.chart.plan.guard_bound = true,
            6 => bad.chart.interval = CandleInterval::OneMinute,
            7 => bad.chart.plan.side = Side::Short,
            8 => bad.chart.plan.basis = Price::new(dec!(49)).unwrap(),
            9 => bad.chart.guard = Some(D::ZERO),
            _ => bad.chart.plan.trigger = Price::new(dec!(49)).unwrap(),
        };
        assert_eq!(check(&bad).unwrap_err(), SizingError::Geometry, "field{field}");
    }
    let mut legacy = i.clone();
    legacy.chart.plan.policy = StopPolicy::LegacyUncapped;
    legacy.chart.plan.cap_basis_distance = None;
    legacy.chart.plan.executable_span = None;
    legacy.chart.plan.tick_size = None;
    assert!(check(&legacy).is_ok());
    legacy.chart.plan.cap_basis_distance = Some(dec!(50));
    assert_eq!(check(&legacy).unwrap_err(), SizingError::Geometry);
}
#[test]
fn identity_cash_and_numeric_configuration_have_no_fallback() {
    for field in 0..12 {
        let mut i = fixture();
        let expected = match field {
            0 => {
                i.cash.quote_asset = "USDT".into();
                SizingError::Identity
            },
            1 => {
                i.cash.observation.binding.accounting_version = 99;
                SizingError::Identity
            },
            2 => {
                i.binding.preparation_id = uuid::Uuid::nil();
                SizingError::Identity
            },
            3 => {
                i.basis.amount = D::ZERO;
                SizingError::Config
            },
            4 => {
                i.cash.locked = -D::ONE;
                SizingError::Config
            },
            5 => {
                i.cash.free = -D::ONE;
                SizingError::Config
            },
            6 => {
                i.policy = CostPolicy::parse("1", None, None).unwrap();
                SizingError::Config
            },
            7 => {
                i.rules.observation.observation_id = uuid::Uuid::nil();
                SizingError::Identity
            },
            8 => {
                i.price.value = D::ZERO;
                SizingError::Config
            },
            9 => {
                i.basis.month = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
                SizingError::Config
            },
            10 => {
                i.chart.observation.binding.preparation_id = uuid::Uuid::from_u128(99);
                SizingError::Identity
            },
            _ => {
                i.basis.observation.binding.account = crate::balance_costs::CostIdentity::new(
                    uuid::Uuid::from_u128(1),
                    crate::venue::VenueId::new(crate::venue::VenueId::BINANCE_SPOT),
                    "other".into(),
                    7,
                    i.binding.account.symbol().clone(),
                )
                .unwrap();
                SizingError::Identity
            },
        };
        assert_eq!(check(&i).unwrap_err(), expected, "field{field}");
    }
    let mut i = fixture();
    i.cash.free = D::ZERO;
    assert!(check(&i).is_ok(), "geometry is not a financial capacity gate");
    i.cash.locked = dec!(999999);
    assert!(check(&i).is_ok());
    for (field, value) in [("stop_buffer_bps", "101"), ("margin_headroom_bps", "1001")] {
        let mut bad = fixture();
        let mut raw = serde_json::to_value(bad.risk).unwrap();
        raw[field] = serde_json::Value::String(value.into());
        bad.risk = serde_json::from_value(raw).unwrap();
        assert_eq!(check(&bad).unwrap_err(), SizingError::Config);
    }
}
#[test]
fn metadata_limits_and_nested_grids_are_exact() {
    for field in 0..14 {
        let mut i = fixture();
        match field {
            0 => i.rules.trading = false,
            1 => i.rules.market = false,
            2 => i.rules.stop_loss = false,
            3 => i.rules.price.tick = D::ZERO,
            4 => i.rules.price.min = -D::ONE,
            5 => i.rules.price.max = -D::ONE,
            6 => i.rules.price.max = dec!(0.01),
            7 => i.rules.lot.max = D::ZERO,
            8 => i.rules.lot.min = dec!(10001),
            9 => i.rules.lot.step = -D::ONE,
            10 => i.rules.notional.min = D::ZERO,
            11 => i.rules.notional.max = dec!(0.1),
            12 => i.rules.market_lot.as_mut().unwrap().step = dec!(0.03),
            _ => i.rules.price.min = dec!(0.01),
        };
        assert!(
            matches!(check(&i), Err(SizingError::Rules | SizingError::Geometry)),
            "field{field}"
        );
    }
    let mut i = fixture();
    i.rules.price.max = D::ZERO;
    i.rules.market_lot.as_mut().unwrap().step = D::ZERO;
    assert!(check(&i).is_ok());
    i.rules.market_lot.as_mut().unwrap().max = D::ZERO;
    assert_eq!(check(&i).unwrap_err(), SizingError::Rules);
}
#[test]
fn filter_observation_is_bound_but_never_qualified_here() {
    for field in 0..7 {
        let mut bad = fixture();
        match field {
            0 => bad.filter_price = None,
            1 => bad.filter_price.as_mut().unwrap().rules_generation = 10,
            2 => bad.filter_price.as_mut().unwrap().source = FilterPriceSource::LastPrice,
            3 => bad.filter_price.as_mut().unwrap().observation.binding.accounting_version = 99,
            4 => bad.filter_price.as_mut().unwrap().value = D::ZERO,
            5 => bad.filter_price.as_mut().unwrap().observation.observation_id = uuid::Uuid::nil(),
            _ => {
                bad.filter_price.as_mut().unwrap().source =
                    FilterPriceSource::Average { minutes: 0 }
            },
        };
        assert_eq!(check(&bad).unwrap_err(), SizingError::FilterPrice);
    }
    let mut i = fixture();
    i.filter_price.as_mut().unwrap().source = FilterPriceSource::VenueReference;
    assert!(check(&i).is_ok());
    i.rules.notional.average_minutes = 0;
    i.filter_price.as_mut().unwrap().source = FilterPriceSource::LastPrice;
    assert!(check(&i).is_ok());
    i.rules.notional.apply_min = false;
    i.rules.notional.apply_max = false;
    i.filter_price = None;
    assert!(check(&i).is_ok());
    // Generations of different feeds are not a global epoch.
    i.cash.observation.source_generation = 999;
    assert!(check(&i).is_ok());
}
#[test]
fn chart_and_final_trigger_bounds_are_independent() {
    let mut chart = fixture();
    chart.risk = chart
        .risk
        .with_stop_distance_bounds(StopDistanceBounds::new(dec!(4990), dec!(5010)).unwrap());
    chart.chart.support = dec!(50.2);
    chart.chart.guard = Some(dec!(50));
    chart.chart.plan.guard_bound = true;
    assert_eq!(
        check(&chart).unwrap_err(),
        SizingError::Geometry,
        "chart invalid despite valid basis and trigger"
    );
    let mut final_stage = fixture();
    final_stage.risk = final_stage
        .risk
        .with_stop_distance_bounds(StopDistanceBounds::new(dec!(4990), dec!(5010)).unwrap())
        .with_stop_buffer(dec!(100))
        .unwrap();
    final_stage.chart.plan.effective_buffer = dec!(0.5);
    final_stage.chart.plan.trigger = Price::new(dec!(49.5)).unwrap();
    final_stage.chart.plan.executable_span = Some(dec!(50.5));
    assert_eq!(
        check(&final_stage).unwrap_err(),
        SizingError::Geometry,
        "valid chart and basis but too distant trigger"
    );
}
#[test]
fn python_fraction_geometry_extremes_and_policy_specific_exports() {
    let rows: serde_json::Value =
        serde_json::from_str(include_str!("testdata/geometry_oracle.json")).unwrap();
    for row in rows.as_array().unwrap() {
        let number = |key: &str| row[key].as_str().unwrap().parse::<D>().unwrap();
        let mut i = fixture();
        i.price.value = number("price");
        i.chart.support = number("chart");
        i.chart.guard = row["guard"].as_str().map(|s| s.parse().unwrap());
        i.rules.price.tick = number("tick");
        i.rules.price.max = D::MAX;
        i.risk = i.risk.with_stop_buffer(number("buffer")).unwrap();
        if let Some(gap) = row["gap"].as_str() {
            i.terms = crate::balance_costs::sizing_terms(&i.binding.account, "0", gap);
            i.policy = CostPolicy::parse(gap, Some("0"), None).unwrap();
        }
        let plan = &mut i.chart.plan;
        plan.policy = if row["policy"] == "span" {
            StopPolicy::ExecutableSpan
        } else {
            StopPolicy::LegacyUncapped
        };
        if let Some(e) = row.get("expected") {
            let d = |key: &str| e[key].as_str().unwrap().parse::<D>().unwrap();
            plan.basis = Price::new(d("basis")).unwrap();
            plan.effective_buffer = d("effective_buffer");
            plan.trigger = Price::new(d("trigger")).unwrap();
            plan.guard_bound = e["guard_bound"].as_bool().unwrap();
            plan.cap_basis_distance = e["cap_basis"].as_str().map(|s| s.parse().unwrap());
            plan.executable_span = e["span"].as_str().map(|s| s.parse().unwrap());
            plan.tick_size =
                (plan.policy == StopPolicy::ExecutableSpan).then_some(i.rules.price.tick);
            let config = geometry::validate(&i).unwrap();
            let mut a = Arithmetic::default();
            let result = geometry::geometry(&mut a, &i, &config).unwrap();
            assert_eq!(native(&mut a, &result).unwrap(), d("trigger"), "{row}");
        } else {
            // Supply structurally matching basis; exact derivation fails before
            // comparing unavailable required outputs, as the independent oracle says.
            plan.basis =
                Price::new(i.chart.guard.map_or(i.chart.support, |g| g.min(i.chart.support)))
                    .unwrap();
            plan.guard_bound = plan.basis.as_decimal() != i.chart.support;
            let config = geometry::validate(&i).unwrap();
            assert_eq!(
                geometry::geometry(&mut Arithmetic::default(), &i, &config).unwrap_err(),
                SizingError::Unrepresentable,
                "{row}"
            );
        }
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(96))]
    #[test]
    fn bounded_i128_geometry_oracle_and_canonical_safe_domain_agree(buffer in 0_i128..101,tick in 1_i128..11,guard in proptest::bool::ANY) {
        // Two independently recorded chart/guard price points in cents.
        // All integer products <=10^8; no production arithmetic in oracle.
        let(p,c,g)=(10000_i128,5000_i128,4773_i128);
        let basis=if guard{g}else{c};let distance=p-basis;
        let buffer_n=(basis*buffer).min(distance*2500); // common denominator10^6
        let raw_n=basis*10000-buffer_n;
        let trigger_c=(raw_n/(tick*10000))*tick;
        let mut i=fixture();i.chart.guard=guard.then_some(D::from_i128_with_scale(g,2));i.rules.price.tick=D::from_i128_with_scale(tick,2);i.risk=i.risk.with_stop_buffer(D::from_i128_with_scale(buffer,0)).unwrap();
        let expected=robson_domain::ExecutableStopPlan{policy:StopPolicy::ExecutableSpan,side:Side::Long,basis:Price::new(D::from_i128_with_scale(basis,2)).unwrap(),guard_bound:guard,cap_basis_distance:Some(D::from_i128_with_scale(distance,2)),executable_span:Some(D::from_i128_with_scale(p-trigger_c,2)),tick_size:Some(i.rules.price.tick),effective_buffer:D::from_i128_with_scale(buffer_n,6),trigger:Price::new(D::from_i128_with_scale(trigger_c,2)).unwrap()};
        i.chart.plan=expected;
        proptest::prop_assert_eq!(check(&i).unwrap(),expected.trigger.as_decimal());
        let rules=robson_domain::SymbolTradingRules::new(i.binding.account.symbol().clone(),i.rules.price.tick,D::ZERO,dec!(0.1),dec!(0.1),dec!(10000),D::ONE,8,8).unwrap();
        // Only these proven small operands reach the legacy shared builder.
        let shared=robson_domain::build_executable_stop_plan(robson_domain::StopPlanInputs{policy:StopPolicy::ExecutableSpan,side:Side::Long,technical_stop:Price::new(i.chart.support).unwrap(),guard:i.chart.guard.map(|g|Price::new(g).unwrap()),entry_reference:Some(Price::new(i.price.value).unwrap()),technical_span:Some(dec!(50)),stop_buffer_bps:i.risk.stop_buffer_bps(),executable_span_source:robson_domain::ExecutableSpanSource::Admission,rules:Some(&rules)}).unwrap();
        proptest::prop_assert_eq!(shared,expected);
    }
}
