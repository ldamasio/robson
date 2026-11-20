//! Public SQLx completion; protocol framing remains the pinned driver's
//! boundary.
use std::future::poll_fn;

use sqlx::{
    postgres::{PgArguments, PgRow},
    query::Query,
    Either, Executor, PgConnection, Postgres,
};

use super::StoreError;

// Only transport uncertainty can reach the caller's one confirmation path.
pub(super) fn classify(error: sqlx::Error) -> StoreError {
    match error {
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => StoreError::Backend,
        _ => StoreError::Invalid,
    }
}

#[derive(Default)]
struct Counts {
    rows: bool,
    command: bool,
}
impl Counts {
    fn row(&mut self) -> Result<(), StoreError> {
        if self.rows {
            return Err(StoreError::Unpublished);
        }
        self.rows = true;
        Ok(())
    }

    fn command(&mut self, affected: u64) -> Result<(), StoreError> {
        if self.command || affected != 1 {
            return Err(StoreError::Unpublished);
        }
        self.command = true;
        Ok(())
    }

    fn finish(self) -> Result<(), StoreError> {
        if self.rows && self.command {
            Ok(())
        } else {
            Err(StoreError::Unpublished)
        }
    }
}

pub(super) async fn one(
    connection: &mut PgConnection,
    query: Query<'_, Postgres, PgArguments>,
) -> Result<PgRow, StoreError> {
    let mut stream = connection.fetch_many(query);
    let mut counts = Counts::default();
    let mut row = None;
    while let Some(next) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        match next.map_err(classify)? {
            Either::Left(result) => counts.command(result.rows_affected())?,
            Either::Right(value) => {
                counts.row()?;
                row = Some(value);
            },
        }
    }
    // Successful exhaustion follows SQLx's Ready processing. No independent
    // full-tag/Idle attestation is available through this public API.
    counts.finish()?;
    row.ok_or(StoreError::Unpublished)
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
