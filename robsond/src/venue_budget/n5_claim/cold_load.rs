//! Read the bounded native N5 journal. Decoding and recovery authority remain separate.
use std::{collections::HashSet, future::poll_fn};

use sqlx::{postgres::PgRow, Either, Executor, PgPool};
use tokio::time::{timeout_at, Instant};

use super::{completion, issue, raw_connection, BudgetError, Trace, STORE_LIMIT};

const SQL: &str = include_str!("cold_load.sql");
const MAX_KEYS: usize = 4096;
const MAX_ROWS: usize = MAX_KEYS + 1;
const MAX_ACTIVE: usize = 64;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_COMPACT_WIRE_BYTES: usize = 1536;
const MAX_ACTIVE_METADATA_WIRE_BYTES: usize = 1024;
const MAX_MESSAGE_METADATA_WIRE_BYTES: usize = 8192;

// The caps include protocol framing for the selected fields. Only active keys
// can add snapshot bytes. The lifetime sentinel counts as a compact row.
const _: () = assert!(
    MAX_ROWS * MAX_COMPACT_WIRE_BYTES
        + MAX_ACTIVE * (MAX_ACTIVE_METADATA_WIRE_BYTES + super::result::MAX_PAYLOAD_BYTES)
        + MAX_MESSAGE_METADATA_WIRE_BYTES
        <= MAX_BYTES
);

pub(super) async fn read_rows(
    pool: &PgPool,
    symbols: &[String],
    deadline: Instant,
    trace: &Trace,
) -> Result<Vec<PgRow>, BudgetError> {
    if symbols.is_empty()
        || symbols.len() > MAX_ACTIVE
        || symbols.iter().any(|symbol| symbol.is_empty() || symbol.len() > 64)
        || symbols.iter().collect::<HashSet<_>>().len() != symbols.len()
    {
        return Err(BudgetError::Invalid);
    }
    let deadline = deadline.min(Instant::now() + STORE_LIMIT);
    let work = async {
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        let mut connection = raw_connection(pool).await?;
        issue(trace, deadline)?;
        let query = sqlx::query(SQL).bind(symbols);
        let mut stream = connection.fetch_many(query);
        let mut rows = Vec::with_capacity(MAX_ROWS);
        let mut command = None;
        while let Some(next) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            match next.map_err(completion::classify)? {
                Either::Right(row) if command.is_none() && rows.len() < MAX_ROWS => {
                    rows.push(row);
                },
                Either::Left(result) if command.is_none() => {
                    command = Some(result.rows_affected());
                },
                _ => return Err(BudgetError::Unpublished),
            }
        }
        if command != Some(rows.len() as u64) || rows.is_empty() || rows.len() > MAX_KEYS {
            return Err(BudgetError::Unpublished);
        }
        if Instant::now() >= deadline {
            return Err(BudgetError::Expired);
        }
        Ok(rows)
    };
    timeout_at(deadline, work).await.map_err(|_| BudgetError::Expired)?
}
