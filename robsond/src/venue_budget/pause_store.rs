//! PS-2b: the pause keeper (PLAN §6.5). A 418/429 pauses every send in
//! memory before any await (`Heads::observe`); this keeper then writes the
//! `venue_pauses` row, never shortening one, and clears `pending` only when a
//! well-formed stored row covers the observation it wrote for, whoever wrote
//! it. `unresolved` restrictions are never written.
use std::{sync::LazyLock, time::Duration};

use chrono::{DateTime, Utc};
use prometheus::{register_int_counter_vec, IntCounterVec};
use robson_exec::venue_clock::NativeClock;
use sqlx::{ConnectOptions, Executor, PgConnection, PgPool};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    bootstrap::{begin_bounded, issue},
    completion,
    heads::PauseObservation,
    pause_value::PauseValue,
    snapshot::get,
    state::SharedBudget,
    BudgetError, Trace,
};

const WRITE_BOUND: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(5);
const OUTCOMES: [&str; 3] = ["landed", "uncovered", "failed"];
static PERSISTS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!("robsond_venue_pause_persist_total", "Pause writes", &["outcome"])
        .expect("register pause persist outcomes")
});

/// Registers the outcomes at start, at 0 (PLAN §6.11).
pub(crate) fn register() {
    for outcome in OUTCOMES {
        PERSISTS.with_label_values(&[outcome]);
    }
}

/// The keeper task (PLAN §6.5): one write in flight, woken by each 418/429,
/// retrying an unlanded one no sooner than 5 s later. Once the stop cancels
/// it, it makes at most one more write (the latest pending observation, so
/// a pause observed just before the stop can still land and the stop stay
/// clean), each bounded by its own 2 s, then ends. `write` is `persist` in
/// production, scripted in the property tests.
pub(crate) async fn keep<W, F>(
    budget: SharedBudget,
    cancel: CancellationToken,
    mut write: W,
) -> bool
where
    W: FnMut(PauseObservation) -> F,
    F: std::future::Future<Output = Result<bool, BudgetError>>,
{
    let Some(wake) = budget.pause_wake() else {
        return false;
    };
    loop {
        let closing = cancel.is_cancelled();
        let retry = match budget.pending_observation() {
            Some(observed) => {
                let landed = write(observed).await;
                let outcome = landed.map_or(2, |covers| usize::from(!covers));
                PERSISTS.with_label_values(&[OUTCOMES[outcome]]).inc();
                if landed == Ok(true) {
                    // Clears `pending` only if `observed` is still the latest.
                    budget.acknowledge_pause(observed.id);
                }
                if closing {
                    return true;
                }
                (landed != Ok(true)).then_some(RETRY)
            },
            None if closing => return true,
            None => None,
        };
        // A 418/429 during the write left a wake-up permit: nothing is lost.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if retry.is_some() {
                    return true;
                }
            },
            _ = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {},
            _ = wake.notified(), if retry.is_none() => {},
        }
    }
}

/// One write under a 2 s deadline on one raw connection: BEGIN, the checked
/// bounds, the never-shortening UPSERT, the stored row when the update was
/// skipped, COMMIT (4 statements, 5 when skipped). A lost COMMIT ACK is
/// confirmed by one read on a fresh connection. `Ok(true)`: a stored row
/// covers `observed`.
pub(crate) async fn persist(
    pool: &PgPool,
    tenant: Uuid,
    observed: &PauseObservation,
    clock: &NativeClock,
    trace: &Trace,
) -> Result<bool, BudgetError> {
    let deadline = Instant::now() + WRITE_BOUND;
    let at = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(BudgetError::Invalid)?;
    let domain = observed.observed.domain();
    let (seconds, nanos) = domain.offset();
    let duration = i128::from(observed.seconds) * 1_000_000_000;
    let options = (*pool.connect_options()).clone().options([
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ]);
    let work = async {
        let mut connection = options.connect().await.map_err(completion::classify)?;
        begin_bounded(&mut connection, deadline, trace).await?;
        issue(trace, deadline)?;
        let written = completion::one(
            &mut connection,
            sqlx::query(include_str!("sql/pause_write.sql"))
                .bind(tenant)
                .bind("binance:spot")
                .bind(observed.status)
                .bind(observed.id)
                .bind(domain.boot())
                .bind(seconds)
                .bind(i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?)
                .bind(observed.observed.nanoseconds().to_string())
                .bind(duration.to_string())
                .bind(observed.until)
                .bind(at),
        )
        .await?;
        // Applied: exactly the written row (recorded_at no earlier), which
        // covers its own observation; skipped: the stored row, judged alone.
        let stored =
            match (get::<i64>(&written, "applied")?, get::<Option<bool>>(&written, "exact")?) {
                (1, Some(true)) => None,
                (0, None) => Some(read_stored(&mut connection, tenant, deadline, trace).await?),
                _ => return Err(BudgetError::Invalid),
            };
        issue(trace, deadline)?;
        // A lost COMMIT ACK: the socket is dropped and one read on a fresh
        // connection, under the same deadline, decides.
        if connection.execute(sqlx::raw_sql("COMMIT")).await.is_err() {
            drop(connection);
            let mut fresh = options.connect().await.map_err(completion::classify)?;
            let confirmed = read_stored(&mut fresh, tenant, deadline, trace).await?;
            return Ok(covers(&confirmed, observed, clock));
        }
        Ok(stored.is_none_or(|stored| covers(&stored, observed, clock)))
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

/// The tenant's stored pause (`sql/pause.sql`), well formed and bounded.
async fn read_stored(
    connection: &mut PgConnection,
    tenant: Uuid,
    deadline: Instant,
    trace: &Trace,
) -> Result<PauseValue, BudgetError> {
    issue(trace, deadline)?;
    let row = completion::one(
        connection,
        sqlx::query(include_str!("sql/pause.sql")).bind(tenant).bind("binance:spot"),
    )
    .await?;
    PauseValue::decode(&row)?
        .filter(|pause| pause.bounded)
        .ok_or(BudgetError::Invalid)
}

/// PLAN §6.5's landing rule: the stored row covers the observation in its own
/// clock domain with a deadline no earlier, or in another domain with a full
/// duration no shorter than what remains of it (a restart re-anchors it).
fn covers(stored: &PauseValue, observed: &PauseObservation, clock: &NativeClock) -> bool {
    let end = i128::try_from(observed.end.nanoseconds()).unwrap_or(i128::MAX);
    let left = clock.capture().map(|now| observed.end.elapsed_since(&now).unwrap_or_default());
    match (stored.restored, stored.duration, left) {
        (Some((domain, _, deadline)), ..) if domain == observed.end.domain() => deadline >= end,
        (Some(_), Some(full), Ok(left)) => {
            full >= i128::try_from(left.as_nanos()).unwrap_or(i128::MAX)
        },
        _ => false,
    }
}

#[cfg(test)]
#[path = "pause_store_pg_tests.rs"]
mod tests;
