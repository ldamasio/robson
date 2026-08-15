//! ExecutableStopPlan property tests (issue #154 deliverable 7).
//!
//! Money invariants of the single stop resolver, proven over randomized
//! inputs for Long and Short:
//!
//! 1. every ExecutableSpan trigger is tick-aligned and quantized AWAY from the
//!    position, never past one tick;
//! 2. the effective buffer never exceeds the configured buffer nor 0.25 x span;
//! 3. the engine's soft-exit boundary and trailing-advance replacement use the
//!    live plan trigger; fill-time protection is pinned separately with
//!    mismatched admission/live tick-size tests;
//! 4. `planned_risk >= modeled adverse-fill loss` and `planned_risk <= capital
//!    x 1%`;
//! 5. a zero buffer under the legacy policy reproduces the historical
//!    derivation bit for bit.
//!
//! The fixture tick (0.10) deliberately differs from `10^-pricePrecision`
//! (0.01): the real BTCUSDT USD-M filters, where precision-driven rounding
//! produces off-grid prices.

use chrono::Utc;
use proptest::prelude::*;
use robson_domain::{
    build_executable_stop_plan, size_entry, value_objects::effective_stop_price, DetectorSignal,
    Event, ExecutableSpanSource, Position, PositionState, Price, Quantity, RiskConfig, Side,
    StopPlanInputs, StopPolicy, Symbol, SymbolTradingRules, TechnicalStopDistance,
};
use robson_engine::{Engine, EngineAction, EngineError, MarketData};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

const TICK: Decimal = Decimal::from_parts(10, 0, 0, false, 2); // 0.10
const MIN_PRICE: Decimal = Decimal::from_parts(55680, 0, 0, false, 2); // 556.80

fn btcusdt_rules() -> SymbolTradingRules {
    SymbolTradingRules::new(
        Symbol::from_pair("BTCUSDT").unwrap(),
        TICK,
        MIN_PRICE,
        dec!(0.001),
        dec!(0.001),
        dec!(1000),
        Decimal::ZERO,
        2,
        3,
    )
    .unwrap()
}

fn is_tick_aligned(rules: &SymbolTradingRules, price: Decimal) -> bool {
    let steps = (price - rules.min_price()) / rules.tick_size();
    steps == steps.trunc()
}

fn side_of(long: bool) -> Side {
    if long {
        Side::Long
    } else {
        Side::Short
    }
}

/// Active ExecutableSpan position fixture with admission evidence persisted.
fn executable_active_position(
    side: Side,
    entry: Decimal,
    trailing_stop: Decimal,
    buffer_bps: Decimal,
) -> Position {
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let mut position = Position::new_with_stop_policy(
        Uuid::now_v7(),
        symbol,
        side,
        StopPolicy::ExecutableSpan,
        Some(buffer_bps),
    );
    let entry_price = Price::new(entry).unwrap();
    let stop_price = Price::new(trailing_stop).unwrap();
    position.entry_price = Some(entry_price);
    position.tech_stop_distance =
        Some(TechnicalStopDistance::from_entry_and_stop(entry_price, stop_price));
    let rules = btcusdt_rules();
    let admission_plan = build_executable_stop_plan(StopPlanInputs {
        policy: StopPolicy::ExecutableSpan,
        side,
        technical_stop: stop_price,
        guard: None,
        entry_reference: Some(entry_price),
        technical_span: Some((entry - trailing_stop).abs()),
        stop_buffer_bps: buffer_bps,
        executable_span_source: ExecutableSpanSource::Admission,
        rules: Some(&rules),
    })
    .unwrap();
    position.initial_executable_stop = Some(admission_plan.trigger);
    position.executable_span = admission_plan.executable_span;
    position.cap_basis_distance = admission_plan.cap_basis_distance;
    position.tick_size_at_admission = admission_plan.tick_size;
    position.quantity = Quantity::new(dec!(0.01)).unwrap();
    position.state = PositionState::Active {
        current_price: entry_price,
        trailing_stop: stop_price,
        favorable_extreme: entry_price,
        extreme_at: Utc::now(),
        insurance_stop_id: Some("INS-1".to_string()),
        invalidation_guard_level: None,
        last_emitted_stop: None,
    };
    position
}

proptest! {
    /// Property 1 + 2: executable-span triggers are tick-aligned, adverse, within one
    /// tick of the raw executable, and the buffer respects both caps.
    #[test]
    fn executable_span_trigger_is_tick_aligned_and_adverse(
        stop_cents in 6_000_000u64..7_000_000, // 60_000.00 .. 70_000.00
        span_ticks in 1u32..5_000,             // 0.10 .. 500.00
        buffer_bps in 0u32..=100,
        long in proptest::bool::ANY,
    ) {
        let rules = btcusdt_rules();
        let side = side_of(long);
        let technical = Decimal::new(stop_cents as i64, 2);
        let span = Decimal::from(span_ticks) * TICK;
        let buffer_bps = Decimal::from(buffer_bps);

        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side,
            technical_stop: Price::new(technical).unwrap(),
            guard: None,
            entry_reference: Some(Price::new(match side {
                Side::Long => technical + span,
                Side::Short => technical - span,
            }).unwrap()),
            technical_span: Some(span),
            stop_buffer_bps: buffer_bps,
            executable_span_source: ExecutableSpanSource::Admission,
            rules: Some(&rules),
        }).unwrap();

        // Tick alignment on the real grid (anchored at minPrice).
        prop_assert!(is_tick_aligned(&rules, plan.trigger.as_decimal()));

        // Buffer caps: never above the configured offset, never above
        // 0.25 x span.
        let configured_offset = technical * buffer_bps / dec!(10000);
        prop_assert!(plan.effective_buffer <= configured_offset);
        prop_assert!(plan.effective_buffer <= span * dec!(0.25));

        // Quantization is adverse and bounded by one tick.
        let raw = match side {
            Side::Long => technical - plan.effective_buffer,
            Side::Short => technical + plan.effective_buffer,
        };
        let trigger = plan.trigger.as_decimal();
        match side {
            Side::Long => {
                prop_assert!(trigger <= raw);
                prop_assert!(raw - trigger < TICK);
            },
            Side::Short => {
                prop_assert!(trigger >= raw);
                prop_assert!(trigger - raw < TICK);
            },
        }
    }

    /// Property 3: the soft-exit boundary and trailing-advance replacement
    /// both sit exactly on the live plan trigger.
    #[test]
    fn executable_span_surfaces_agree_on_the_single_trigger(
        stop_ticks in 600_000u64..640_000,     // trailing stop 60_..64_k on grid
        span_ticks in 3_000u32..20_000,        // span 300.00 .. 2_000.00
        buffer_bps in 0u32..=100,
        long in proptest::bool::ANY,
    ) {
        let rules = btcusdt_rules();
        let side = side_of(long);
        let trailing = MIN_PRICE + Decimal::from(stop_ticks) * TICK;
        let span = Decimal::from(span_ticks) * TICK;
        let entry = match side {
            Side::Long => trailing + span,
            Side::Short => trailing - span,
        };
        prop_assume!(entry > Decimal::ZERO);
        let buffer_bps = Decimal::from(buffer_bps);

        let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
        let position = executable_active_position(side, entry, trailing, buffer_bps);

        // Surface 1: the plan itself (also what the API and startup
        // recovery derive through Engine::stop_plan).
        let plan = engine
            .stop_plan(&position, Price::new(trailing).unwrap(), None, Some(&rules))
            .unwrap();
        let trigger = plan.trigger.as_decimal();

        // Surface 2: soft exit fires AT the trigger and not one tick before.
        let market_at = MarketData::new(position.symbol.clone(), plan.trigger);
        let decision_at = engine
            .process_active_position_with_rules(&position, &market_at, Some(&rules))
            .unwrap();
        prop_assert!(
            decision_at.actions.iter().any(|a| matches!(a, EngineAction::TriggerExit { .. })),
            "exit must trigger exactly at the plan trigger {trigger}"
        );
        let one_tick_inside = match side {
            Side::Long => trigger + TICK,
            Side::Short => trigger - TICK,
        };
        prop_assume!(one_tick_inside > Decimal::ZERO);
        let market_inside =
            MarketData::new(position.symbol.clone(), Price::new(one_tick_inside).unwrap());
        let decision_inside = engine
            .process_active_position_with_rules(&position, &market_inside, Some(&rules))
            .unwrap();
        prop_assert!(
            !decision_inside
                .actions
                .iter()
                .any(|a| matches!(a, EngineAction::TriggerExit { .. })),
            "exit must NOT trigger one tick inside the trigger"
        );

        // Surface 3: trailing-advance replacement price. Advance by one full
        // span; the replacement must sit on the plan trigger for the NEW
        // trailing stop.
        let persisted_span = position.executable_span.unwrap();
        let advance_price = match side {
            Side::Long => entry + persisted_span,
            Side::Short => entry - persisted_span,
        };
        prop_assume!(advance_price > Decimal::ZERO);
        let market_advance =
            MarketData::new(position.symbol.clone(), Price::new(advance_price).unwrap());
        let advance_decision = engine
            .process_active_position_with_rules(&position, &market_advance, Some(&rules))
            .unwrap();
        let (new_stop, replaced_price) = advance_decision
            .actions
            .iter()
            .find_map(|a| match a {
                EngineAction::ReplaceInsuranceStop { new_stop_price, .. } => {
                    advance_decision.actions.iter().find_map(|b| match b {
                        EngineAction::UpdateTrailingStop { new_stop, .. } => {
                            Some((*new_stop, *new_stop_price))
                        },
                        _ => None,
                    })
                },
                _ => None,
            })
            .expect("a full-span advance must replace the insurance stop");
        let expected = engine
            .stop_plan(&position, new_stop, None, Some(&rules))
            .unwrap()
            .trigger;
        prop_assert_eq!(replaced_price, expected);
    }

    /// Property 4: planned risk covers the modeled adverse-fill loss and
    /// never exceeds the 1% ceiling.
    #[test]
    fn planned_risk_covers_modeled_loss_and_respects_cap(
        entry_cents in 5_000_000u64..8_000_000, // 50_000.00 .. 80_000.00
        distance_bps in 30u32..800,             // 0.3% .. 8%
        buffer_bps in 0u32..=100,
        long in proptest::bool::ANY,
    ) {
        let rules = btcusdt_rules();
        let side = side_of(long);
        let entry = Decimal::new(entry_cents as i64, 2);
        let distance = entry * Decimal::from(distance_bps) / dec!(10000);
        let technical = match side {
            Side::Long => entry - distance,
            Side::Short => entry + distance,
        };
        prop_assume!(technical > Decimal::ZERO);
        let buffer_bps = Decimal::from(buffer_bps);

        let config = RiskConfig::new(dec!(100000)).unwrap();
        let entry_price = Price::new(entry).unwrap();
        let technical_price = Price::new(technical).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry_price, technical_price);

        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side,
            technical_stop: technical_price,
            guard: None,
            entry_reference: Some(entry_price),
            technical_span: Some(tech_stop.span()),
            stop_buffer_bps: buffer_bps,
            executable_span_source: ExecutableSpanSource::Admission,
            rules: Some(&rules),
        }).unwrap();

        let sized = match size_entry(&config, &entry_price, &tech_stop, &plan, None, Some(&rules)) {
            Ok(sized) => sized,
            // The buffer can legitimately push the trigger past the max
            // bound (typed rejection) or quantize the qty below the lot
            // minimum; both are governed denials, not property violations.
            Err(_) => return Ok(()),
        };

        // Ceiling: never above capital x 1%.
        prop_assert!(sized.planned_risk <= config.max_risk_amount());

        // Coverage: planned risk >= independently modeled worst loss
        // (distance to the adverse fill plus both fees, from the
        // tick-quantized trigger).
        let gap = plan.trigger.as_decimal() * config.stop_gap_bps() / dec!(10000);
        let adverse = match side {
            Side::Long => plan.trigger.as_decimal() - gap,
            Side::Short => plan.trigger.as_decimal() + gap,
        };
        let per_unit_modeled = match side {
            Side::Long => entry - adverse,
            Side::Short => adverse - entry,
        } + config.taker_fee_rate() * (entry + adverse);
        let modeled = sized.quantity.as_decimal() * per_unit_modeled;
        prop_assert!(
            sized.planned_risk >= modeled,
            "planned {} < modeled {}",
            sized.planned_risk,
            modeled
        );
    }

    /// Property 5: a zero buffer under the legacy policy reproduces the
    /// historical derivation bit for bit (trigger == technical stop ==
    /// pre-#154 helper output).
    #[test]
    fn legacy_zero_buffer_is_bit_for_bit_historical(
        stop_cents in 1_000u64..10_000_000,
        long in proptest::bool::ANY,
    ) {
        let side = side_of(long);
        let technical = Price::new(Decimal::new(stop_cents as i64, 2)).unwrap();

        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::LegacyUncapped,
            side,
            technical_stop: technical,
            guard: None,
            entry_reference: None,
            technical_span: None,
            stop_buffer_bps: Decimal::ZERO,
            executable_span_source: ExecutableSpanSource::Admission,
            rules: None,
        }).unwrap();

        prop_assert_eq!(plan.trigger, technical);
        prop_assert_eq!(plan.trigger, effective_stop_price(side, technical, Decimal::ZERO));
    }
}

#[test]
fn fill_uses_live_tick_size_and_emits_durable_drift_evidence() {
    let admission_rules = btcusdt_rules();
    let live_rules = SymbolTradingRules::new(
        Symbol::from_pair("BTCUSDT").unwrap(),
        dec!(0.25),
        MIN_PRICE,
        dec!(0.001),
        dec!(0.001),
        dec!(1000),
        Decimal::ZERO,
        2,
        3,
    )
    .unwrap();
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let mut entering = executable_active_position(Side::Long, dec!(62000), dec!(61000), dec!(10));
    let persisted_trigger = entering.initial_executable_stop.unwrap();
    entering.state = PositionState::Entering {
        entry_order_id: Uuid::now_v7(),
        expected_entry: Price::new(dec!(62000)).unwrap(),
        signal_id: Uuid::now_v7(),
    };
    let live_plan = engine
        .stop_plan(&entering, Price::new(dec!(61000)).unwrap(), None, Some(&live_rules))
        .unwrap();
    assert_ne!(live_plan.trigger, persisted_trigger);

    let decision = engine
        .process_entry_fill_with_rules(
            &entering,
            Price::new(dec!(62000)).unwrap(),
            Quantity::new(dec!(0.01)).unwrap(),
            Decimal::ZERO,
            Utc::now(),
            None,
            None,
            Some(&live_rules),
        )
        .unwrap();

    let placed = decision.actions.iter().find_map(|action| match action {
        EngineAction::PlaceInsuranceStop { stop_price, .. } => Some(*stop_price),
        _ => None,
    });
    assert_eq!(placed, Some(live_plan.trigger), "live trigger must win at fill");
    assert!(decision.actions.iter().any(|action| matches!(
        action,
        EngineAction::EmitEvent(Event::ExecutableStopPlanDriftDetected {
            persisted_trigger: event_persisted,
            live_trigger,
            tick_size_at_admission: Some(admission_tick),
            live_tick_size: Some(live_tick),
            severity,
            ..
        }) if *event_persisted == persisted_trigger
            && *live_trigger == live_plan.trigger
            && *admission_tick == admission_rules.tick_size()
            && *live_tick == live_rules.tick_size()
            && severity == "critical"
    )));
}

#[test]
fn fill_without_persisted_trigger_still_places_fallback_insurance() {
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let mut entering = executable_active_position(Side::Long, dec!(62000), dec!(61000), dec!(10));
    entering.initial_executable_stop = None;
    entering.state = PositionState::Entering {
        entry_order_id: Uuid::now_v7(),
        expected_entry: Price::new(dec!(62000)).unwrap(),
        signal_id: Uuid::now_v7(),
    };

    let decision = engine
        .process_entry_fill_with_rules(
            &entering,
            Price::new(dec!(62000)).unwrap(),
            Quantity::new(dec!(0.01)).unwrap(),
            Decimal::ZERO,
            Utc::now(),
            None,
            None,
            None,
        )
        .expect("a real fill must never fail before fallback protection");

    assert!(decision.actions.iter().any(|action| matches!(
        action,
        EngineAction::PlaceInsuranceStop { stop_price, .. }
            if *stop_price == Price::new(dec!(61000)).unwrap()
    )));
    assert!(decision.actions.iter().any(|action| matches!(
        action,
        EngineAction::EmitEvent(Event::EntryFillProtectionFallback {
            fallback_source,
            fallback_trigger,
            persisted_trigger: None,
            requires_operator_review: true,
            ..
        }) if fallback_source == "initial_trailing_stop"
            && *fallback_trigger == Price::new(dec!(61000)).unwrap()
    )));
}

#[test]
fn fill_resolution_failure_prefers_persisted_trigger_fallback() {
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let mut entering = executable_active_position(Side::Long, dec!(62000), dec!(61000), dec!(10));
    let persisted_trigger = entering.initial_executable_stop.unwrap();
    entering.state = PositionState::Entering {
        entry_order_id: Uuid::now_v7(),
        expected_entry: Price::new(dec!(62000)).unwrap(),
        signal_id: Uuid::now_v7(),
    };

    let decision = engine
        .process_entry_fill_with_rules(
            &entering,
            Price::new(dec!(62000)).unwrap(),
            Quantity::new(dec!(0.01)).unwrap(),
            Decimal::ZERO,
            Utc::now(),
            None,
            None,
            None,
        )
        .expect("persisted protection must survive a live-rules outage");

    assert!(decision.actions.iter().any(|action| matches!(
        action,
        EngineAction::PlaceInsuranceStop { stop_price, .. }
            if *stop_price == persisted_trigger
    )));
    assert!(decision.actions.iter().any(|action| matches!(
        action,
        EngineAction::EmitEvent(Event::EntryFillProtectionFallback {
            fallback_source,
            fallback_trigger,
            persisted_trigger: Some(event_persisted),
            requires_operator_review: true,
            ..
        }) if fallback_source == "persisted_initial_executable_stop"
            && *fallback_trigger == persisted_trigger
            && *event_persisted == persisted_trigger
    )));
}

/// Guard binding + cap binding, including release on the first trailing
/// advance (issue #154 deliverable 7, deterministic).
#[test]
fn executable_span_guard_and_cap_bind_and_release_on_first_advance() {
    let rules = btcusdt_rules();
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());

    // Short: entry 61_909.10, technical 62_214.70, guard 62_386.70 binds.
    let entry = dec!(61909.10);
    let technical = dec!(62214.70);
    let guard = Price::new(dec!(62386.70)).unwrap();
    let mut position = executable_active_position(Side::Short, entry, technical, dec!(100));
    let admission_plan = build_executable_stop_plan(StopPlanInputs {
        policy: StopPolicy::ExecutableSpan,
        side: Side::Short,
        technical_stop: Price::new(technical).unwrap(),
        guard: Some(guard),
        entry_reference: Some(Price::new(entry).unwrap()),
        technical_span: Some(dec!(305.60)),
        stop_buffer_bps: dec!(100),
        executable_span_source: ExecutableSpanSource::Admission,
        rules: Some(&rules),
    })
    .unwrap();
    position.initial_executable_stop = Some(admission_plan.trigger);
    position.executable_span = admission_plan.executable_span;
    position.cap_basis_distance = admission_plan.cap_basis_distance;
    position.tick_size_at_admission = admission_plan.tick_size;
    if let PositionState::Active { invalidation_guard_level, .. } = &mut position.state {
        *invalidation_guard_level = Some(guard);
    }

    // While the guard binds, the normative span is entry -> guard basis
    // (477.60) and the cap is 0.25 x 477.60 = 119.40; the configured 100 bps
    // offset (623.867) is capped there. Trigger = guard + 119.40, rounded UP
    // to the grid for a Buy stop.
    let plan = engine
        .stop_plan(&position, Price::new(technical).unwrap(), Some(guard), Some(&rules))
        .unwrap();
    assert!(plan.guard_bound);
    assert_eq!(plan.cap_basis_distance, Some(dec!(477.60)));
    assert_eq!(plan.effective_buffer, dec!(119.40));
    let raw = guard.as_decimal() + dec!(119.40);
    assert!(plan.trigger.as_decimal() >= raw);
    assert!(plan.trigger.as_decimal() - raw < dec!(0.10));

    // First full-span advance releases the guard; the new plan derives from
    // the trailing stop with the ORIGINAL technical span (305.60) as cap
    // span: cap = 76.40 < 100 bps offset, still binding.
    let span = dec!(305.60);
    let advance_price = entry - position.executable_span.unwrap();
    let market = MarketData::new(position.symbol.clone(), Price::new(advance_price).unwrap());
    let decision = engine
        .process_active_position_with_rules(&position, &market, Some(&rules))
        .unwrap();
    let updated = decision.updated_position.expect("advance must update the position");
    let (new_stop, released_guard) = match &updated.state {
        PositionState::Active {
            trailing_stop, invalidation_guard_level, ..
        } => (*trailing_stop, *invalidation_guard_level),
        other => panic!("expected Active, got {other:?}"),
    };
    assert_eq!(released_guard, None, "first advance must release the guard");

    let released_plan = engine.stop_plan(&updated, new_stop, None, Some(&rules)).unwrap();
    assert!(!released_plan.guard_bound);
    assert_eq!(released_plan.cap_basis_distance, Some(span));
    assert_eq!(released_plan.effective_buffer, span * dec!(0.25));
}

/// Review fix on PR #155: while the guard binds, the live plan measures its
/// cap span against the SIGNAL entry reference (the one `decide_entry`
/// priced the admission risk with), not the fill price. A fill worse than
/// the signal reference must not widen the capped buffer past what sizing
/// charged.
#[test]
fn executable_span_guard_bound_plan_uses_persisted_admission_basis() {
    let rules = btcusdt_rules();
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());

    // Short armed off signal entry 61_909.10; filled WORSE at 61_950.00.
    let signal_entry = Price::new(dec!(61909.10)).unwrap();
    let fill_price = Price::new(dec!(61950.00)).unwrap();
    let technical = Price::new(dec!(62214.70)).unwrap();
    let guard = Price::new(dec!(62386.70)).unwrap();

    let mut position = Position::new_with_stop_policy(
        Uuid::now_v7(),
        Symbol::from_pair("BTCUSDT").unwrap(),
        Side::Short,
        StopPolicy::ExecutableSpan,
        Some(dec!(100)),
    );
    position.stop_plan_entry_reference = Some(signal_entry);
    // tech_stop_distance also preserves the SIGNAL entry for pre-migration
    // replay compatibility; entry_price is the fill.
    position.tech_stop_distance =
        Some(TechnicalStopDistance::from_entry_and_stop(signal_entry, technical));
    position.entry_price = Some(fill_price);
    position.quantity = Quantity::new(dec!(0.01)).unwrap();
    position.state = PositionState::Active {
        current_price: fill_price,
        trailing_stop: technical,
        favorable_extreme: fill_price,
        extreme_at: Utc::now(),
        insurance_stop_id: None,
        invalidation_guard_level: Some(guard),
        last_emitted_stop: None,
    };

    // The live plan trigger is identical to the admission-time plan built
    // from the signal reference: priced == executed.
    let admission_plan = build_executable_stop_plan(StopPlanInputs {
        policy: StopPolicy::ExecutableSpan,
        side: Side::Short,
        technical_stop: technical,
        guard: Some(guard),
        entry_reference: Some(signal_entry),
        technical_span: Some(dec!(305.60)),
        stop_buffer_bps: dec!(100),
        executable_span_source: ExecutableSpanSource::Admission,
        rules: Some(&rules),
    })
    .unwrap();
    position.initial_executable_stop = Some(admission_plan.trigger);
    position.executable_span = admission_plan.executable_span;
    position.cap_basis_distance = admission_plan.cap_basis_distance;
    position.tick_size_at_admission = admission_plan.tick_size;

    let live_plan = engine.stop_plan(&position, technical, Some(guard), Some(&rules)).unwrap();
    assert!(live_plan.guard_bound);
    assert_eq!(position.stop_plan_entry_reference, Some(signal_entry));
    // Cap basis = |signal entry - guard basis| = 477.60, not the 436.70
    // fill-to-basis distance.
    assert_eq!(live_plan.cap_basis_distance, Some(dec!(477.60)));
    assert_eq!(live_plan.trigger, admission_plan.trigger);
    assert_eq!(live_plan.effective_buffer, admission_plan.effective_buffer);
}

/// An ExecutableSpan position without trading rules fails closed at the engine
/// surface (no silent fallback to the unquantized derivation).
#[test]
fn executable_span_without_rules_fails_closed_at_the_engine() {
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let position = executable_active_position(Side::Long, dec!(62000.00), dec!(61000.00), dec!(10));
    let market = MarketData::new(position.symbol.clone(), Price::new(dec!(61500.00)).unwrap());

    let result = engine.process_active_position_with_rules(&position, &market, None);
    assert!(result.is_err(), "missing rules must be an error, got {result:?}");
}

#[test]
fn replay_after_tick_size_change_uses_persisted_span_for_first_advance() {
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let position = executable_active_position(Side::Long, dec!(62000), dec!(61000), dec!(10));
    assert_eq!(position.executable_span, Some(dec!(1061)));

    // Refreshed metadata would quantize the admission trigger to 60_930 and
    // re-derive S as 1_070. Replay must retain the persisted 1_061 ruler.
    let refreshed_rules = SymbolTradingRules::new(
        position.symbol.clone(),
        dec!(10),
        Decimal::ZERO,
        dec!(0.001),
        dec!(0.001),
        dec!(1000),
        Decimal::ZERO,
        2,
        3,
    )
    .unwrap();
    let market = MarketData::new(position.symbol.clone(), Price::new(dec!(63061)).unwrap());
    let decision = engine
        .process_active_position_with_rules(&position, &market, Some(&refreshed_rules))
        .unwrap();
    let candidate = decision.actions.iter().find_map(|action| match action {
        EngineAction::UpdateTrailingStop { new_stop, .. } => Some(*new_stop),
        _ => None,
    });

    assert_eq!(candidate, Some(Price::new(dec!(62000)).unwrap()));
}

#[test]
fn executable_span_admission_refuses_to_rederive_existing_evidence() {
    let rules = btcusdt_rules();
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let mut position = Position::new_with_stop_policy(
        Uuid::now_v7(),
        Symbol::from_pair("BTCUSDT").unwrap(),
        Side::Long,
        StopPolicy::ExecutableSpan,
        Some(dec!(10)),
    );
    position.initial_executable_stop = Some(Price::new(dec!(60939)).unwrap());
    position.executable_span = Some(dec!(1061));
    position.cap_basis_distance = Some(dec!(1000));
    position.tick_size_at_admission = Some(dec!(0.1));
    let signal = DetectorSignal {
        signal_id: Uuid::now_v7(),
        position_id: position.id,
        symbol: position.symbol.clone(),
        side: Side::Long,
        entry_price: Price::new(dec!(62000)).unwrap(),
        stop_loss: Price::new(dec!(61000)).unwrap(),
        technical_stop_analysis: None,
        timestamp: Utc::now(),
    };

    let error = engine
        .decide_entry_with_rules(&position, &signal, None, Some(&rules))
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::DomainError(
            robson_domain::DomainError::AdmissionEvidenceAlreadyPresent(message)
        )
            if message.contains("already contains immutable admission evidence")
    ));
}

#[test]
fn legacy_replay_derivation_is_pinned_to_historical_constants() {
    let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let entry = Price::new(dec!(62000)).unwrap();
    let initial_stop = Price::new(dec!(61000)).unwrap();
    let mut position = Position::new_with_stop_policy(
        Uuid::now_v7(),
        symbol.clone(),
        Side::Long,
        StopPolicy::LegacyUncapped,
        Some(dec!(10)),
    );
    position.entry_price = Some(entry);
    position.tech_stop_distance =
        Some(TechnicalStopDistance::from_entry_and_stop(entry, initial_stop));
    position.quantity = Quantity::new(dec!(0.01)).unwrap();
    position.state = PositionState::Active {
        current_price: entry,
        trailing_stop: initial_stop,
        favorable_extreme: entry,
        extreme_at: Utc::now(),
        insurance_stop_id: Some("legacy-insurance".to_string()),
        invalidation_guard_level: None,
        last_emitted_stop: None,
    };

    let initial_plan = engine.stop_plan(&position, initial_stop, None, None).unwrap();
    assert_eq!(initial_plan.trigger.as_decimal(), dec!(60939));
    assert_eq!(initial_plan.executable_span, None);

    let market = MarketData::new(symbol, Price::new(dec!(64000)).unwrap());
    let decision = engine.process_active_position_with_rules(&position, &market, None).unwrap();
    let (candidate, insurance) = decision
        .actions
        .iter()
        .find_map(|action| match action {
            EngineAction::ReplaceInsuranceStop { new_stop_price, .. } => {
                decision.actions.iter().find_map(|other| match other {
                    EngineAction::UpdateTrailingStop { new_stop, .. } => {
                        Some((*new_stop, *new_stop_price))
                    },
                    _ => None,
                })
            },
            _ => None,
        })
        .expect("legacy two-span move must update and replace protection");

    assert_eq!(candidate.as_decimal(), dec!(63000));
    assert_eq!(insurance.as_decimal(), dec!(62937));
    let replayed = decision.updated_position.expect("legacy update state");
    assert_eq!(replayed.stop_policy, StopPolicy::LegacyUncapped);
    assert_eq!(replayed.executable_span, None);
    assert_eq!(replayed.cap_basis_distance, None);
    assert_eq!(replayed.tick_size_at_admission, None);
}
