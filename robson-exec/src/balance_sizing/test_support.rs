use chrono::{NaiveDate, TimeZone, Utc};
use robson_domain::{
    ExecutableStopPlan, Price, RiskConfig, Side, StopDistanceBounds, StopPolicy, Symbol,
};
use rust_decimal::Decimal as D;
use rust_decimal_macros::dec;
use uuid::Uuid;

use super::inputs::*;
use crate::{
    balance_costs::{sizing_terms, CostIdentity, CostPolicy},
    ports::CandleInterval,
    venue::VenueId,
};

pub(super) fn fixture() -> SizingInputs {
    let id = CostIdentity::new(
        Uuid::from_u128(1),
        VenueId::new(VenueId::BINANCE_SPOT),
        "scope".into(),
        7,
        Symbol::new("SOL".into(), "USDC".into()).unwrap(),
    )
    .unwrap();
    let binding = PreparedBinding {
        account: id.clone(),
        preparation_id: Uuid::from_u128(2),
        accounting_version: 5,
    };
    let obs = ObservationRef {
        binding: binding.clone(),
        observation_id: Uuid::from_u128(3),
        source_generation: 9,
        original_as_of: Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap(),
    };
    // Labeled analyzer fixture: second15m support50, entry reference100.
    SizingInputs {
        binding,
        basis: BasisInput {
            amount: dec!(2000),
            month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            observation: obs.clone(),
        },
        cash: CashInput {
            free: dec!(2000),
            locked: dec!(9000),
            quote_asset: "USDC".into(),
            observation: obs.clone(),
        },
        price: PriceInput {
            value: dec!(100),
            observation: obs.clone(),
        },
        chart: ChartInput {
            support: dec!(50),
            interval: CandleInterval::FifteenMinutes,
            guard: None,
            plan: ExecutableStopPlan {
                policy: StopPolicy::ExecutableSpan,
                side: Side::Long,
                basis: Price::new(dec!(50)).unwrap(),
                guard_bound: false,
                cap_basis_distance: Some(dec!(50)),
                executable_span: Some(dec!(50)),
                tick_size: Some(dec!(0.1)),
                effective_buffer: D::ZERO,
                trigger: Price::new(dec!(50)).unwrap(),
            },
            observation: obs.clone(),
        },
        rules: SizingRules {
            price: PriceLimits {
                min: D::ZERO,
                max: dec!(1000),
                tick: dec!(0.1),
            },
            lot: LotLimits {
                min: dec!(0.1),
                max: dec!(10000),
                step: dec!(0.1),
            },
            market_lot: Some(LotLimits {
                min: D::ZERO,
                max: dec!(10000),
                step: dec!(0.1),
            }),
            notional: NotionalLimits {
                min: dec!(1),
                max: dec!(1000000),
                apply_min: true,
                apply_max: true,
                average_minutes: 5,
            },
            trading: true,
            market: true,
            stop_loss: true,
            observation: obs.clone(),
        },
        filter_price: Some(FilterPriceInput {
            value: dec!(110),
            source: FilterPriceSource::Average { minutes: 5 },
            observation: obs,
            rules_generation: 9,
        }),
        terms: sizing_terms(&id, "0", "0"),
        policy: CostPolicy::parse("0", Some("0"), Some("1000000")).unwrap(),
        risk: RiskConfig::new(dec!(999999))
            .unwrap()
            .with_margin_headroom(D::ZERO)
            .unwrap()
            .with_stop_distance_bounds(StopDistanceBounds::new(dec!(1), dec!(10000)).unwrap()),
    }
}
