//! Typed classification of a stop cancel (ADR-0056 step 7, PR C).
//!
//! One venue-neutral reading of a cancel's result, used by every production
//! cancel through [`OrderJournal::journaled_cancel`]. Only the two typed
//! definite answers settle a cancel. An error's type or text never does: an
//! adapter's code or message detection can only choose between two uncertain
//! classes, and a later close of the leg never resolves a cancel.
//!
//! Between this PR and the fence, an uncertain cancel opens no incident and
//! closes no admission. Its class does change how the software exit handles the
//! stop.
//!
//! [`OrderJournal::journaled_cancel`]: crate::order_journal::OrderJournal::journaled_cancel

use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::{
    error::ExecError,
    order_journal::VenueOutcome,
    ports::{OrderResult, StopCancelOutcome},
    venue::OrderLookup,
};

/// What a cancel's result establishes.
#[derive(Debug)]
pub enum CancelClass {
    /// Definite: the venue acknowledged the cancel.
    Cancelled {
        filled_before_cancel: Decimal,
        fills: Option<OrderResult>,
    },
    /// Definite: the order is gone and the venue gave its final state.
    GoneTerminal {
        filled_qty: Decimal,
        fills: Option<OrderResult>,
    },
    /// Uncertain: "already gone", with no terminal state behind it.
    GoneUnresolved { final_state: OrderLookup },
    /// Uncertain: the adapter could not establish the outcome.
    Indeterminate,
    /// Uncertain: the cancel returned an error, whatever its type or text.
    Errored(ExecError),
}

impl CancelClass {
    /// The label stored in `order_intents.cancel_class`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cancelled { .. } => "cancelled",
            Self::GoneTerminal { .. } => "gone_terminal",
            Self::GoneUnresolved { .. } => "gone_unresolved",
            Self::Indeterminate => "indeterminate",
            Self::Errored(_) => "error",
        }
    }

    /// The intent's venue outcome; `None` for every uncertain class.
    pub fn venue_outcome(&self) -> Option<VenueOutcome> {
        match self {
            Self::Cancelled { .. } => Some(VenueOutcome::Success),
            Self::GoneTerminal { .. } => Some(VenueOutcome::GoneTerminal),
            Self::GoneUnresolved { .. } | Self::Indeterminate | Self::Errored(_) => None,
        }
    }

    pub fn is_definite(&self) -> bool {
        self.venue_outcome().is_some()
    }

    /// The fills a definite class carries, for the intent's `venue_evidence`.
    pub(crate) fn evidence(&self) -> Option<Value> {
        match self {
            Self::Cancelled { filled_before_cancel, fills } => {
                Some(json!({ "filled_before_cancel": filled_before_cancel, "fills": fills }))
            },
            Self::GoneTerminal { filled_qty, fills } => {
                Some(json!({ "filled_qty": filled_qty, "fills": fills }))
            },
            Self::GoneUnresolved { .. } | Self::Indeterminate | Self::Errored(_) => None,
        }
    }
}

/// Classify a cancel's result. Exhaustive on purpose: a new variant anywhere
/// must be classified here before it compiles.
pub fn classify_cancel(result: Result<StopCancelOutcome, ExecError>) -> CancelClass {
    match result {
        Ok(StopCancelOutcome::Cancelled { filled_before_cancel, fills }) => {
            CancelClass::Cancelled { filled_before_cancel, fills }
        },
        Ok(StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { filled_qty, fills, .. },
        }) => CancelClass::GoneTerminal { filled_qty, fills },
        Ok(StopCancelOutcome::AlreadyGone {
            final_state:
                final_state @ (OrderLookup::Absent { .. }
                | OrderLookup::Open { .. }
                | OrderLookup::PartiallyFilled { .. }
                | OrderLookup::Unknown),
        }) => CancelClass::GoneUnresolved { final_state },
        Ok(StopCancelOutcome::Indeterminate) => CancelClass::Indeterminate,
        Err(error) => CancelClass::Errored(error),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use robson_domain::{Price, Quantity};
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;
    use crate::venue::VenueFinalityRule;

    fn fill() -> OrderResult {
        OrderResult {
            exchange_order_id: "77".to_string(),
            client_order_id: "ins-1".to_string(),
            fill_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        }
    }

    /// The cancel evidence bytes, captured before ADR-0056 step 8 (PR 8c)
    /// added `OrderResult`'s evidence fields. USD-M's `venue_evidence` must
    /// not change.
    #[test]
    fn usdm_cancel_evidence_is_byte_identical() {
        let cancelled = classify_cancel(Ok(StopCancelOutcome::Cancelled {
            filled_before_cancel: dec!(0),
            fills: None,
        }));
        assert_eq!(
            cancelled.evidence().unwrap().to_string(),
            r#"{"filled_before_cancel":"0","fills":null}"#
        );
        let mut result = fill();
        result.filled_at = chrono::DateTime::parse_from_rfc3339("2026-11-11T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let terminal = gone(OrderLookup::Terminal {
            filled_qty: dec!(0.1),
            cancelled_qty: dec!(0),
            fills: Some(result),
        });
        assert_eq!(
            terminal.evidence().unwrap().to_string(),
            r#"{"filled_qty":"0.1","fills":{"client_order_id":"ins-1","exchange_order_id":"77","fee":"0.01","fee_asset":"USDT","fill_price":"90","filled_at":"2026-11-11T00:00:00Z","filled_quantity":"0.1"}}"#
        );
    }

    fn gone(final_state: OrderLookup) -> CancelClass {
        classify_cancel(Ok(StopCancelOutcome::AlreadyGone { final_state }))
    }

    /// Every variant of [`ExecError`]. The match has no wildcard, so a new
    /// variant breaks this test's build until it joins the list.
    fn every_error(text: &str) -> Vec<ExecError> {
        let errors = vec![
            ExecError::Exchange(text.to_string()),
            ExecError::OrderRejected(text.to_string()),
            ExecError::RulesRejected(text.to_string()),
            ExecError::ReduceOnlyRejected(text.to_string()),
            ExecError::SendRefused(text.to_string()),
            ExecError::IntentJournal(text.to_string()),
            ExecError::AlreadyProcessed(Uuid::nil()),
            ExecError::Store(robson_store::StoreError::NotFound {
                entity_type: text.to_string(),
                id: text.to_string(),
            }),
            ExecError::Domain(robson_domain::DomainError::InvalidSymbol(text.to_string())),
            ExecError::Engine(robson_engine::EngineError::InvalidPositionState {
                expected: text.to_string(),
                actual: text.to_string(),
            }),
            ExecError::InvalidState(text.to_string()),
            ExecError::Config(text.to_string()),
            ExecError::Timeout(text.to_string()),
            ExecError::FuturesSafetyViolation {
                expected: text.to_string(),
                actual: text.to_string(),
                advice: text.to_string(),
            },
        ];
        for error in &errors {
            match error {
                ExecError::Exchange(_)
                | ExecError::OrderRejected(_)
                | ExecError::RulesRejected(_)
                | ExecError::ReduceOnlyRejected(_)
                | ExecError::SendRefused(_)
                | ExecError::IntentJournal(_)
                | ExecError::AlreadyProcessed(_)
                | ExecError::Store(_)
                | ExecError::Domain(_)
                | ExecError::Engine(_)
                | ExecError::InvalidState(_)
                | ExecError::Config(_)
                | ExecError::Timeout(_)
                | ExecError::FuturesSafetyViolation { .. } => {},
            }
        }
        errors
    }

    #[test]
    fn a_cancelled_outcome_is_definite_and_keeps_its_payload() {
        let class = classify_cancel(Ok(StopCancelOutcome::Cancelled {
            filled_before_cancel: dec!(0.02),
            fills: Some(fill()),
        }));
        assert_eq!(class.label(), "cancelled");
        assert_eq!(class.venue_outcome(), Some(VenueOutcome::Success));
        assert!(class.is_definite());
        let evidence = class.evidence().expect("a definite class keeps its fills");
        assert_eq!(evidence["filled_before_cancel"], json!(dec!(0.02)));
        assert_eq!(evidence["fills"]["exchange_order_id"], json!("77"));
    }

    #[test]
    fn gone_with_a_terminal_state_is_definite_and_keeps_its_fills() {
        for (fills, expected) in [(Some(fill()), json!("77")), (None, Value::Null)] {
            let class = gone(OrderLookup::Terminal {
                filled_qty: dec!(0.1),
                cancelled_qty: dec!(0),
                fills,
            });
            assert_eq!(class.label(), "gone_terminal");
            assert_eq!(class.venue_outcome(), Some(VenueOutcome::GoneTerminal));
            assert!(class.is_definite());
            let evidence = class.evidence().expect("the terminal fills are kept");
            assert_eq!(evidence["filled_qty"], json!(dec!(0.1)));
            assert_eq!(evidence["fills"]["exchange_order_id"].clone(), expected);
        }
    }

    /// Every [`OrderLookup`] other than `Terminal`, payload included: being
    /// gone is not a final state, whatever else the lookup reports.
    #[test]
    fn gone_with_any_other_state_is_uncertain() {
        let finality = VenueFinalityRule::EveryAttemptRejected;
        for final_state in [
            OrderLookup::Absent { finality },
            OrderLookup::Open { filled_qty: dec!(0) },
            OrderLookup::PartiallyFilled { filled_qty: dec!(0.05) },
            OrderLookup::Unknown,
        ] {
            let case = format!("{final_state:?}");
            let class = gone(final_state);
            assert_eq!(class.label(), "gone_unresolved", "{case}");
            assert_eq!(class.venue_outcome(), None, "{case}");
            assert!(!class.is_definite(), "{case}");
            assert!(class.evidence().is_none(), "{case}");
            let CancelClass::GoneUnresolved { final_state } = class else {
                panic!("{case}");
            };
            assert_eq!(format!("{final_state:?}"), case, "the state is carried, not read");
        }
    }

    #[test]
    fn an_indeterminate_outcome_is_uncertain() {
        let class = classify_cancel(Ok(StopCancelOutcome::Indeterminate));
        assert_eq!(class.label(), "indeterminate");
        assert_eq!(class.venue_outcome(), None);
        assert!(class.evidence().is_none());
    }

    #[test]
    fn every_error_is_uncertain() {
        for error in every_error("anything") {
            let case = error.to_string();
            let class = classify_cancel(Err(error));
            assert_eq!(class.label(), "error", "{case}");
            assert_eq!(class.venue_outcome(), None, "{case}");
            assert!(!class.is_definite(), "{case}");
        }
    }

    /// The messages the old `is_unknown_order_error` and the adapter's text
    /// detection key on, and the variants that carried them. See also the
    /// generated strings in `robsond/tests/cancel_class.rs`.
    #[test]
    fn no_message_can_make_a_definite_class() {
        for text in [
            "Unknown order sent.",
            "unknown order",
            "Order does not exist.",
            "order already gone",
            "already cancelled",
            "order filled",
            "cancelled",
            "gone_terminal",
            "success",
            "-2011",
            "-2013",
            "",
        ] {
            for error in every_error(text) {
                let case = format!("{error:?}");
                assert!(!classify_cancel(Err(error)).is_definite(), "{case}");
            }
        }
    }
}
