//! Dormant native N5 claim persistence. A storage observation carries neither
//! startup authority nor an unused transport token.
use std::time::Duration;

use chrono::{DateTime, Utc};
use robson_exec::{clock_records::TICK_MAX, venue_clock::TickCapture};
use sqlx::{ConnectOptions, Executor, PgConnection, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{bootstrap::issue, completion, BudgetError, Trace};

const STORE_LIMIT: Duration = Duration::from_secs(2);
const BEGIN: &str = "BEGIN";
const COMMIT: &str = "COMMIT";
const SETTINGS: &str = include_str!("../sql/settings.sql");
const CLAIM: &str = include_str!("claim.sql");
const CONFIRM: &str = include_str!("confirm.sql");

mod cold_load;
#[cfg(all(test, feature = "crash-points"))]
pub(crate) mod rehearsal_c2;
mod commission;
mod response;
mod result;

pub(crate) use robson_exec::balance_costs::native_n5::{
    valid_audit_at, valid_identity, ClaimIdentity, ClaimResult, FrequencyKey, PriorClaim,
    PriorEligibility, DAY_NS, HANDOFF_NS,
};

#[derive(Clone, Debug)]
pub(crate) struct ClaimRequest {
    pub(crate) key: FrequencyKey,
    pub(crate) prior: PriorClaim,
    pub(crate) next: ClaimIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimObservation {
    pub(crate) revision: i64,
    pub(crate) identity: ClaimIdentity,
}

#[derive(Debug)]
pub(crate) enum ClaimOutcome {
    Stored(ClaimObservation),
    Refused,
    Uncertain(Confirmation),
}

/// A single exact check of this operation's possibly committed claim. Private
/// fields and move-only consumption prevent reissuing the observation check
/// or constructing one from a new process's read of the old journal.
#[derive(Debug)]
pub(crate) struct Confirmation {
    key: FrequencyKey,
    expected: ClaimObservation,
    deadline: Instant,
}

/// Check the shape and exact native arithmetic before sending SQL. PostgreSQL
/// still evaluates the actual persisted bounds under the conditional UPDATE.
fn eligibility_floor(request: &ClaimRequest) -> Result<u128, BudgetError> {
    let key = &request.key;
    let prior = &request.prior;
    let next = &request.next;
    let start = prior.identity.captured;
    if !key.is_valid()
        || !(1..i64::MAX).contains(&prior.revision)
        || !valid_identity(&prior.identity)
        || !valid_identity(next)
        || prior.identity.claim_id == next.claim_id
        || start.domain() != next.captured.domain()
    {
        return Err(BudgetError::Invalid);
    }
    let basis = if let Some(handoff) = prior.handoff {
        let age = handoff.elapsed_since(&start).map_err(|_| BudgetError::Invalid)?;
        if age.as_nanos() > HANDOFF_NS {
            return Err(BudgetError::Invalid);
        }
        handoff.nanoseconds()
    } else {
        start.nanoseconds().checked_add(HANDOFF_NS).ok_or(BudgetError::Invalid)?
    };
    basis
        .checked_add(DAY_NS)
        .filter(|end| *end <= TICK_MAX as u128)
        .ok_or(BudgetError::Invalid)
}

async fn raw_connection(pool: &PgPool) -> Result<PgConnection, BudgetError> {
    let options = (*pool.connect_options()).clone().options([
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ]);
    // Incomplete work drops this owner. No borrowing Transaction destructor
    // queues a ROLLBACK and no close_hard call flushes pending protocol bytes.
    options.connect().await.map_err(completion::classify)
}

async fn begin(
    connection: &mut PgConnection,
    deadline: Instant,
    trace: &Trace,
) -> Result<(), BudgetError> {
    issue(trace, deadline)?;
    completion::run(connection, sqlx::query(BEGIN), false).await?;
    issue(trace, deadline)?;
    let settings = completion::one(connection, sqlx::query(SETTINGS)).await?;
    if settings.len() != 3 {
        return Err(BudgetError::Invalid);
    }
    for (name, value) in [
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
        ("idle_timeout", "1s"),
    ] {
        if settings.try_get::<&str, _>(name).map_err(|_| BudgetError::Invalid)? != value {
            return Err(BudgetError::Invalid);
        }
    }
    Ok(())
}

async fn advance(
    connection: &mut PgConnection,
    request: &ClaimRequest,
    deadline: Instant,
    trace: &Trace,
) -> Result<Option<i64>, BudgetError> {
    let key = &request.key;
    let prior = &request.prior;
    let next = &request.next;
    let domain = prior.identity.captured.domain();
    let (offset_seconds, offset_ns) = domain.offset();
    let offset_ns = i32::try_from(offset_ns).map_err(|_| BudgetError::Invalid)?;
    let sql = sqlx::query(CLAIM)
        .bind(&key.symbol)
        .bind(key.initialization_id)
        .bind(key.recovery_generation)
        .bind(key.registration_id)
        .bind(prior.revision)
        .bind(prior.identity.claim_id)
        .bind(prior.identity.owner)
        .bind(prior.identity.digest.as_slice())
        .bind(domain.boot())
        .bind(offset_seconds)
        .bind(offset_ns)
        .bind(prior.identity.captured.nanoseconds().to_string())
        .bind(prior.identity.audit_at)
        .bind(prior.eligibility.as_str())
        .bind(prior.handoff.map(|tick| tick.nanoseconds().to_string()))
        .bind(prior.result.map(ClaimResult::as_str))
        .bind(next.claim_id)
        .bind(next.owner)
        .bind(next.digest.as_slice())
        .bind(next.captured.nanoseconds().to_string())
        .bind(next.audit_at);
    issue(trace, deadline)?;
    let row = completion::one(connection, sql).await?;
    row.try_get("revision").map_err(|_| BudgetError::Invalid)
}

pub(crate) async fn claim(
    pool: &PgPool,
    request: ClaimRequest,
    deadline: Instant,
    trace: &Trace,
) -> Result<ClaimOutcome, BudgetError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(BudgetError::Expired);
    }
    let deadline = deadline.min(now + STORE_LIMIT);
    let minimum = eligibility_floor(&request)?;
    let expected = ClaimObservation {
        revision: request.prior.revision + 1,
        identity: request.next.clone(),
    };
    let mut commit_started = false;
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = raw_connection(pool).await?;
        begin(&mut connection, deadline, trace).await?;
        let Some(revision) = advance(&mut connection, &request, deadline, trace).await? else {
            return Ok(false);
        };
        if revision != expected.revision || request.next.captured.nanoseconds() < minimum {
            return Err(BudgetError::Invalid);
        }
        issue(trace, deadline)?;
        commit_started = true;
        // A single simple COMMIT has no row or bound arguments. Executor
        // drains command completion before granting a stored observation.
        let result =
            connection.execute(sqlx::raw_sql(COMMIT)).await.map_err(completion::classify)?;
        if result.rows_affected() != 0 {
            return Err(BudgetError::Unpublished);
        }
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(true)
    };
    match timeout_at(deadline, work).await {
        Ok(Ok(true)) => Ok(ClaimOutcome::Stored(expected)),
        Ok(Ok(false)) => Ok(ClaimOutcome::Refused),
        _ if commit_started => {
            Ok(ClaimOutcome::Uncertain(Confirmation { key: request.key, expected, deadline }))
        },
        Ok(Err(error)) => Err(error),
        Err(_) => Err(BudgetError::Expired),
    }
}

pub(crate) async fn confirm_once(
    pool: &PgPool,
    original: Confirmation,
    trace: &Trace,
) -> Result<Option<ClaimObservation>, BudgetError> {
    let Confirmation { key, expected, deadline } = original;
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = raw_connection(pool).await?;
        let identity = &expected.identity;
        let domain = identity.captured.domain();
        let (offset_seconds, offset_ns) = domain.offset();
        let offset_ns = i32::try_from(offset_ns).map_err(|_| BudgetError::Invalid)?;
        let query = sqlx::query(CONFIRM)
            .bind(&key.symbol)
            .bind(key.initialization_id)
            .bind(key.recovery_generation)
            .bind(key.registration_id)
            .bind(expected.revision)
            .bind(identity.claim_id)
            .bind(identity.owner)
            .bind(identity.digest.as_slice())
            .bind(domain.boot())
            .bind(offset_seconds)
            .bind(offset_ns)
            .bind(identity.captured.nanoseconds().to_string())
            .bind(identity.audit_at);
        issue(trace, deadline)?;
        let row = completion::one(&mut connection, query).await?;
        let exact = row.try_get::<bool, _>("exact_claim").map_err(|_| BudgetError::Invalid)?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(exact.then_some(expected))
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

#[cfg(test)]
mod tests;
