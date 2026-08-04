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
//! - `ExecutableSpan` (ADR-0052): the buffer is capped at 0.25 x the cap-basis
//!   distance and the trigger is tick-quantized adversely from
//!   [`SymbolTradingRules`]. Admission derives the immutable executable span
//!   from that trigger; every later resolution must consume the persisted span
//!   and admission cap basis. Missing or non-positive persisted values are hard
//!   errors: fail closed, never fall back to uncapped.

use rust_decimal::Decimal;

use crate::{
    stop_policy::StopPolicy,
    trading_rules::SymbolTradingRules,
    value_objects::{DomainError, Price, RiskConfig, Side, StopDistanceBounds},
};

/// Buffer cap ratio for `ExecutableSpan`: effective buffer <= 0.25 x the
/// cap-basis distance (ADR-0052 Decision 1).
pub const BUFFER_CAP_RATIO: Decimal = Decimal::from_parts(25, 0, 0, false, 2);

/// Source of the immutable executable-span persistence contract.
///
/// Admission is the only point allowed to derive `S`. Every live/replay
/// resolution carries the persisted values explicitly, including `None`, so
/// corrupt state cannot be mistaken for a fresh admission and re-derived from
/// current exchange metadata.
#[derive(Debug, Clone, Copy)]
pub enum ExecutableSpanSource {
    /// Resolve the admission plan and derive `S = |entry_reference - trigger|`.
    Admission,
    /// Resolve a risk-bearing position from its durable admission values.
    Persisted {
        /// Immutable buffer-inclusive span recorded at admission.
        executable_span: Option<Decimal>,
        /// Admission-time distance used to cap the initial buffer.
        cap_basis_distance: Option<Decimal>,
    },
}

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
    /// Signal entry reference price; required by `ExecutableSpan` admission.
    pub entry_reference: Option<Price>,
    /// Original technical span (`TechnicalStopDistance::span()`).
    pub technical_span: Option<Decimal>,
    /// ADR-0041 buffer in basis points (position snapshot at arm, or the
    /// live config for legacy positions without a snapshot).
    pub stop_buffer_bps: Decimal,
    /// Whether the executable span is being derived at admission or consumed
    /// from durable position state.
    pub executable_span_source: ExecutableSpanSource,
    /// Runtime symbol trading rules; required by `ExecutableSpan`.
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
    /// Distance the buffer cap was measured against (`None` under
    /// `LegacyUncapped`, which has no cap).
    pub cap_basis_distance: Option<Decimal>,
    /// Immutable buffer-inclusive unit of risk, including adverse tick
    /// quantization. Derived only at admission and consumed from persistence
    /// thereafter (`None` under `LegacyUncapped`).
    pub executable_span: Option<Decimal>,
    /// Tick size used for this resolution (`None` under `LegacyUncapped`).
    pub tick_size: Option<Decimal>,
    /// Effective buffer in price units after the span cap.
    pub effective_buffer: Decimal,
    /// THE executable trigger: the one price every surface uses. Tick
    /// quantized under `ExecutableSpan`; raw under `LegacyUncapped`.
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
/// - [`DomainError::DegenerateStopSpan`] when `ExecutableSpan` has no positive
///   cap-basis distance or executable span.
/// - [`DomainError::TradingRulesUnavailable`] when `ExecutableSpan` has no
///   symbol trading rules to quantize with.
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
        executable_span_source,
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
                cap_basis_distance: None,
                executable_span: None,
                tick_size: None,
                effective_buffer,
                trigger,
            })
        },
        StopPolicy::ExecutableSpan => {
            let persisted_cap_basis = match executable_span_source {
                ExecutableSpanSource::Admission => None,
                ExecutableSpanSource::Persisted { cap_basis_distance, .. } => {
                    let distance = cap_basis_distance.ok_or_else(|| {
                        DomainError::DegenerateStopSpan(
                            "ExecutableSpan requires persisted cap_basis_distance".to_string(),
                        )
                    })?;
                    if distance <= Decimal::ZERO {
                        return Err(DomainError::DegenerateStopSpan(format!(
                            "ExecutableSpan cap_basis_distance must be positive: {distance}"
                        )));
                    }
                    Some(distance)
                },
            };

            // While the guard binds, recovery consumes the admission-time cap
            // basis verbatim. Admission derives it from E -> A0. After guard
            // release the original TechnicalStopDistance span is authoritative.
            let cap_basis_distance = if guard_bound {
                match persisted_cap_basis {
                    Some(distance) => distance,
                    None => {
                        let entry = entry_reference.ok_or_else(|| {
                            DomainError::DegenerateStopSpan(
                                "ExecutableSpan admission with a binding guard requires an entry reference"
                                    .to_string(),
                            )
                        })?;
                        (entry.as_decimal() - basis.as_decimal()).abs()
                    },
                }
            } else {
                technical_span.ok_or_else(|| {
                    DomainError::DegenerateStopSpan(
                        "ExecutableSpan requires the original technical span".to_string(),
                    )
                })?
            };
            if cap_basis_distance <= Decimal::ZERO {
                return Err(DomainError::DegenerateStopSpan(format!(
                    "ExecutableSpan cap_basis_distance must be positive: {cap_basis_distance}"
                )));
            }

            let rules = rules.ok_or_else(|| {
                DomainError::TradingRulesUnavailable(
                    "ExecutableSpan requires symbol trading rules for tick quantization"
                        .to_string(),
                )
            })?;

            let effective_buffer = if stop_buffer_bps <= Decimal::ZERO {
                Decimal::ZERO
            } else {
                let offset = basis.as_decimal() * stop_buffer_bps / Decimal::from(10_000);
                offset.min(cap_basis_distance * BUFFER_CAP_RATIO)
            };
            let raw = match side {
                Side::Long => basis.as_decimal() - effective_buffer,
                Side::Short => basis.as_decimal() + effective_buffer,
            };
            let raw = Price::new(raw)?;
            // Quantize adversely: a Long is protected by a Sell stop (round
            // down), a Short by a Buy stop (round up).
            let trigger = rules.quantize_stop_trigger(side.exit_action(), raw)?;
            let executable_span = match executable_span_source {
                ExecutableSpanSource::Admission => {
                    let entry = entry_reference.ok_or_else(|| {
                        DomainError::DegenerateStopSpan(
                            "ExecutableSpan admission requires an entry reference".to_string(),
                        )
                    })?;
                    (entry.as_decimal() - trigger.as_decimal()).abs()
                },
                ExecutableSpanSource::Persisted { executable_span, .. } => executable_span
                    .ok_or_else(|| {
                        DomainError::DegenerateStopSpan(
                            "ExecutableSpan requires persisted executable_span".to_string(),
                        )
                    })?,
            };
            if executable_span <= Decimal::ZERO {
                return Err(DomainError::DegenerateStopSpan(format!(
                    "ExecutableSpan executable_span must be positive: {executable_span}"
                )));
            }
            Ok(ExecutableStopPlan {
                policy,
                side,
                basis,
                guard_bound,
                cap_basis_distance: Some(cap_basis_distance),
                executable_span: Some(executable_span),
                tick_size: Some(rules.tick_size()),
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
/// trigger       = plan.trigger (tick-quantized under ExecutableSpan)
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
fn loss_distance_and_fees_at_trigger(
    risk_config: &RiskConfig,
    side: Side,
    entry_price: Price,
    trigger: Price,
) -> Result<(Decimal, Decimal), DomainError> {
    let entry = entry_price.as_decimal();
    if entry <= Decimal::ZERO {
        return Err(DomainError::PositionSizingError("Entry price must be positive".to_string()));
    }
    let gap = trigger.as_decimal() * risk_config.stop_gap_bps() / Decimal::from(10_000);
    let adverse_fill = match side {
        Side::Long => trigger.as_decimal() - gap,
        Side::Short => trigger.as_decimal() + gap,
    };
    if adverse_fill <= Decimal::ZERO {
        return Err(DomainError::PositionSizingError(format!(
            "Adverse fill bound {adverse_fill} must be positive"
        )));
    }
    let distance = match side {
        Side::Long => entry - adverse_fill,
        Side::Short => adverse_fill - entry,
    };
    let fees = risk_config.taker_fee_rate() * (entry + adverse_fill);
    Ok((distance, fees))
}

/// Cost-priced residual risk per unit at an executable trigger.
///
/// Uses the same adverse gap and round-trip taker-fee envelope as admission
/// sizing, while clamping directional loss at zero once a trailing stop has
/// crossed into profit. This is the canonical latent-risk price for an
/// already committed Entering or Active position.
pub fn latent_risk_per_unit_at_trigger(
    risk_config: &RiskConfig,
    side: Side,
    entry_price: Price,
    trigger: Price,
) -> Result<Decimal, DomainError> {
    let (distance, fees) =
        loss_distance_and_fees_at_trigger(risk_config, side, entry_price, trigger)?;
    Ok(distance.max(Decimal::ZERO) + fees)
}

/// Worst expected realized loss per unit at an executable trigger.
///
/// Unlike latent pricing for an already-open winner, admission requires the
/// adverse fill to remain on the loss side of entry.
pub fn worst_case_loss_per_unit_at_trigger(
    risk_config: &RiskConfig,
    side: Side,
    entry_price: Price,
    trigger: Price,
) -> Result<Decimal, DomainError> {
    let (distance, fees) =
        loss_distance_and_fees_at_trigger(risk_config, side, entry_price, trigger)?;
    if distance <= Decimal::ZERO {
        return Err(DomainError::PositionSizingError(format!(
            "Adverse fill bound for trigger {} is not on the loss side of entry {}",
            trigger.as_decimal(),
            entry_price.as_decimal()
        )));
    }
    Ok(distance + fees)
}

/// Worst expected realized loss per unit for an entry priced from a resolved
/// executable-stop plan.
pub fn worst_case_loss_per_unit_planned(
    risk_config: &RiskConfig,
    entry_price: Price,
    plan: &ExecutableStopPlan,
) -> Result<Decimal, DomainError> {
    worst_case_loss_per_unit_at_trigger(risk_config, plan.side, entry_price, plan.trigger)
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

    fn executable_inputs(
        side: Side,
        technical_stop: Decimal,
        span: Decimal,
        bps: Decimal,
        rules: &SymbolTradingRules,
    ) -> StopPlanInputs<'_> {
        StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side,
            technical_stop: Price::new(technical_stop).unwrap(),
            guard: None,
            entry_reference: Some(
                Price::new(match side {
                    Side::Long => technical_stop + span,
                    Side::Short => technical_stop - span,
                })
                .unwrap(),
            ),
            technical_span: Some(span),
            stop_buffer_bps: bps,
            executable_span_source: ExecutableSpanSource::Admission,
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
                executable_span_source: ExecutableSpanSource::Admission,
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
            assert_eq!(plan.cap_basis_distance, None);
            assert_eq!(plan.executable_span, None);
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
            executable_span_source: ExecutableSpanSource::Admission,
            rules: None,
        })
        .unwrap();
        assert_eq!(plan.trigger, technical_stop);
        assert_eq!(plan.effective_buffer, Decimal::ZERO);
    }

    #[test]
    fn executable_span_caps_buffer_at_quarter_basis_and_quantizes_adversely() {
        let rules = rules();
        // Long, stop 62873.90 (tick-aligned), span 4 (tight), buffer 10 bps.
        // Uncapped offset = 62.8739; cap = 0.25 x 4 = 1.0 binds.
        // Raw = 62872.90 -> already aligned.
        let plan = build_executable_stop_plan(executable_inputs(
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
        let plan = build_executable_stop_plan(executable_inputs(
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
        let plan = build_executable_stop_plan(executable_inputs(
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
    fn executable_span_zero_buffer_quantizes_only() {
        let rules = rules();
        let plan = build_executable_stop_plan(executable_inputs(
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
        let plan = build_executable_stop_plan(executable_inputs(
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
    fn executable_span_buffer_cap_boundary_is_exactly_quarter_basis() {
        let rules = rules();
        // Configured offset = 1000 x 250 / 10_000 = 25, exactly equal to
        // 0.25 x cap_basis_distance (100). Equality must not widen or shrink.
        let plan = build_executable_stop_plan(executable_inputs(
            Side::Long,
            dec!(1000),
            dec!(100),
            dec!(250),
            &rules,
        ))
        .unwrap();
        assert_eq!(plan.cap_basis_distance, Some(dec!(100)));
        assert_eq!(plan.effective_buffer, dec!(25));
        assert_eq!(plan.trigger.as_decimal(), dec!(975));
        assert_eq!(plan.executable_span, Some(dec!(125)));
    }

    #[test]
    fn persisted_executable_span_is_consumed_not_rederived() {
        let rules = rules();
        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side: Side::Long,
            technical_stop: Price::new(dec!(1100)).unwrap(),
            guard: None,
            entry_reference: Some(Price::new(dec!(1100)).unwrap()),
            technical_span: Some(dec!(100)),
            stop_buffer_bps: dec!(10),
            executable_span_source: ExecutableSpanSource::Persisted {
                executable_span: Some(dec!(125)),
                cap_basis_distance: Some(dec!(100)),
            },
            rules: Some(&rules),
        })
        .unwrap();

        assert_eq!(plan.trigger.as_decimal(), dec!(1098.90));
        assert_eq!(plan.executable_span, Some(dec!(125)));
        assert_ne!(
            plan.executable_span,
            Some((dec!(1100) - plan.trigger.as_decimal()).abs()),
            "live resolution must never redefine S from the current candidate"
        );
    }

    #[test]
    fn persisted_executable_span_contract_fails_closed() {
        let rules = rules();
        for (span, basis) in [
            (None, Some(dec!(100))),
            (Some(Decimal::ZERO), Some(dec!(100))),
            (Some(dec!(-1)), Some(dec!(100))),
            (Some(dec!(125)), None),
            (Some(dec!(125)), Some(Decimal::ZERO)),
            (Some(dec!(125)), Some(dec!(-1))),
        ] {
            let result = build_executable_stop_plan(StopPlanInputs {
                policy: StopPolicy::ExecutableSpan,
                side: Side::Long,
                technical_stop: Price::new(dec!(1000)).unwrap(),
                guard: None,
                entry_reference: Some(Price::new(dec!(1100)).unwrap()),
                technical_span: Some(dec!(100)),
                stop_buffer_bps: dec!(10),
                executable_span_source: ExecutableSpanSource::Persisted {
                    executable_span: span,
                    cap_basis_distance: basis,
                },
                rules: Some(&rules),
            });
            assert!(matches!(result, Err(DomainError::DegenerateStopSpan(_))));
        }
    }

    #[test]
    fn executable_span_guard_bound_uses_entry_reference_basis() {
        let rules = rules();
        // Short: technical 62214.70, guard 62386.70 binds, entry 61909.10.
        // Normative span = 62386.70 - 61909.10 = 477.60.
        let plan = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side: Side::Short,
            technical_stop: Price::new(dec!(62214.70)).unwrap(),
            guard: Some(Price::new(dec!(62386.70)).unwrap()),
            entry_reference: Some(Price::new(dec!(61909.10)).unwrap()),
            technical_span: Some(dec!(305.60)),
            stop_buffer_bps: dec!(10),
            executable_span_source: ExecutableSpanSource::Admission,
            rules: Some(&rules),
        })
        .unwrap();
        assert!(plan.guard_bound);
        assert_eq!(plan.cap_basis_distance, Some(dec!(477.60)));
        // Offset = 62386.70 x 0.001 = 62.3867 < 0.25 x 477.60 = 119.40:
        // cap does not bind. Raw = 62449.0867 -> Buy stop rounds UP to the
        // grid: 62449.10.
        assert_eq!(plan.effective_buffer, dec!(62.38670));
        assert_eq!(plan.trigger.as_decimal(), dec!(62449.10));
        assert_eq!(plan.executable_span, Some(dec!(540.00)));
    }

    #[test]
    fn executable_span_degenerate_basis_fails_closed() {
        let rules = rules();
        for span in [Some(Decimal::ZERO), Some(dec!(-1)), None] {
            let result = build_executable_stop_plan(StopPlanInputs {
                policy: StopPolicy::ExecutableSpan,
                side: Side::Long,
                technical_stop: Price::new(dec!(100)).unwrap(),
                guard: None,
                entry_reference: Some(Price::new(dec!(101)).unwrap()),
                technical_span: span,
                stop_buffer_bps: dec!(10),
                executable_span_source: ExecutableSpanSource::Admission,
                rules: Some(&rules),
            });
            assert!(
                matches!(result, Err(DomainError::DegenerateStopSpan(_))),
                "span {span:?} must fail closed, got {result:?}"
            );
        }
    }

    #[test]
    fn executable_span_without_rules_fails_closed() {
        let result = build_executable_stop_plan(StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side: Side::Long,
            technical_stop: Price::new(dec!(100)).unwrap(),
            guard: None,
            entry_reference: Some(Price::new(dec!(101)).unwrap()),
            technical_span: Some(dec!(1)),
            stop_buffer_bps: dec!(10),
            executable_span_source: ExecutableSpanSource::Admission,
            rules: None,
        });
        assert!(matches!(result, Err(DomainError::TradingRulesUnavailable(_))));
    }

    #[test]
    fn adverse_fill_bound_per_side() {
        let rules = rules();
        let plan = build_executable_stop_plan(executable_inputs(
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

        let plan = build_executable_stop_plan(executable_inputs(
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
        let plan = build_executable_stop_plan(executable_inputs(
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
    fn latent_risk_keeps_residual_costs_after_stop_crosses_into_profit() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let trigger = Price::new(dec!(110)).unwrap();

        let latent = latent_risk_per_unit_at_trigger(&config, Side::Long, entry, trigger).unwrap();
        let adverse_fill = dec!(110) - dec!(110) * dec!(10) / dec!(10000);
        let expected_fees = dec!(0.0005) * (dec!(100) + adverse_fill);

        assert!(adverse_fill > dec!(100), "the trigger is already profitable");
        assert_eq!(latent, expected_fees);
    }

    #[test]
    fn admission_bounds_reject_the_final_trigger_not_the_raw_level() {
        let rules = rules();
        let bounds = StopDistanceBounds::default();
        // Long entry 65000, technical stop 58600 (9.8%, inside max 10%);
        // buffer 100 bps on a wide span pushes the trigger past 10%.
        let plan = build_executable_stop_plan(executable_inputs(
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
        let plan = build_executable_stop_plan(executable_inputs(
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
