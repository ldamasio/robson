//! Private durable running/failed source audit. Never a completed audit,
//! fencing proof, BootstrapInput or readiness. Callers must already own the
//! external quiescence barrier; nothing here manufactures or checks it.
use std::time::Duration;

use sqlx::{postgres::PgRow, ConnectOptions, PgConnection, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{decode, Page};
use crate::venue_budget::{bootstrap::issue, completion, snapshot::get, BudgetError, Trace};

const VENUE: &str = "binance:spot";
const OVERALL: Duration = Duration::from_secs(60);
const TRANSACTION: Duration = Duration::from_secs(2);

macro_rules! audit_page {
    ($page:literal) => {
        concat!(
            include_str!("../sql/audit_page_binding.sql"),
            include_str!($page),
            "\n) AS source"
        )
    };
}
const FIRST_PAGE: &str = audit_page!("../sql/source_page_first.sql");
const NEXT_PAGE: &str = audit_page!("../sql/source_page_next.sql");

/// The exact persisted binding last confirmed by an acknowledged COMMIT.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone, Copy))]
pub(super) struct Run {
    pub(super) audit: Uuid,
    pub(super) generation: i64,
    pub(super) high_water: Option<Uuid>,
    pub(super) cursor: Option<Uuid>,
    pub(super) examined: i64,
    pub(super) envelope: Envelope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Discrepancy {
    MissingIntent,
    MissingClock,
    MissingCharge,
    Mismatched,
}

impl Discrepancy {
    fn reason(self) -> &'static str {
        match self {
            Self::MissingIntent => "source_missing_intent",
            Self::MissingClock => "source_missing_clock",
            Self::MissingCharge => "source_missing_charge",
            Self::Mismatched => "source_mismatched_association",
        }
    }

    fn of(page: &Page) -> Option<Self> {
        [
            (&page.missing_intent, Self::MissingIntent),
            (&page.missing_clock, Self::MissingClock),
            (&page.missing_charge, Self::MissingCharge),
            (&page.mismatched, Self::Mismatched),
        ]
        .into_iter()
        .find_map(|(ids, kind)| (!ids.is_empty()).then_some(kind))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pass {
    /// Acknowledged COMMIT of exactly one checkpoint at the current binding.
    Checkpointed(Run),
    /// Cursor already reached the recorded high-water, or no source existed.
    /// No SQL ran; this is never completion.
    Exhausted,
    /// Acknowledged COMMIT of the typed terminal failure under this binding.
    Failed(Discrepancy),
    /// Binding was not current; nothing was written by this pass.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Connect,
    Begin,
    Settings,
    Lock,
    Setup,
    Page,
    Checkpoint,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fault {
    pub(super) phase: Phase,
    pub(super) audit: Option<Uuid>,
    pub(super) error: BudgetError,
}

/// One caller-owned monotonic envelope for every pass of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Envelope(Instant);

impl Envelope {
    pub(super) fn new(deadline: Instant) -> Result<Self, BudgetError> {
        let now = Instant::now();
        if deadline <= now || deadline > now + OVERALL {
            return Err(BudgetError::Invalid);
        }
        Ok(Self(deadline))
    }

    fn transaction(self) -> Result<Instant, BudgetError> {
        let now = Instant::now();
        if now >= self.0 {
            return Err(BudgetError::Expired);
        }
        Ok(self.0.min(now + TRANSACTION))
    }
}

struct Step<'a> {
    phase: Phase,
    audit: Option<Uuid>,
    trace: &'a Trace,
    deadline: Instant,
}

impl Step<'_> {
    fn at(&mut self, phase: Phase) -> Result<(), Fault> {
        self.phase = phase;
        issue(self.trace, self.deadline).map_err(|error| self.fault(error))
    }

    fn fault(&self, error: BudgetError) -> Fault {
        Fault {
            phase: self.phase,
            audit: self.audit,
            error,
        }
    }
}

fn options(pool: &PgPool) -> sqlx::postgres::PgConnectOptions {
    (*pool.connect_options()).clone().options([
        ("statement_timeout", "1s"),
        ("idle_in_transaction_session_timeout", "1s"),
    ])
}

async fn begin(pool: &PgPool, step: &mut Step<'_>) -> Result<PgConnection, Fault> {
    if Instant::now() >= step.deadline {
        return Err(step.fault(BudgetError::Expired));
    }
    let mut connection =
        options(pool).connect().await.map_err(|e| step.fault(completion::classify(e)))?;
    step.at(Phase::Begin)?;
    completion::run(&mut connection, sqlx::query(include_str!("../sql/begin.sql")), false)
        .await
        .map_err(|e| step.fault(e))?;
    step.at(Phase::Settings)?;
    let settings =
        completion::one(&mut connection, sqlx::query(include_str!("../sql/settings.sql")))
            .await
            .map_err(|e| step.fault(e))?;
    if settings.len() != 3 {
        return Err(step.fault(BudgetError::Invalid));
    }
    for (name, value) in [
        ("lock_timeout", "300ms"),
        ("statement_timeout", "1s"),
        ("idle_timeout", "1s"),
    ] {
        if settings
            .try_get::<String, _>(name)
            .map_err(|_| step.fault(BudgetError::Invalid))?
            != value
        {
            return Err(step.fault(BudgetError::Invalid));
        }
    }
    Ok(connection)
}

/// Full COMMIT completion before the deadline, else the owned socket is
/// dropped with no cleanup SQL and no trusted outcome.
async fn commit(connection: &mut PgConnection, step: &mut Step<'_>) -> Result<(), Fault> {
    // A deadline that expires before issuing COMMIT stays a pre-COMMIT fault.
    issue(step.trace, step.deadline).map_err(|error| step.fault(error))?;
    step.phase = Phase::Commit;
    completion::run(connection, sqlx::query(include_str!("../sql/commit.sql")), false)
        .await
        .map_err(|e| step.fault(e))?;
    if Instant::now() >= step.deadline {
        return Err(step.fault(BudgetError::Expired));
    }
    Ok(())
}

fn expired(step: &Step<'_>) -> Fault {
    step.fault(BudgetError::Expired)
}

/// Five executions: BEGIN, settings, venue lock, binding/audit CTE, COMMIT.
pub(super) async fn start(
    pool: &PgPool,
    audit: Uuid,
    envelope: Envelope,
    trace: &Trace,
) -> Result<Run, Fault> {
    let mut step = Step {
        phase: Phase::Connect,
        audit: Some(audit),
        trace,
        deadline: envelope.transaction().map_err(|error| Fault {
            phase: Phase::Connect,
            audit: Some(audit),
            error,
        })?,
    };
    if audit.is_nil() {
        return Err(step.fault(BudgetError::Invalid));
    }
    let deadline = step.deadline;
    let work = async {
        let mut connection = begin(pool, &mut step).await?;
        step.at(Phase::Lock)?;
        let locked = completion::one(
            &mut connection,
            sqlx::query(include_str!("../sql/lock.sql")).bind(VENUE),
        )
        .await
        .map_err(|e| step.fault(e))?;
        let generation: i64 = get(&locked, "quiescence_generation").map_err(|e| step.fault(e))?;
        let prior: Option<Uuid> = get(&locked, "audit_id").map_err(|e| step.fault(e))?;
        if get::<String>(&locked, "venue_id").map_err(|e| step.fault(e))? != VENUE
            || generation < 0
            || prior == Some(audit)
        {
            return Err(step.fault(BudgetError::Invalid));
        }
        let next = generation.checked_add(1).ok_or_else(|| step.fault(BudgetError::Invalid))?;
        step.at(Phase::Setup)?;
        let row = completion::one(
            &mut connection,
            sqlx::query(include_str!("../sql/audit_setup.sql"))
                .bind(VENUE)
                .bind(audit)
                .bind(generation)
                .bind(prior),
        )
        .await
        .map_err(|e| step.fault(e))?;
        let run = setup_row(&row, audit, next, envelope).map_err(|e| step.fault(e))?;
        commit(&mut connection, &mut step).await?;
        Ok(run)
    };
    // Timeout/cancel drops the owned raw session; no queued cleanup SQL.
    let result = timeout_at(deadline, work).await;
    result.unwrap_or_else(|_| Err(expired(&step)))
}

fn setup_row(
    row: &PgRow,
    audit: Uuid,
    generation: i64,
    envelope: Envelope,
) -> Result<Run, BudgetError> {
    if row.len() != 7
        || get::<Uuid>(row, "audit_id")? != audit
        || get::<String>(row, "venue_id")? != VENUE
        || get::<i64>(row, "quiescence_generation")? != generation
        || get::<Option<Uuid>>(row, "cursor")?.is_some()
        || get::<i64>(row, "examined_count")? != 0
        || get::<String>(row, "status")? != "running"
    {
        return Err(BudgetError::Invalid);
    }
    Ok(Run {
        audit,
        generation,
        high_water: get(row, "high_water")?,
        cursor: None,
        examined: 0,
        envelope,
    })
}

/// At most five executions: BEGIN, settings, page+binding, one CAS, COMMIT.
/// The input must be the last acknowledged run; any fault abandons it.
pub(super) async fn pass(pool: &PgPool, run: Run, trace: &Trace) -> Result<Pass, Fault> {
    let fault = |phase, error| Fault { phase, audit: Some(run.audit), error };
    let Some(high_water) = run.high_water else {
        // Empty historical source: never completes in this slice.
        return if run.cursor.is_none() && run.examined == 0 {
            Ok(Pass::Exhausted)
        } else {
            Err(fault(Phase::Page, BudgetError::Invalid))
        };
    };
    if run.audit.is_nil()
        || run.generation < 0
        || !(0..=1_000_000).contains(&run.examined)
        || run.cursor.is_some_and(|c| c > high_water)
        || run.cursor.is_none() != (run.examined == 0)
    {
        return Err(fault(Phase::Page, BudgetError::Invalid));
    }
    if run.cursor == Some(high_water) {
        return Ok(Pass::Exhausted);
    }
    let deadline = run.envelope.transaction().map_err(|e| fault(Phase::Connect, e))?;
    let mut step = Step {
        phase: Phase::Connect,
        audit: Some(run.audit),
        trace,
        deadline,
    };
    let work = async {
        let mut connection = begin(pool, &mut step).await?;
        step.at(Phase::Page)?;
        let query = match run.cursor {
            None => sqlx::query(FIRST_PAGE).bind(high_water),
            Some(cursor) => sqlx::query(NEXT_PAGE).bind(high_water).bind(cursor),
        };
        let row = completion::one(&mut connection, query).await.map_err(|e| step.fault(e))?;
        if row.len() != 15 {
            return Err(step.fault(BudgetError::Invalid));
        }
        // Binding before any page decode: absent/ambiguous/foreign is stale.
        if !bound(&row, &run).map_err(|e| step.fault(e))? {
            return Ok(Pass::Stale);
        }
        let page = decode(&row, run.cursor, high_water, run.examined).map_err(|e| step.fault(e))?;
        step.at(Phase::Checkpoint)?;
        let (outcome, cas) = if let Some(kind) = Discrepancy::of(&page) {
            let row = completion::one(
                &mut connection,
                sqlx::query(include_str!("../sql/audit_fail.sql"))
                    .bind(run.audit)
                    .bind(run.generation)
                    .bind(run.high_water)
                    .bind(run.cursor)
                    .bind(run.examined)
                    .bind(kind.reason()),
            )
            .await
            .map_err(|e| step.fault(e))?;
            let moved =
                moved(&row, run.cursor, run.examined, "failed").map_err(|e| step.fault(e))?;
            (Pass::Failed(kind), moved)
        } else {
            // A short page that stops below the recorded high-water means
            // the historical envelope changed; never checkpoint or complete.
            let next = page.cursor().ok_or_else(|| step.fault(BudgetError::Invalid))?;
            if page.examined.len() < 1000 && next != high_water {
                return Err(step.fault(BudgetError::Invalid));
            }
            let examined = run
                .examined
                .checked_add(
                    i64::try_from(page.examined.len())
                        .map_err(|_| step.fault(BudgetError::Invalid))?,
                )
                .ok_or_else(|| step.fault(BudgetError::Invalid))?;
            let row = completion::one(
                &mut connection,
                sqlx::query(include_str!("../sql/audit_checkpoint.sql"))
                    .bind(run.audit)
                    .bind(run.generation)
                    .bind(run.high_water)
                    .bind(run.cursor)
                    .bind(run.examined)
                    .bind(next)
                    .bind(examined),
            )
            .await
            .map_err(|e| step.fault(e))?;
            let moved = moved(&row, Some(next), examined, "running").map_err(|e| step.fault(e))?;
            (Pass::Checkpointed(Run { cursor: Some(next), examined, ..run }), moved)
        };
        if !cas {
            // Lost CAS: no write by this owner; the session is just dropped.
            return Ok(Pass::Stale);
        }
        commit(&mut connection, &mut step).await?;
        Ok(outcome)
    };
    // A lost/late COMMIT reply is uncertain: no cursor, no retry, no cleanup.
    let result = timeout_at(deadline, work).await;
    result.unwrap_or_else(|_| Err(expired(&step)))
}

/// Exact current persisted binding equals the caller's acknowledged run.
fn bound(row: &PgRow, run: &Run) -> Result<bool, BudgetError> {
    let bindings: i64 = get(row, "bindings")?;
    if bindings != 1 {
        return if bindings == 0 {
            Ok(false)
        } else {
            Err(BudgetError::Invalid)
        };
    }
    Ok(get::<Option<Uuid>>(row, "bound_audit")? == Some(run.audit)
        && get::<Option<i64>>(row, "bound_generation")? == Some(run.generation)
        && get::<Option<Uuid>>(row, "bound_high_water")? == run.high_water
        && get::<Option<Uuid>>(row, "bound_cursor")? == run.cursor
        && get::<Option<i64>>(row, "bound_count")? == Some(run.examined)
        && get::<Option<String>>(row, "bound_status")?.as_deref() == Some("running"))
}

fn moved(
    row: &PgRow,
    cursor: Option<Uuid>,
    examined: i64,
    status: &str,
) -> Result<bool, BudgetError> {
    if row.len() != 4 {
        return Err(BudgetError::Invalid);
    }
    match get::<i64>(row, "moved")? {
        0 => Ok(false),
        1 if get::<Option<Uuid>>(row, "cursor")? == cursor
            && get::<Option<i64>>(row, "examined_count")? == Some(examined)
            && get::<Option<String>>(row, "status")?.as_deref() == Some(status) =>
        {
            Ok(true)
        },
        _ => Err(BudgetError::Invalid),
    }
}
