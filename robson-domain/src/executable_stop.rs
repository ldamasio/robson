//! ExecutableStopPlan: the single stop resolver (issue #154 deliverable 2).
//!
//! One plan is built per stop decision and consumed by EVERY surface that
//! needs the executable stop: the engine soft-exit comparison, insurance
//! placement/replacement, the API, startup recovery, and sizing/costing.
//! ADR-0041's single-price invariant holds by construction because there is
//! exactly one derivation.
//!
//! Derivation is versioned by [`StopPolicy`]:
//!
//! - `LegacyUncapped` reproduces the historical path bit for bit: uncapped
//!   ADR-0041 buffer over the guard-aware basis, no domain-side tick
//!   quantization (the exchange adapter aligns at placement).
//! - `SpanCappedV1` (ADR-0050 §3/§4): the buffer is capped at 0.25 x the
//!   normative span, and the trigger is tick-quantized adversely from
//!   [`SymbolTradingRules`]. The normative span is `abs(entry_reference -
//!   guard_clamped_basis)` while the guard binds and the original technical
//!   span after the guard is released. A degenerate span (missing, zero, or
//!   negative) is a hard error: fail closed, never fall back to uncapped.

use rust_decimal::Decimal;

use crate::{
    stop_policy::StopPolicy,
    trading_rules::SymbolTradingRules,
    value_objects::{DomainError, Price, RiskConfig, Side, StopDistanceBounds},
};

/// Span cap ratio for `SpanCappedV1`: effective buffer <= 0.25 x span
/// (ADR-0050 §4).
pub const SPAN_CAP_RATIO: Decimal = Decimal::from_parts(25, 0, 0, false, 2);

/// Inputs to [`build_executable_stop_plan`].
#[derive(Debug, Clone, Copy)]
pub struct StopPlanInputs<'a> {
    /// Stop policy pinned to the position at arm time.
    pub policy: StopPolicy,
    /// Position side.
    pub side: Side,
    /// Chart-derived technical (trailing) stop, the system-wide reference.
    pub technical_stop: Price,
    /// Entry-time invalidation guard level while active (ADR-0042).
    pub guard: Option<Price>,
    /// Entry reference price; required by `SpanCappedV1` while the guard
    /// binds (the normative span is measured against it).
    pub entry_reference: Option<Price>,
    /// Original technical span (`TechnicalStopDistance::span()`).
    pub technical_span: Option<Decimal>,
    /// ADR-0041 buffer in basis points (position snapshot at arm, or the
    /// live config for legacy positions without a snapshot).
    pub stop_buffer_bps: Decimal,
    /// Runtime symbol trading rules; required by `SpanCappedV1`.
    pub rules: Option<&'a SymbolTradingRules>,
}

/// The resolved executable stop, built once per stop decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableStopPlan {
    /// Policy this plan was derived under.
    pub policy: StopPolicy,
    /// Position side.
    pub side: Side,
    /// Guard-aware basis (technical stop clamped to the guard when the guard
    /// lies on the adverse side).
    pub basis: Price,
    /// True when the guard clamped the basis away from the technical stop.
    pub guard_bound: bool,
    /// Normative span the buffer cap was measured against (`None` under
    /// `LegacyUncapped`, which has no cap).
    pub cap_span: Option<Decimal>,
    /// Effective buffer in price units after the span cap.
    pub effective_buffer: Decimal,
    /// THE executable trigger: the one price every surface uses. Tick
    /// quantized under `SpanCappedV1`; raw under `LegacyUncapped`.
    pub trigger: Price,
}

impl ExecutableStopPlan {
    /// Adverse fill bound for this trigger (issue #154 deliverable 4):
    /// `gap = trigger x gap_bps / 10_000`, Long fills at `trigger - gap`,
    /// Short at `trigger + gap`.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidPrice`] when the bound is non-positive.
    pub fn adverse_fill_bound(&self, gap_bps: Decimal) -> Result<Price, DomainError> {
        let gap = self.trigger.as_decimal() * gap_bps / Decimal::from(10_000);
        let bound = match self.side {
            Side::Long => self.trigger.as_decimal() - gap,
            Side::Short => self.trigger.as_decimal() + gap,
        };
        Price::new(bound)
    }

    /// Validate the FINAL executable stage (post guard, cap, and tick)
    /// against the shared distance bounds (ADR-0050 §5 stage 3). This is an
    /// admission-time check only: trailing stops that later cross the entry
    /// protecting profit are legitimately outside these bounds and must NOT
    /// be re-validated here.
    ///
    /// # Errors
    /// Returns [`DomainError::ExecutableStopOutOfBounds`] when the trigger
    /// distance from the entry reference leaves the allowed band.
    pub fn validate_admission_bounds(
        &self,
        entry_reference: Price,
        bounds: &StopDistanceBounds,
    ) -> Result<(), DomainError> {
        let entry = entry_reference.as_decimal();
        if entry <= Decimal::ZERO {
            return Err(DomainError::ExecutableStopOutOfBounds(
                "Entry reference must be positive".to_string(),
            ));
        }
        let distance_pct = (entry - self.trigger.as_decimal()).abs() / entry * Decimal::from(100);
        if distance_pct > bounds.max_pct() {
            return Err(DomainError::ExecutableStopOutOfBounds(format!(
                "Executable stop {} is {}% from entry {} (max {}%)",
                self.trigger,
                distance_pct.round_dp(4).normalize(),
                entry.normalize(),
                bounds.max_pct().normalize()
            )));
        }
        if distance_pct < bounds.min_pct() {
            return Err(DomainError::ExecutableStopOutOfBounds(format!(
                "Executable stop {} is {}% from entry {} (min {}%)",
                self.trigger,
                distance_pct.round_dp(4).normalize(),
                entry.normalize(),
                bounds.min_pct().normalize()
            )));
        }
        Ok(())
    }
}

/// Build the executable stop plan: the single derivation every consumer uses.
///
/// # Errors
/// - [`DomainError::DegenerateStopSpan`] when `SpanCappedV1` has no positive
///   normative span (fail closed: a v1 entry with a degenerate span is the
///   exact 2x-loss case ADR-0050 exists to kill).
/// - [`DomainError::TradingRulesUnavailable`] when `SpanCappedV1` has no symbol
///   trading rules to quantize with.
/// - [`DomainError::InvalidPrice`] when a derived price is non-positive.
pub fn build_executable_stop_plan(
    inputs: StopPlanInputs<'_>,
) -> Result<ExecutableStopPlan, DomainError> {
    let StopPlanInputs {
        policy,
        side,
        technical_stop,
        guard,
        entry_reference,
        technical_span,
        stop_buffer_bps,
        rules,
    } = inputs;

    // Guard-aware basis (ADR-0042): identical clamp for both policies.
    let basis = match guard {
        Some(g) => Price::new(match side {
            Side::Short => technical_stop.as_decimal().max(g.as_decimal()),
            Side::Long => technical_stop.as_decimal().min(g.as_decimal()),
        })
        .unwrap_or(technical_stop),
        None => technical_stop,
    };
    let guard_bound = basis != technical_stop;

    match policy {
        StopPolicy::LegacyUncapped => {
            // Bit-for-bit historical derivation
            // (`effective_stop_price_with_guard`): uncapped bps offset over
            // the clamped basis, no quantization.
            let trigger = crate::value_objects::effective_stop_price_with_guard(
                side,
                technical_stop,
                stop_buffer_bps,
                guard,
            );
            let effective_buffer = (trigger.as_decimal() - basis.as_decimal()).abs();
            Ok(ExecutableStopPlan {
                policy,
                side,
                basis,
                guard_bound,
                cap_span: None,
                effective_buffer,
                trigger,
            })
        },
        StopPolicy::SpanCappedV1 => {
            // Normative span (ADR-0050 §4, per the issue #154 spec): while
            // the guard binds, the distance actually protected is
            // entry_reference -> clamped basis; after release, the original
            // technical span.
            let span = if guard_bound {
                let entry = entry_reference.ok_or_else(|| {
                    DomainError::DegenerateStopSpan(
                        "SpanCappedV1 with a binding guard requires an entry reference".to_string(),
                    )
                })?;
                (entry.as_decimal() - basis.as_decimal()).abs()
            } else {
                technical_span.ok_or_else(|| {
                    DomainError::DegenerateStopSpan(
                        "SpanCappedV1 requires the technical span".to_string(),
                    )
                })?
            };
            if span <= Decimal::ZERO {
                return Err(DomainError::DegenerateStopSpan(format!(
                    "SpanCappedV1 span must be positive: {span}"
                )));
            }

            let rules = rules.ok_or_else(|| {
                DomainError::TradingRulesUnavailable(
                    "SpanCappedV1 requires symbol trading rules for tick quantization".to_string(),
                )
            })?;

            let effective_buffer = if stop_buffer_bps <= Decimal::ZERO {
                Decimal::ZERO
            } else {
                let offset = basis.as_decimal() * stop_buffer_bps / Decimal::from(10_000);
                offset.min(span * SPAN_CAP_RATIO)
            };
            let raw = match side {
                Side::Long => basis.as_decimal() - effective_buffer,
                Side::Short => basis.as_decimal() + effective_buffer,
            };
            let raw = Price::new(raw)?;
            // Quantize adversely: a Long is protected by a Sell stop (round
            // down), a Short by a Buy stop (round up).
            let trigger = rules.quantize_stop_trigger(side.exit_action(), raw)?;
            Ok(ExecutableStopPlan {
                policy,
                side,
                basis,
                guard_bound,
                cap_span: Some(span),
                effective_buffer,
                trigger,
            })
        },
    }
}

/// Worst expected realized loss per unit for an entry priced from the plan
/// (issue #154 deliverable 4, normative formula):
///
/// ```text
/// trigger       = plan.trigger (tick-quantized under v1)
/// gap           = trigger x gap_bps / 10_000
/// adverse_fill  = Long: trigger - gap | Short: trigger + gap
/// loss_per_unit = directional_distance(entry, adverse_fill)
///               + taker_fee x entry + taker_fee x adverse_fill
/// ```
///
/// The distance already runs through the buffer AND the gap (it ends at the
/// adverse fill), so nothing is double counted, and the exit fee is charged
/// on the adverse fill, not on the pre-buffer stop (the Short subpricing the
/// slice-4 review rejected).
///
/// # Errors
/// Returns [`DomainError::PositionSizingError`] when the adverse-fill
/// distance is non-positive (the trigger is on the wrong side of entry).
pub fn worst_case_loss_per_unit_planned(
    risk_config: &RiskConfig,
    entry_price: Price,
    plan: &ExecutableStopPlan,
) -> Result<Decimal, DomainError> {
    let entry = entry_price.as_decimal();
    if entry <= Decimal::ZERO {
        return Err(DomainError::PositionSizingError("Entry price must be positive".to_string()));
    }
    let adverse_fill = plan
        .adverse_fill_bound(risk_config.stop_gap_bps())
        .map_err(|e| DomainError::PositionSizingError(e.to_string()))?
        .as_decimal();
    let distance = match plan.side {
        Side::Long => entry - adverse_fill,
        Side::Short => adverse_fill - entry,
    };
    if distance <= Decimal::ZERO {
        return Err(DomainError::PositionSizingError(format!(
            "Adverse fill bound {adverse_fill} is not on the loss side of entry {entry}"
        )));
    }
    let fees = risk_config.taker_fee_rate() * (entry + adverse_fill);
    Ok(distance + fees)
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;
    use crate::value_objects::Symbol;

    fn rules() -> SymbolTradingRules {
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

    fn v1_inputs(
        side: Side,
        technical_stop: Decimal,
        span: Decimal,
        bps: Decimal,
        rules: &SymbolTradingRules,
    ) -> StopPlanInputs<'_> {
        StopPlanInputs {
            policy: StopPolicy::SpanCappedV1,
            side,
            technical_stop: Price::new(technical_stop).unwrap(),
            guard: None,
            entry_reference: None,
            technical_span: Some(span),
            stop_buffer_bps: bps,
            rules: Some(rules),
        }
    }

    #[test]
    fn legacy_plan_is_bit_for_bit_the_historical_derivation() {
        for (side, technical, guard) in [
            (Side::Long, dec!(58888.00), None),
            (Side::Short, dec!(62180.00), None),
            (Side::Short, dec!(62214.70), Some(dec!(62386.70))),
            (Side::Long, dec!(58888.00), Some(dec!(58700.00))),
        ] {
            let technical_stop = Price::new(technical).unwrap();
            let guard = guard.map(|g| Price::new(g).unwrap());
            let plan = build_executable_stop_plan(StopPlanInputs {
                policy: StopPolicy::LegacyUncapped,
                side,
                technical_stop,
                guard,
                entry_reference: None,
                technical_span: Some(dec!(300)),
                stop_buffer_bps: dec!(10),
                rules: None,
            })
            .unwrap();
            let legacy = crate::value_objects::effective_stop_price_with_guard(
                side,
                technical_stop,
                dec!(10),
                guard,
            );
            assert_eq!(plan.trigger, legacy, "legacy plan must match the historical helper");
            assert_eq!(plan.cap_span, None);
        }
    }

    #[test]
    fn legacy_zero_buffer_is_the_technical_stop_exactly() {
        let technical_stop = Price::new(dec!(62180.00)).unwrap();
        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::LegacyUncapped,
            side: Side::Long,
            technical_stop,
            guard: None,
            entry_reference: None,
            technical_span: None,
            stop_buffer_bps: Decimal::ZERO,
            rules: None,
        })
        .unwrap();
        assert_eq!(plan.trigger, technical_stop);
        assert_eq!(plan.effective_buffer, Decimal::ZERO);
    }

    #[test]
    fn v1_caps_buffer_at_quarter_span_and_quantizes_adversely() {
        let rules = rules();
        // Long, stop 62873.90 (tick-aligned), span 4 (tight), buffer 10 bps.
        // Uncapped offset = 62.8739; cap = 0.25 x 4 = 1.0 binds.
        // Raw = 62872.90 -> already aligned.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(62873.90),
            dec!(4),
            dec!(10),
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.effective_buffer, dec!(1.00));
        assert_eq!(plan.trigger.as_decimal(), dec!(62872.90));
        assert!(rules.is_tick_aligned(plan.trigger.as_decimal()));

        // Wide span: cap does not bind; raw = 62873.90 - 62.8739 = 62811.0261
        // -> Sell stop rounds DOWN to 62811.00 (grid-aligned from 556.80).
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(62873.90),
            dec!(1000),
            dec!(10),
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.trigger.as_decimal(), dec!(62811.00));

        // Short mirror: raw = 62873.90 + 62.8739 = 62936.7739 -> Buy stop
        // rounds UP to 62936.80.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Short,
            dec!(62873.90),
            dec!(1000),
            dec!(10),
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.trigger.as_decimal(), dec!(62936.80));
    }

    #[test]
    fn v1_zero_buffer_quantizes_only() {
        let rules = rules();
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(62873.90),
            dec!(100),
            Decimal::ZERO,
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.effective_buffer, Decimal::ZERO);
        assert_eq!(plan.trigger.as_decimal(), dec!(62873.90));

        // Unaligned technical stop with zero buffer still quantizes
        // adversely: protection must not depend on exchange leniency.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(62873.87),
            dec!(100),
            Decimal::ZERO,
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.trigger.as_decimal(), dec!(62873.80));
    }

    #[test]
    fn v1_guard_bound_uses_entry_reference_span() {
        let rules = rules();
        // Short: technical 62214.70, guard 62386.70 binds, entry 61909.10.
        // Normative span = 62386.70 - 61909.10 = 477.60.
        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::SpanCappedV1,
            side: Side::Short,
            technical_stop: Price::new(dec!(62214.70)).unwrap(),
            guard: Some(Price::new(dec!(62386.70)).unwrap()),
            entry_reference: Some(Price::new(dec!(61909.10)).unwrap()),
            technical_span: Some(dec!(305.60)),
            stop_buffer_bps: dec!(10),
            rules: Some(&rules),
        })
        .unwrap();
        assert!(plan.guard_bound);
        assert_eq!(plan.cap_span, Some(dec!(477.60)));
        // Offset = 62386.70 x 0.001 = 62.3867 < 0.25 x 477.60 = 119.40:
        // cap does not bind. Raw = 62449.0867 -> Buy stop rounds UP to the
        // grid: 62449.10.
        assert_eq!(plan.effective_buffer, dec!(62.38670));
        assert_eq!(plan.trigger.as_decimal(), dec!(62449.10));
    }

    #[test]
    fn v1_degenerate_span_fails_closed() {
        let rules = rules();
        for span in [Some(Decimal::ZERO), Some(dec!(-1)), None] {
            let result = build_executable_stop_plan(StopPlanInputs {
                policy: StopPolicy::SpanCappedV1,
                side: Side::Long,
                technical_stop: Price::new(dec!(100)).unwrap(),
                guard: None,
                entry_reference: None,
                technical_span: span,
                stop_buffer_bps: dec!(10),
                rules: Some(&rules),
            });
            assert!(
                matches!(result, Err(DomainError::DegenerateStopSpan(_))),
                "span {span:?} must fail closed, got {result:?}"
            );
        }
    }

    #[test]
    fn v1_without_rules_fails_closed() {
        let result = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::SpanCappedV1,
            side: Side::Long,
            technical_stop: Price::new(dec!(100)).unwrap(),
            guard: None,
            entry_reference: None,
            technical_span: Some(dec!(1)),
            stop_buffer_bps: dec!(10),
            rules: None,
        });
        assert!(matches!(result, Err(DomainError::TradingRulesUnavailable(_))));
    }

    #[test]
    fn adverse_fill_bound_per_side() {
        let rules = rules();
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(62873.90),
            dec!(1000),
            Decimal::ZERO,
            &rules,
        ))
        .unwrap();
        // gap 10 bps of 62873.90 = 62.8739; Long fills below the trigger.
        assert_eq!(
            plan.adverse_fill_bound(dec!(10)).unwrap().as_decimal(),
            dec!(62873.90) - dec!(62.87390)
        );

        let plan = build_executable_stop_plan(v1_inputs(
            Side::Short,
            dec!(62873.90),
            dec!(1000),
            Decimal::ZERO,
            &rules,
        ))
        .unwrap();
        assert_eq!(
            plan.adverse_fill_bound(dec!(10)).unwrap().as_decimal(),
            dec!(62873.90) + dec!(62.87390)
        );
    }

    #[test]
    fn worst_case_loss_charges_exit_fee_on_the_adverse_fill() {
        let rules = rules();
        let config = RiskConfig::new(dec!(10000)).unwrap();
        // Short: entry 62000, trigger 62873.90 (zero buffer), gap 10 bps.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Short,
            dec!(62873.90),
            dec!(1000),
            Decimal::ZERO,
            &rules,
        ))
        .unwrap();
        let entry = Price::new(dec!(62000)).unwrap();
        let loss = worst_case_loss_per_unit_planned(&config, entry, &plan).unwrap();

        let adverse = dec!(62873.90) + dec!(62873.90) * dec!(10) / dec!(10000);
        let expected = (adverse - dec!(62000)) + dec!(0.0005) * (dec!(62000) + adverse);
        assert_eq!(loss, expected);
        // The exit fee base is the adverse fill, strictly above the
        // pre-buffer stop: the Short deficit the review found is gone.
        assert!(adverse > dec!(62873.90));
    }

    #[test]
    fn admission_bounds_reject_the_final_trigger_not_the_raw_level() {
        let rules = rules();
        let bounds = StopDistanceBounds::default();
        // Long entry 65000, technical stop 58600 (9.8%, inside max 10%);
        // buffer 100 bps on a wide span pushes the trigger past 10%.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(58600.00),
            dec!(6400),
            dec!(100),
            &rules,
        ))
        .unwrap();
        let entry = Price::new(dec!(65000)).unwrap();
        let err = plan.validate_admission_bounds(entry, &bounds).unwrap_err();
        assert!(matches!(err, DomainError::ExecutableStopOutOfBounds(_)), "got {err:?}");

        // A modest buffer keeps the trigger inside the band.
        let plan = build_executable_stop_plan(v1_inputs(
            Side::Long,
            dec!(58600.00),
            dec!(6400),
            dec!(1),
            &rules,
        ))
        .unwrap();
        assert!(plan.validate_admission_bounds(entry, &bounds).is_ok());
    }
}
