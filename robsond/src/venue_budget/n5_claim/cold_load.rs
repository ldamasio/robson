//! Adapter for the dormant native N5 read and exec-owned history validation.
#[cfg(all(test, feature = "crash-points"))]
pub(super) use robson_exec::balance_costs::native_n5::{
    fixture_manifest, ReceiptState, Registration,
};
use robson_exec::{
    balance_costs::{
        native_n5::{verify_history, HistoryError, VerifiedHistory},
        CostIdentity,
    },
    venue_clock::NativeClock,
};
use robson_store::native_n5::{self, Observation, MAX_ACTIVE};
use sqlx::PgPool;
use tokio::time::Instant;

use super::{BudgetError, Trace, STORE_LIMIT};

pub(super) async fn load(
    pool: &PgPool,
    expected: &[CostIdentity],
    clock: &NativeClock,
    deadline: Instant,
    trace: &Trace,
) -> Result<VerifiedHistory, BudgetError> {
    if expected.is_empty()
        || expected.len() > MAX_ACTIVE
        || expected
            .iter()
            .any(|entry| entry.venue().as_str() != robson_exec::venue::VenueId::BINANCE_SPOT)
    {
        return Err(BudgetError::Invalid);
    }
    let deadline = deadline.min(Instant::now() + STORE_LIMIT);
    let symbols: Vec<_> = expected.iter().map(|entry| entry.symbol().as_pair()).collect();
    let observation = read_rows(pool, &symbols, deadline, trace).await?;
    verify_history(observation, expected, clock, deadline).map_err(map_history_error)
}

fn map_history_error(error: HistoryError) -> BudgetError {
    match error {
        HistoryError::Invalid => BudgetError::Invalid,
        HistoryError::Backend => BudgetError::Backend,
        HistoryError::Unpublished => BudgetError::Unpublished,
        HistoryError::Expired => BudgetError::Expired,
        HistoryError::PrerequisiteRefusal => BudgetError::PrerequisiteRefusal,
    }
}

pub(super) async fn read_rows(
    pool: &PgPool,
    symbols: &[String],
    deadline: Instant,
    trace: &Trace,
) -> Result<Observation, BudgetError> {
    native_n5::read(pool, symbols, deadline, &trace.0)
        .await
        .map_err(|error| map_history_error(error.into()))
}

#[cfg(all(test, feature = "crash-points"))]
#[path = "cold_load_tests.rs"]
mod tests;
#[cfg(all(test, feature = "crash-points"))]
pub(super) use tests::verify_real_receipt_controls;
