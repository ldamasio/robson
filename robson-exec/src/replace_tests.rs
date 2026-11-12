//! `replace_cancel_then_place` over a scripted balance-account venue
//! (ADR-0056 step 8, PR 8c-1; plan §9): the old stop's cancel and the
//! successor's answer are scripted, and the outcome is read from the records.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use robson_domain::{OrderSide, Price, Quantity, Symbol};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::*;
use crate::ports::FeeEvidence;

fn btcusdt() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

fn params() -> StopParams {
    StopParams {
        close_side: OrderSide::Sell,
        quantity: Quantity::new(dec!(0.0001)).unwrap(),
        trigger: Price::new(dec!(65955.60)).unwrap(),
        client_order_id: "ins-2".to_string(),
    }
}

/// The record a matching successor answers with.
fn successor() -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: "902".to_string(),
        client_order_id: "ins-2".to_string(),
        orig_client_order_id: None,
        symbol: "BTCUSDT".to_string(),
        side: OrderSide::Sell,
        order_type: "STOP_LOSS".to_string(),
        protection_kind: Some(ProtectionKind::MarketOnTrigger),
        status: "NEW".to_string(),
        phase: OrderPhase::Live,
        orig_qty: dec!(0.00010000),
        executed_qty: dec!(0),
        cumulative_quote_qty: Some(dec!(0)),
        stop_price: Some(dec!(65955.6)),
        expiry_reason: None,
        transact_time_ms: Some(1_762_819_200_000),
        update_time_ms: None,
    }
}

fn placed(record: Option<VenueOrderRecord>) -> PlacedStop {
    PlacedStop {
        stop: StopRef::new("902"),
        acceptance: OrderResult {
            exchange_order_id: "902".to_string(),
            client_order_id: "ins-2".to_string(),
            fill_price: params().trigger,
            filled_quantity: Quantity::zero(),
            fee: Decimal::ZERO,
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
            fills: Vec::new(),
            record,
            fee_evidence: FeeEvidence::Total,
        },
    }
}

/// How the scripted successor placement fails, when it does.
#[derive(Clone, Copy)]
enum PlacementFailure {
    Rejected,
    Unresolved,
    NotRecorded,
}

/// A balance-account venue whose cancel and placement answers are scripted.
struct Scripted {
    cancel: std::sync::Mutex<Option<Result<StopCancelOutcome, ExecError>>>,
    place: std::sync::Mutex<Option<Result<PlacedStop, PlacementFailure>>>,
    attempts: SubmitAttemptLedger,
    placements: AtomicUsize,
}

impl Scripted {
    fn new(
        cancel: Result<StopCancelOutcome, ExecError>,
        place: Result<PlacedStop, PlacementFailure>,
    ) -> Self {
        Self {
            cancel: std::sync::Mutex::new(Some(cancel)),
            place: std::sync::Mutex::new(Some(place)),
            attempts: SubmitAttemptLedger::new(8),
            placements: AtomicUsize::new(0),
        }
    }

    async fn replace(&self) -> ReplaceOutcome {
        replace_cancel_then_place(self, &self.attempts, &btcusdt(), &StopRef::new("901"), params())
            .await
            .unwrap()
    }
}

fn unused<T>() -> T {
    unreachable!("the replacement helper does not call this")
}

#[async_trait]
impl TradingPort for Scripted {
    async fn place_market_order(
        &self,
        _: &Symbol,
        _: OrderSide,
        _: Quantity,
        _: &str,
        _: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        unused()
    }

    async fn place_protection(
        &self,
        _: &Symbol,
        _: OrderSide,
        _: Quantity,
        _: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        self.placements.fetch_add(1, Ordering::SeqCst);
        match self.place.lock().unwrap().take().expect("one placement") {
            Ok(placed) => Ok(placed),
            Err(PlacementFailure::Rejected) => {
                self.attempts.begin(client_order_id).rejected();
                Err(ExecError::OrderRejected("scripted -2010".to_string()))
            },
            Err(PlacementFailure::Unresolved) => {
                let _ = self.attempts.begin(client_order_id);
                Err(ExecError::Timeout("scripted".to_string()))
            },
            Err(PlacementFailure::NotRecorded) => {
                Err(ExecError::SendRefused("scripted".to_string()))
            },
        }
    }

    async fn cancel_protection(
        &self,
        _: &Symbol,
        _: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        self.cancel.lock().unwrap().take().expect("one cancel")
    }

    async fn replace_protection(
        &self,
        _: &Symbol,
        _: &StopRef,
        _: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        unused()
    }

    async fn cancel_order(&self, _: &Symbol, _: &str) -> Result<(), ExecError> {
        unused()
    }

    async fn order_by_client_id(&self, _: &Symbol, _: &str) -> Result<OrderLookup, ExecError> {
        unused()
    }

    async fn order_by_exchange_id(
        &self,
        _: &Symbol,
        _: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        unused()
    }

    async fn protection_fill(
        &self,
        _: &Symbol,
        _: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        unused()
    }

    async fn open_protective_orders(&self, _: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError> {
        unused()
    }

    async fn user_trades_since(
        &self,
        _: &Symbol,
        _: DateTime<Utc>,
        _: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        unused()
    }

    async fn price(&self, _: &Symbol) -> Result<Price, ExecError> {
        unused()
    }

    async fn health(&self) -> Result<(), ExecError> {
        unused()
    }
}

#[async_trait]
impl AccountPort for Scripted {
    async fn cash(&self, _: &str) -> Result<Cash, ExecError> {
        unused()
    }

    async fn valuation(&self, _: &Symbol) -> Result<Mark, ExecError> {
        unused()
    }

    async fn execution_costs(&self, _: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        unused()
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            market_kind: MarketKind::Spot,
            account_kind: AccountKind::BalanceAccount,
            supports_short: false,
            reduce_only_wire_flag: false,
            protection: ProtectionKind::MarketOnTrigger,
            replacement: ReplacementProtocol::CancelThenPlace,
            has_funding: false,
            order_finality: VenueFinalityRule::EveryAttemptRejected,
            one_attempt_per_client_id: true,
        }
    }
}

fn cancelled(filled: Decimal) -> Result<StopCancelOutcome, ExecError> {
    Ok(StopCancelOutcome::Cancelled {
        filled_before_cancel: filled,
        fills: None,
    })
}

fn gone(final_state: OrderLookup) -> Result<StopCancelOutcome, ExecError> {
    Ok(StopCancelOutcome::AlreadyGone { final_state })
}

fn terminal(filled: Decimal) -> OrderLookup {
    OrderLookup::Terminal {
        filled_qty: filled,
        cancelled_qty: dec!(0.0001) - filled,
        fills: None,
    }
}

#[tokio::test]
async fn a_definite_cancel_and_a_matching_live_successor_replace() {
    for cancel in [cancelled(dec!(0)), gone(terminal(dec!(0)))] {
        let venue = Scripted::new(cancel, Ok(placed(Some(successor()))));
        let outcome = venue.replace().await;
        assert!(
            matches!(&outcome, ReplaceOutcome::Replaced { old_filled, .. } if old_filled.is_zero()),
            "{outcome:?}"
        );
    }
}

/// R1: nothing is placed until the old stop is definitely gone.
#[tokio::test]
async fn an_indefinite_cancel_places_nothing() {
    for cancel in [
        Ok(StopCancelOutcome::Unknown),
        gone(OrderLookup::Unknown),
        gone(OrderLookup::Open { filled_qty: dec!(0) }),
        gone(OrderLookup::PartiallyFilled { filled_qty: dec!(0.00004) }),
        gone(OrderLookup::Absent {
            finality: VenueFinalityRule::EveryAttemptRejected,
        }),
        Err(ExecError::Timeout("cancel".to_string())),
    ] {
        let venue = Scripted::new(cancel, Ok(placed(Some(successor()))));
        let outcome = venue.replace().await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
        assert_eq!(venue.placements.load(Ordering::SeqCst), 0);
    }
}

/// After a partial or full execution of the old stop the residual changed:
/// the helper derives no quantity and places nothing.
#[tokio::test]
async fn an_executed_old_stop_places_nothing() {
    for (cancel, filled) in [
        (cancelled(dec!(0.00004)), dec!(0.00004)),
        (gone(terminal(dec!(0.00004))), dec!(0.00004)),
        (gone(terminal(dec!(0.0001))), dec!(0.0001)),
    ] {
        let venue = Scripted::new(cancel, Ok(placed(Some(successor()))));
        let outcome = venue.replace().await;
        assert!(
            matches!(&outcome, ReplaceOutcome::OldExecuted { old_filled } if *old_filled == filled),
            "{outcome:?}"
        );
        assert_eq!(venue.placements.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn a_failed_placement_is_a_rejection_only_with_all_attempts_rejected() {
    let venue = Scripted::new(cancelled(dec!(0)), Err(PlacementFailure::Rejected));
    let outcome = venue.replace().await;
    assert!(
        matches!(&outcome, ReplaceOutcome::OldCancelledNewRejected { old_filled, .. }
            if old_filled.is_zero()),
        "{outcome:?}"
    );
    for failure in [PlacementFailure::Unresolved, PlacementFailure::NotRecorded] {
        let venue = Scripted::new(cancelled(dec!(0)), Err(failure));
        let outcome = venue.replace().await;
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }), "{outcome:?}");
    }
}

/// The successor's own record decides; `Ok` alone certifies nothing.
#[tokio::test]
async fn the_successor_is_read_from_its_record() {
    let with = |change: fn(&mut VenueOrderRecord)| {
        let mut record = successor();
        change(&mut record);
        Some(record)
    };
    let terminal_zero = with(|r| {
        r.status = "EXPIRED".to_string();
        r.phase = OrderPhase::Terminal;
    });
    let executing = with(|r| {
        r.status = "PARTIALLY_FILLED".to_string();
        r.executed_qty = dec!(0.00004);
    });
    let unknown = with(|r| {
        r.status = "PENDING_CANCEL".to_string();
        r.phase = OrderPhase::Unknown;
    });
    let cases = [
        (terminal_zero, "terminal"),
        (executing, "executing"),
        (unknown, "unknown"),
        (None, "unknown"),
    ];
    for (record, expected) in cases {
        let venue = Scripted::new(cancelled(dec!(0)), Ok(placed(record.clone())));
        let outcome = venue.replace().await;
        let got = match &outcome {
            ReplaceOutcome::SuccessorTerminal { .. } => "terminal",
            ReplaceOutcome::SuccessorExecuting { .. } => "executing",
            ReplaceOutcome::Unknown { new: Some(_) } => "unknown",
            _ => "other",
        };
        assert_eq!(got, expected, "{record:?}: {outcome:?}");
    }
}

/// A live, unexecuted successor that is not the stop asked for: a
/// `STOP_LOSS_LIMIT`, another symbol, other references, side, quantity or
/// trigger.
#[tokio::test]
async fn a_mismatched_successor_is_never_a_replacement() {
    let mismatches: [fn(&mut VenueOrderRecord); 8] = [
        |r| {
            r.order_type = "STOP_LOSS_LIMIT".to_string();
            r.protection_kind = None;
        },
        |r| r.symbol = "ETHUSDT".to_string(),
        |r| r.exchange_order_id = "903".to_string(),
        |r| r.client_order_id = "ins-9".to_string(),
        |r| r.side = OrderSide::Buy,
        |r| r.orig_qty = dec!(0.0002),
        |r| r.stop_price = Some(dec!(65955.61)),
        |r| r.stop_price = None,
    ];
    for (index, change) in mismatches.into_iter().enumerate() {
        let mut record = successor();
        change(&mut record);
        let venue = Scripted::new(cancelled(dec!(0)), Ok(placed(Some(record))));
        let outcome = venue.replace().await;
        assert!(
            matches!(outcome, ReplaceOutcome::SuccessorMismatched { .. }),
            "{index}: {outcome:?}"
        );
    }
}
