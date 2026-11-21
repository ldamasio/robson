//! Descriptive global source-page observation. Never a complete audit, fencing
//! proof, catalog result, clock qualification or production readiness input.
use sqlx::{ConnectOptions, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::{bootstrap::issue, completion, snapshot::get, BudgetError, Trace};

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
        let page = Page {
            examined: get(&row, "examined")?,
            spot: get(&row, "spot")?,
            missing_intent: get(&row, "missing_intent")?,
            missing_clock: get(&row, "missing_clock")?,
            missing_charge: get(&row, "missing_charge")?,
            mismatched: get(&row, "mismatched")?,
            cancels: get(&row, "cancels")?,
            legacy_null: get(&row, "legacy_null")?,
        };
        if page.examined.len() > 1000
            || !page.examined.windows(2).all(|ids| ids[0] < ids[1])
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
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(page)
    };
    // An error/cancel drops the actual raw session; no reuse or cleanup SQL.
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}

#[path = "source_audit/pg_tests.rs"]
mod pg_tests;
