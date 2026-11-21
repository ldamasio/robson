//! Dormant exact result persistence for a same-domain claim.

use robson_exec::{
    balance_costs::{CostSource, EvidenceIdentity, COST_VALIDATION_VERSION},
    venue_clock::TickCapture,
};
use sqlx::{postgres::PgArguments, query::Query, Executor, PgPool, Postgres, Row};
use tokio::time::{timeout_at, Instant};

use super::{
    begin, completion, issue, raw_connection, valid_audit_at, valid_identity, BudgetError,
    ClaimObservation, ClaimResult, FrequencyKey, Trace, COMMIT, HANDOFF_NS, STORE_LIMIT,
};

const PUBLISH: &str =
    concat!(include_str!("result_binding.sql"), include_str!("publish_result.sql"));
const CONFIRM: &str = concat!(
    include_str!("result_binding.sql"),
    "SELECT EXISTS(SELECT 1 FROM exact_replay) AS exact_result",
);
pub(super) use robson_store::native_n5::MAX_PAYLOAD_BYTES;

#[derive(Debug, Clone)]
pub(super) struct PreparedSnapshot {
    pub(super) evidence: EvidenceIdentity,
    pub(super) observed: TickCapture,
    pub(super) payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(super) struct ResultRequest {
    pub(super) key: FrequencyKey,
    pub(super) claim: ClaimObservation,
    pub(super) handoff: Option<TickCapture>,
    pub(super) conclusion: ClaimResult,
    pub(super) snapshot: Option<PreparedSnapshot>,
}

#[derive(Debug)]
pub(super) enum Publication {
    Published,
    Replayed,
    Refused,
    Uncertain(ResultConfirmation),
}

#[derive(Debug)]
pub(super) struct ResultConfirmation {
    request: ResultRequest,
    deadline: Instant,
}

fn validate(request: &ResultRequest) -> Result<(), BudgetError> {
    let identity = &request.claim.identity;
    if !request.key.is_valid() || request.claim.revision <= 0 || !valid_identity(identity) {
        return Err(BudgetError::Invalid);
    }
    if let Some(handoff) = request.handoff {
        let elapsed =
            handoff.elapsed_since(&identity.captured).map_err(|_| BudgetError::Invalid)?;
        if elapsed.as_nanos() > HANDOFF_NS || matches!(request.conclusion, ClaimResult::NoSend) {
            return Err(BudgetError::Invalid);
        }
    }
    match (request.conclusion, &request.snapshot) {
        (ClaimResult::Success, Some(snapshot)) => {
            let account = snapshot.evidence.account();
            let handoff = request.handoff.ok_or(BudgetError::Invalid)?;
            snapshot.observed.elapsed_since(&handoff).map_err(|_| BudgetError::Invalid)?;
            if !valid_audit_at(snapshot.evidence.as_of())
                || snapshot.evidence.source() != CostSource::Venue
                || snapshot.evidence.validation_version() != COST_VALIDATION_VERSION
                || account.venue().as_str() != "binance:spot"
                || account.symbol().as_pair() != request.key.symbol
                || snapshot.payload.is_empty()
                || snapshot.payload.len() > MAX_PAYLOAD_BYTES
            {
                return Err(BudgetError::Invalid);
            }
        },
        (ClaimResult::Success, None) | (_, Some(_)) => return Err(BudgetError::Invalid),
        (_, None) => {},
    }
    Ok(())
}

fn bind<'a>(
    sql: &'static str,
    request: &'a ResultRequest,
) -> Result<Query<'a, Postgres, PgArguments>, BudgetError> {
    let id = &request.claim.identity;
    let domain = id.captured.domain();
    let (seconds, nanos) = domain.offset();
    let nanos = i32::try_from(nanos).map_err(|_| BudgetError::Invalid)?;
    let snapshot = request.snapshot.as_ref();
    let evidence = snapshot.map(|s| &s.evidence);
    let account = evidence.map(EvidenceIdentity::account);
    let version = i16::try_from(COST_VALIDATION_VERSION).map_err(|_| BudgetError::Invalid)?;
    Ok(sqlx::query(sql)
        .bind(&request.key.symbol)
        .bind(request.key.initialization_id)
        .bind(request.key.recovery_generation)
        .bind(request.key.registration_id)
        .bind(request.claim.revision)
        .bind(id.claim_id)
        .bind(id.owner)
        .bind(id.digest.as_slice())
        .bind(domain.boot())
        .bind(seconds)
        .bind(nanos)
        .bind(id.captured.nanoseconds().to_string())
        .bind(id.audit_at)
        .bind(request.handoff.map(|tick| tick.nanoseconds().to_string()))
        .bind(request.conclusion.as_str())
        .bind(account.map(|a| a.tenant()))
        .bind(account.map(|a| a.scope()))
        .bind(account.map(|a| a.uid().to_string()))
        .bind(snapshot.map(|s| s.observed.nanoseconds().to_string()))
        .bind(evidence.map(EvidenceIdentity::as_of))
        .bind(snapshot.map(|_| version))
        .bind(evidence.map(|e| e.hash().as_slice()))
        .bind(snapshot.map(|s| s.payload.as_slice())))
}

pub(super) async fn publish_result(
    pool: &PgPool,
    request: ResultRequest,
    deadline: Instant,
    trace: &Trace,
) -> Result<Publication, BudgetError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(BudgetError::Expired);
    }
    let deadline = deadline.min(now + STORE_LIMIT);
    validate(&request)?;
    let mut commit_started = false;
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = raw_connection(pool).await?;
        begin(&mut connection, deadline, trace).await?;
        let query = bind(PUBLISH, &request)?;
        issue(trace, deadline)?;
        let row = completion::one(&mut connection, query).await?;
        let verdict = match row
            .try_get::<Option<&str>, _>("disposition")
            .map_err(|_| BudgetError::Invalid)?
        {
            Some("published") => Publication::Published,
            Some("replayed") => Publication::Replayed,
            None => return Ok(Publication::Refused),
            _ => return Err(BudgetError::Invalid),
        };
        issue(trace, deadline)?;
        commit_started = true;
        let result =
            connection.execute(sqlx::raw_sql(COMMIT)).await.map_err(completion::classify)?;
        if result.rows_affected() != 0 {
            return Err(BudgetError::Unpublished);
        }
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(verdict)
    };
    match timeout_at(deadline, work).await {
        Ok(Ok(verdict)) => Ok(verdict),
        _ if commit_started => Ok(Publication::Uncertain(ResultConfirmation { request, deadline })),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(BudgetError::Expired),
    }
}

pub(super) async fn confirm_result_once(
    pool: &PgPool,
    original: ResultConfirmation,
    trace: &Trace,
) -> Result<bool, BudgetError> {
    let ResultConfirmation { request, deadline } = original;
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let query = bind(CONFIRM, &request)?;
        let mut connection = raw_connection(pool).await?;
        issue(trace, deadline)?;
        let row = completion::one(&mut connection, query).await?;
        let exact = row.try_get("exact_result").map_err(|_| BudgetError::Invalid)?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(exact)
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}
