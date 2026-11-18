//! Public SQLx drain/count rules for this concrete budget operation.
use std::future::poll_fn;

use sqlx::{
    postgres::{PgArguments, PgRow},
    query::Query,
    Either, Executor, PgConnection, Postgres,
};

use super::BudgetError;

pub(super) fn classify(error: sqlx::Error) -> BudgetError {
    match error {
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => BudgetError::Backend,
        _ => BudgetError::Invalid,
    }
}

pub(super) async fn run(
    connection: &mut PgConnection,
    query: Query<'_, Postgres, PgArguments>,
    returns_row: bool,
) -> Result<Option<PgRow>, BudgetError> {
    let mut stream = connection.fetch_many(query);
    let mut row = None;
    let mut command = false;
    while let Some(next) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        match next.map_err(classify)? {
            Either::Left(result) => {
                if command || result.rows_affected() != u64::from(returns_row) {
                    return Err(BudgetError::Unpublished);
                }
                command = true;
            },
            Either::Right(value) => {
                if !returns_row || row.is_some() {
                    return Err(BudgetError::Unpublished);
                }
                row = Some(value);
            },
        }
    }
    if command && row.is_some() == returns_row {
        Ok(row)
    } else {
        Err(BudgetError::Unpublished)
    }
}

pub(super) async fn one(
    connection: &mut PgConnection,
    query: Query<'_, Postgres, PgArguments>,
) -> Result<PgRow, BudgetError> {
    run(connection, query, true).await?.ok_or(BudgetError::Unpublished)
}
