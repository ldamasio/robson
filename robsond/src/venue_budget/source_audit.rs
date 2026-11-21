//! Descriptive global source-page observation. Never a complete audit, fencing
//! proof, catalog result, clock qualification or production readiness input.
use sqlx::{postgres::PgRow, ConnectOptions, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{bootstrap::issue, completion, snapshot::get, BudgetError, Trace};

const MAX_EXAMINED: i64 = 1_000_000;

#[derive(Debug, PartialEq, Eq)]
struct Page {
    examined: Vec<Uuid>,
    spot: Vec<Uuid>,
    missing_intent: Vec<Uuid>,
    missing_clock: Vec<Uuid>,
    missing_charge: Vec<Uuid>,
    mismatched: Vec<Uuid>,
    cancels: Vec<Uuid>,
    legacy_null: Vec<Uuid>,
}

impl Page {
    fn cursor(&self) -> Option<Uuid> {
        self.examined.last().copied()
    }
}

async fn observe(
    pool: &PgPool,
    cursor: Option<Uuid>,
    high_water: Uuid,
    deadline: Instant,
    trace: &Trace,
) -> Result<Page, BudgetError> {
    if cursor.is_some_and(|cursor| cursor > high_water) {
        return Err(BudgetError::Invalid);
    }
    // Private raw session, never J's pool or a borrowed Transaction. Startup
    // settings are not extra SQL; connection/auth work consumes this deadline.
    let options = (*pool.connect_options()).clone().options([
        ("default_transaction_read_only", "on"),
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ]);
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = options.connect().await.map_err(completion::classify)?;
        issue(trace, deadline)?;
        // A real nil source UUID is legal; first page has no lower-bound bind.
        let query = match cursor {
            None => sqlx::query(include_str!("sql/source_page_first.sql")).bind(high_water),
            Some(cursor) => sqlx::query(include_str!("sql/source_page_next.sql"))
                .bind(high_water)
                .bind(cursor),
        };
        let row = completion::one(&mut connection, query).await?;
        if row.len() != 8 {
            return Err(BudgetError::Invalid);
        }
        let page = decode(&row, cursor, high_water, 0)?;
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(page)
    };
    // An error/cancel drops the actual raw session; no reuse or cleanup SQL.
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

/// The 8 page fields of one fully completed row. `prior` is the count already
/// examined; the aggregate cap is checked before any subset is decoded.
fn decode(
    row: &PgRow,
    cursor: Option<Uuid>,
    high_water: Uuid,
    prior: i64,
) -> Result<Page, BudgetError> {
    let examined: Vec<Uuid> = get(row, "examined")?;
    let total = i64::try_from(examined.len())
        .ok()
        .and_then(|len| prior.checked_add(len))
        .ok_or(BudgetError::Invalid)?;
    if examined.len() > 1000 || !(0..=MAX_EXAMINED).contains(&total) {
        return Err(BudgetError::Invalid);
    }
    let page = Page {
        examined,
        spot: get(row, "spot")?,
        missing_intent: get(row, "missing_intent")?,
        missing_clock: get(row, "missing_clock")?,
        missing_charge: get(row, "missing_charge")?,
        mismatched: get(row, "mismatched")?,
        cancels: get(row, "cancels")?,
        legacy_null: get(row, "legacy_null")?,
    };
    if !page.examined.windows(2).all(|ids| ids[0] < ids[1])
        || page
            .examined
            .iter()
            .any(|id| *id > high_water || cursor.is_some_and(|c| *id <= c))
    {
        return Err(BudgetError::Invalid);
    }
    for ids in [
        &page.spot,
        &page.missing_intent,
        &page.missing_clock,
        &page.missing_charge,
        &page.mismatched,
        &page.cancels,
        &page.legacy_null,
    ] {
        if !ids.windows(2).all(|ids| ids[0] < ids[1])
            || ids.iter().any(|id| page.examined.binary_search(id).is_err())
        {
            return Err(BudgetError::Invalid);
        }
    }
    for ids in [
        &page.missing_intent,
        &page.missing_clock,
        &page.missing_charge,
        &page.mismatched,
        &page.cancels,
        &page.legacy_null,
    ] {
        if ids.iter().any(|id| page.spot.binary_search(id).is_err()) {
            return Err(BudgetError::Invalid);
        }
    }
    Ok(page)
}

// Private durable running/failed audit controller. No runtime entry exists.
#[path = "source_audit/run.rs"]
mod run;

/// A fresh descriptive terminal audit. This carries no bootstrap capability.
/// Test-only in package 4, like the supervised startup that runs it.
#[cfg(test)]
pub(crate) struct DescriptiveAudit {
    pub(crate) id: Uuid,
    pub(crate) examined: i64,
    pub(crate) commands: u64,
}

/// Run only against a target held closed by the supervisor. Retain exactly
/// one acknowledged continuation and never retry a refused or uncertain run.
#[cfg(test)]
pub(crate) async fn describe_existing_source(
    pool: &PgPool,
    deadline: Instant,
) -> Result<DescriptiveAudit, BudgetError> {
    use run::{Envelope, Pass, Terminal};

    const MAX_PASSES: u64 = 1_000;
    let envelope = Envelope::new(deadline)?;
    let trace = Trace::default();
    let id = Uuid::now_v7();
    let mut current = run::start_existing(pool, id, envelope, &trace)
        .await
        .map_err(|fault| fault.error)?;
    let mut passes = 0;
    while current.cursor != current.high_water {
        if passes == MAX_PASSES {
            return Err(BudgetError::Invalid);
        }
        current = match run::pass(pool, current, &trace).await.map_err(|fault| fault.error)? {
            Pass::Checkpointed(next) => next,
            Pass::Exhausted | Pass::Failed(_) | Pass::Stale => {
                return Err(BudgetError::Unpublished)
            },
        };
        passes += 1;
    }

    let examined = current.examined;
    let result = run::finish_audit(pool, current, &trace).await.map_err(|fault| fault.error)?;
    let Terminal::Complete(complete) = result else {
        return Err(BudgetError::Unpublished);
    };
    if Instant::now() >= deadline {
        return Err(BudgetError::Expired);
    }
    let commands = trace.issued();
    if complete.audit != id || complete.examined != examined || commands != 10 + 5 * passes {
        return Err(BudgetError::Invalid);
    }
    Ok(DescriptiveAudit { id, examined, commands })
}

#[cfg(test)]
#[path = "source_audit/pg_tests.rs"]
mod pg_tests;
#[cfg(test)]
#[path = "source_audit/run_pg_tests.rs"]
mod run_pg_tests;

#[cfg(test)]
#[path = "source_audit/physical_recovery.rs"]
pub(crate) mod physical_recovery;
