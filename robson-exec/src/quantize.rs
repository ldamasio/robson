//! Quantisation and metadata rules (ADR-0056 §5).
//!
//! Two different rules, on purpose:
//!
//! - **Admission** needs fresh validated metadata and fails closed without it:
//!   [`market_quantity`] and [`admission_quantity`] take the rules as a
//!   required argument, so there is nothing to fall back on.
//! - **Risk reduction** (exit, protection) never waits for a fetch when it
//!   holds rules: [`submit_reduction`] takes the newest it holds, submits, and
//!   on a venue rules rejection refreshes once and resubmits once. It cannot do
//!   more: the function has one refresh and two submissions in it, no loop.
//!   Whoever calls it decides how soon the leg tries again. With nothing held
//!   it does one lookup, and if that fails the quantity goes out as given.
//!
//! No venue is named here and no symbol-specific constant exists here
//! (AGENTS.md invariant 9): the venue's codes become
//! [`ExecError::RulesRejected`] in its adapter.

use std::{future::Future, time::Duration};

use robson_domain::{OrderSide, Price, Quantity, Symbol, SymbolTradingRules};
use rust_decimal::Decimal;
use tracing::warn;

use crate::{error::ExecError, ports::ExchangePort};

/// How long a reduction, or managing a leg, may wait for metadata end to end:
/// the venue's request timeout covers one request, not the queue for the
/// refresh gate ahead of it.
pub const METADATA_WAIT_BOUND: Duration = Duration::from_secs(15);

/// Wait for a metadata operation for at most `limit`. Running out of time is a
/// failure like any other, which the caller already knows how to handle.
pub async fn bounded_metadata<F>(
    limit: Duration,
    operation: F,
) -> Result<SymbolTradingRules, ExecError>
where
    F: Future<Output = Result<SymbolTradingRules, ExecError>>,
{
    match tokio::time::timeout(limit, operation).await {
        Ok(result) => result,
        Err(_) => Err(ExecError::Timeout(format!(
            "Trading rules not available within {} ms",
            limit.as_millis()
        ))),
    }
}

/// The lot-aligned market quantity for `quantity`, rounded down and validated
/// against the minimum, the maximum and the step of `rules`. Never rounds up.
///
/// # Errors
/// [`ExecError::OrderRejected`] when the rounded quantity is outside the rules.
pub fn market_quantity(
    rules: &SymbolTradingRules,
    quantity: Quantity,
) -> Result<Quantity, ExecError> {
    let rounded = rules.quantize_qty_down(quantity.as_decimal());
    rules.validate_order_qty(rounded).map_err(|error| {
        ExecError::OrderRejected(format!(
            "{error}. Increase capital or widen the stop distance; Robson will not round up."
        ))
    })?;
    Ok(Quantity::new(rounded)?)
}

/// [`market_quantity`] plus the minimum notional at `reference_price`: the
/// check an entry passes before anything is submitted.
///
/// # Errors
/// [`ExecError::OrderRejected`] when the quantity or the notional is refused.
pub fn admission_quantity(
    rules: &SymbolTradingRules,
    quantity: Quantity,
    reference_price: Decimal,
) -> Result<Quantity, ExecError> {
    let quantity = market_quantity(rules, quantity)?;
    rules
        .validate_notional(reference_price, quantity.as_decimal())
        .map_err(|error| ExecError::OrderRejected(error.to_string()))?;
    Ok(quantity)
}

/// What a risk-reducing order can do with a quantity under the rules it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReductionPlan {
    /// Submit `quantity`; `residual` is what the lot grid leaves behind.
    Submit {
        quantity: Quantity,
        residual: Decimal,
    },
    /// The whole quantity rounds below the lot minimum: nothing can be sent.
    NotExecutable { residual: Decimal },
}

/// Round `requested` down for a risk-reducing order.
///
/// Without rules the quantity goes out as it is: it came from a venue fill, and
/// the venue is the authority on it (this replaces the old hard-coded fallback
/// table, ADR-0056 §5). The maximum is deliberately not checked here. A local
/// refusal on possibly stale rules must not stop a reduction from reaching the
/// venue; a venue refusal is a rules rejection like any other.
pub fn reduction_plan(rules: Option<&SymbolTradingRules>, requested: Quantity) -> ReductionPlan {
    let Some(rules) = rules else {
        return ReductionPlan::Submit {
            quantity: requested,
            residual: Decimal::ZERO,
        };
    };
    let raw = requested.as_decimal();
    let rounded = rules.quantize_qty_down(raw);
    if rounded < rules.min_qty() {
        return ReductionPlan::NotExecutable { residual: raw };
    }
    match Quantity::new(rounded) {
        Ok(quantity) => ReductionPlan::Submit { quantity, residual: raw - rounded },
        Err(_) => ReductionPlan::NotExecutable { residual: raw },
    }
}

/// The protective trigger to send: the domain stop resolver's adverse tick
/// rounding when rules are known, the trigger as given otherwise. Rounding only
/// widens a trigger that is off the grid; it never recomputes the persisted
/// ADR-0052 span.
pub fn protective_trigger(
    rules: Option<&SymbolTradingRules>,
    close_side: OrderSide,
    trigger: Price,
) -> Price {
    let Some(rules) = rules else {
        return trigger;
    };
    if rules.is_tick_aligned(trigger.as_decimal()) {
        return trigger;
    }
    match rules.quantize_stop_trigger(close_side, trigger) {
        Ok(aligned) => {
            warn!(
                symbol = %rules.symbol().as_pair(),
                requested = %trigger.as_decimal(),
                aligned = %aligned.as_decimal(),
                tick_size = %rules.tick_size(),
                "Stop trigger not tick-aligned (legacy derivation); aligned adversely"
            );
            aligned
        },
        Err(error) => {
            warn!(
                symbol = %rules.symbol().as_pair(),
                requested = %trigger.as_decimal(),
                %error,
                "Stop trigger cannot be aligned; submitting it as given"
            );
            trigger
        },
    }
}

/// How a risk-reducing submission ended, when the venue did not refuse it.
#[derive(Debug)]
pub enum ReductionOutcome<T> {
    /// The venue accepted `quantity`; `residual` of the request was not sent.
    Submitted {
        accepted: T,
        quantity: Quantity,
        residual: Decimal,
        rules: Option<SymbolTradingRules>,
    },
    /// The venue refused the request and the newest rules that could be had
    /// agree that it rounds below the lot minimum. Nothing executed.
    NotExecutable {
        residual: Decimal,
        rules: Option<SymbolTradingRules>,
    },
}

/// Which submission of one reduction a `submit` call is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submission {
    /// The first, under the intent's own client id.
    First,
    /// The one resubmission after a rules rejection, under the same client id.
    SameIdRetry,
    /// The journal refused that resubmission before it left, so the refreshed
    /// order goes out once under a fresh client id. Carries the venue's rules
    /// rejection of the first submission: nothing else can reach this.
    FreshIdAfterRefusedRetry { rules_rejection: String },
}

/// Submit a risk-reducing order under ADR-0056 §5.
///
/// 1. Takes the last validated rules, then the leg's `snapshot`: no venue
///    request and no wait on a refresh in flight. The cache goes first because
///    it is never older than the snapshot taken from it at admission; after a
///    restart it is empty and the snapshot serves. With nothing held it looks
///    the rules up, for at most `metadata_limit`; a failed lookup sends the
///    quantity as given, so metadata never blocks a reduction.
/// 2. Submits with `submit(quantity, rules)`, where `rules` lets the caller
///    align a trigger on the same grid. When the rules held say nothing is
///    executable they may be stale, so the raw quantity goes out and the venue
///    decides.
/// 3. On [`ExecError::RulesRejected`], refreshes **once** and submits **once**
///    more. A second rejection is returned to the caller, which keeps whatever
///    protection the leg already has. [`ReductionOutcome::NotExecutable`] means
///    the venue refused and the newest rules that could be had agree.
/// 4. If the journal refuses that resubmission, [`ExecError::SendRefused`], it
///    never left and its client id is closed. The refreshed order goes out once
///    under a fresh id, at once: on a cancel-first exit the stop is already
///    gone. No loop: whatever that submission returns is the result.
///
/// The caller supplies one client id for every submission: a rules rejection
/// is one intent retried, and on `binance:usdm` the reduce-only flag bounds
/// what a mistaken reading of a rejection can do.
///
/// # Errors
/// A non-rules failure of a submission, the rules rejection when the refresh
/// fails and the rules held allow a submission, or the second rules rejection.
pub async fn submit_reduction<E, T, F, Fut>(
    exchange: &E,
    symbol: &Symbol,
    requested: Quantity,
    snapshot: Option<&SymbolTradingRules>,
    metadata_limit: Duration,
    mut submit: F,
) -> Result<ReductionOutcome<T>, ExecError>
where
    E: ExchangePort + ?Sized,
    F: FnMut(Quantity, Option<SymbolTradingRules>, Submission) -> Fut,
    Fut: Future<Output = Result<T, ExecError>>,
{
    let held = match exchange.last_validated_rules(symbol).or_else(|| snapshot.cloned()) {
        Some(held) => Some(held),
        None => match bounded_metadata(metadata_limit, exchange.trading_rules(symbol)).await {
            Ok(rules) => Some(rules),
            Err(error) => {
                warn!(
                    symbol = %symbol.as_pair(),
                    %error,
                    "No trading rules to be had for a reduction; submitting the quantity as given"
                );
                None
            },
        },
    };

    let (quantity, residual) = match reduction_plan(held.as_ref(), requested) {
        ReductionPlan::Submit { quantity, residual } => (quantity, residual),
        ReductionPlan::NotExecutable { .. } => (requested, Decimal::ZERO),
    };

    let reason = match submit(quantity, held.clone(), Submission::First).await {
        Ok(accepted) => {
            return Ok(ReductionOutcome::Submitted {
                accepted,
                quantity,
                residual,
                rules: held,
            });
        },
        Err(ExecError::RulesRejected(reason)) => reason,
        Err(error) => return Err(error),
    };

    warn!(
        symbol = %symbol.as_pair(),
        quantity = %quantity.as_decimal(),
        %reason,
        "Venue refused a reduction on its rules; refreshing once and resubmitting"
    );
    let rules = match bounded_metadata(metadata_limit, exchange.refresh_trading_rules(symbol)).await
    {
        Ok(fresh) => Some(fresh),
        Err(error) => {
            warn!(symbol = %symbol.as_pair(), %error, "Rules refresh failed after a rejection");
            // The venue refused and the rules held already said so.
            return match reduction_plan(held.as_ref(), requested) {
                ReductionPlan::NotExecutable { residual } => {
                    Ok(ReductionOutcome::NotExecutable { residual, rules: held })
                },
                ReductionPlan::Submit { .. } => Err(ExecError::RulesRejected(reason)),
            };
        },
    };
    let (quantity, residual) = match reduction_plan(rules.as_ref(), requested) {
        ReductionPlan::Submit { quantity, residual } => (quantity, residual),
        ReductionPlan::NotExecutable { residual } => {
            return Ok(ReductionOutcome::NotExecutable { residual, rules });
        },
    };
    let accepted = match submit(quantity, rules.clone(), Submission::SameIdRetry).await {
        Err(ExecError::SendRefused(refusal)) => {
            warn!(
                symbol = %symbol.as_pair(),
                %refusal,
                "Journal refused the resubmission; sending it once under a fresh client id"
            );
            let fresh = Submission::FreshIdAfterRefusedRetry { rules_rejection: reason };
            submit(quantity, rules.clone(), fresh).await?
        },
        other => other?,
    };
    Ok(ReductionOutcome::Submitted { accepted, quantity, residual, rules })
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    /// Longer than any bound a test here injects, so a test fails rather than
    /// hangs.
    const NOT_FOREVER: Duration = Duration::from_secs(5);

    fn rules_with_step(step: Decimal, min_qty: Decimal) -> SymbolTradingRules {
        SymbolTradingRules::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            dec!(0.10),
            dec!(556.80),
            step,
            min_qty,
            dec!(120),
            dec!(50),
            2,
            3,
        )
        .unwrap()
    }

    /// The `BTCUSDT` USDⓈ-M filters of the 2026-11-04 evidence file.
    fn btcusdt() -> SymbolTradingRules {
        rules_with_step(dec!(0.001), dec!(0.001))
    }

    fn qty(value: Decimal) -> Quantity {
        Quantity::new(value).unwrap()
    }

    #[test]
    fn market_quantity_rounds_down_and_validates() {
        assert_eq!(
            market_quantity(&btcusdt(), qty(dec!(0.0219))).unwrap().as_decimal(),
            dec!(0.021)
        );
        assert_eq!(market_quantity(&btcusdt(), qty(dec!(120))).unwrap().as_decimal(), dec!(120));

        let below = market_quantity(&btcusdt(), qty(dec!(0.0005))).unwrap_err().to_string();
        assert!(below.contains("below BTCUSDT minimum 0.001"), "{below}");
        assert!(below.contains("will not round up"), "{below}");

        let above = market_quantity(&btcusdt(), qty(dec!(120.001))).unwrap_err().to_string();
        assert!(above.contains("above BTCUSDT maximum 120"), "{above}");
    }

    #[test]
    fn market_quantity_uses_the_step_of_the_rules_not_a_table() {
        // The removed helper truncated every symbol to 0.001.
        let rules = rules_with_step(dec!(0.01), dec!(0.01));
        assert_eq!(market_quantity(&rules, qty(dec!(0.105))).unwrap().as_decimal(), dec!(0.10));
        assert!(market_quantity(&rules, qty(dec!(0.009))).is_err());
    }

    #[test]
    fn admission_quantity_adds_the_minimum_notional() {
        let rules = btcusdt();
        assert!(admission_quantity(&rules, qty(dec!(0.001)), dec!(77500)).is_ok());
        let refused = admission_quantity(&rules, qty(dec!(0.001)), dec!(30000)).unwrap_err();
        assert!(refused.to_string().contains("Notional 30"), "{refused}");
    }

    #[test]
    fn a_reduction_keeps_what_rounding_leaves_as_a_residual() {
        let rules = btcusdt();
        assert_eq!(reduction_plan(Some(&rules), qty(dec!(0.0219))), ReductionPlan::Submit {
            quantity: qty(dec!(0.021)),
            residual: dec!(0.0009),
        });
        assert_eq!(reduction_plan(Some(&rules), qty(dec!(0.021))), ReductionPlan::Submit {
            quantity: qty(dec!(0.021)),
            residual: Decimal::ZERO,
        });
        assert_eq!(reduction_plan(Some(&rules), qty(dec!(0.0005))), ReductionPlan::NotExecutable {
            residual: dec!(0.0005),
        });
    }

    #[test]
    fn a_reduction_without_rules_goes_out_unrounded_for_the_venue_to_judge() {
        assert_eq!(reduction_plan(None, qty(dec!(0.0015))), ReductionPlan::Submit {
            quantity: qty(dec!(0.0015)),
            residual: Decimal::ZERO,
        });
    }

    #[test]
    fn a_reduction_above_the_maximum_is_not_refused_locally() {
        // Stale rules must not stop a reduction from reaching the venue.
        assert_eq!(reduction_plan(Some(&btcusdt()), qty(dec!(150))), ReductionPlan::Submit {
            quantity: qty(dec!(150)),
            residual: Decimal::ZERO,
        });
    }

    #[test]
    fn protective_trigger_reuses_the_adverse_tick_rounding() {
        let rules = btcusdt();
        let off_grid = Price::new(dec!(62873.86105)).unwrap();
        assert_eq!(
            protective_trigger(Some(&rules), OrderSide::Sell, off_grid).as_decimal(),
            dec!(62873.80)
        );
        assert_eq!(
            protective_trigger(Some(&rules), OrderSide::Buy, off_grid).as_decimal(),
            dec!(62873.90)
        );
        let on_grid = Price::new(dec!(62873.9)).unwrap();
        assert_eq!(protective_trigger(Some(&rules), OrderSide::Sell, on_grid), on_grid);
        assert_eq!(protective_trigger(None, OrderSide::Sell, off_grid), off_grid);
    }

    #[tokio::test]
    async fn the_metadata_bound_is_a_failure_when_it_runs_out_and_nothing_when_it_does_not() {
        let hung = tokio::time::timeout(
            NOT_FOREVER,
            bounded_metadata(
                Duration::from_millis(10),
                std::future::pending::<Result<SymbolTradingRules, ExecError>>(),
            ),
        )
        .await
        .expect("the bound must hold");
        assert!(matches!(hung, Err(ExecError::Timeout(_))), "{hung:?}");

        let answered = bounded_metadata(Duration::from_secs(60), async { Ok(btcusdt()) }).await;
        assert_eq!(answered.unwrap(), btcusdt());
    }

    /// A reduction on a hung lookup runs out of time and goes out as given.
    #[tokio::test]
    async fn a_hung_lookup_bounds_a_reduction_and_the_quantity_goes_out_as_given() {
        let exchange = crate::stub::StubExchange::new(dec!(95000));
        exchange.set_rules_fetch_hangs(true);
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let outcome = tokio::time::timeout(
            NOT_FOREVER,
            submit_reduction(
                &exchange,
                &symbol,
                qty(dec!(0.1055)),
                None,
                Duration::from_millis(10),
                |submitted, rules, _| async move {
                    assert!(rules.is_none());
                    Ok::<_, ExecError>(submitted)
                },
            ),
        )
        .await
        .expect("the bound must hold")
        .unwrap();

        assert!(matches!(
            outcome,
            ReductionOutcome::Submitted { accepted, .. } if accepted.as_decimal() == dec!(0.1055)
        ));
    }

    /// A hung refresh after a rejection is a failed refresh: no resubmission.
    #[tokio::test]
    async fn a_hung_refresh_bounds_a_reduction_and_counts_as_a_failed_refresh() {
        let exchange = crate::stub::StubExchange::new(dec!(95000));
        exchange.set_rules_fetch_hangs(true);
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut submissions = 0;

        let outcome = tokio::time::timeout(
            NOT_FOREVER,
            submit_reduction(
                &exchange,
                &symbol,
                qty(dec!(0.1)),
                Some(&btcusdt()),
                Duration::from_millis(10),
                |_, _, _| {
                    submissions += 1;
                    async { Err::<Quantity, _>(ExecError::RulesRejected("refused".to_string())) }
                },
            ),
        )
        .await
        .expect("the bound must hold");

        assert!(matches!(outcome, Err(ExecError::RulesRejected(_))), "{outcome:?}");
        assert_eq!(submissions, 1);
    }
}
