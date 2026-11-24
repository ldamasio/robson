//! The cutover certificate's advisory key (ADR-0056 step 8m-2a; 8m PLAN-r4
//! §6.7).
//!
//! One key per database, in PostgreSQL's two-int4 form, so it never meets
//! SQLx's migrator lock (one bigint key) or a test fixture's single key. Only
//! the certificate's completion (8m-5) takes it exclusive, at most once per
//! venue. Every production write of a certification input, by any robsond
//! process on any venue, takes it shared in its own transaction before any
//! row lock: right after the transaction's bounds statement where it has one,
//! else as its first statement. A participant therefore either holds the key
//! when completion asks for it, and completion waits for its `COMMIT` and
//! reads what it wrote, or asks while completion holds it and is ordered
//! after completion's `COMMIT`. Shared holders never block each other.
use sqlx::{
    postgres::{PgArguments, PgQueryResult},
    query::Query,
    PgConnection, PgPool, Postgres,
};
use tracing::Instrument as _;

/// The key's first half: "RB" and ADR-0056.
pub const CERTIFICATE_KEY_NAMESPACE: i32 = 0x5242_0056;
/// The key's second half: "8m".
pub const CERTIFICATE_KEY: i32 = 0x0000_386D;

/// A participant's statement: the key in shared mode, waiting at most the
/// transaction's `lock_timeout` (none on a position account's pool, whose wait
/// completion's own bounds limit).
pub const PARTICIPATE: &str = "SELECT pg_advisory_xact_lock_shared(1380057174, 14445)";
/// The append's statement: the key in shared mode now, or `false` with nothing
/// waited for.
pub const TRY_PARTICIPATE: &str = "SELECT pg_try_advisory_xact_lock_shared(1380057174, 14445)";
/// Completion's statement (8m-5); until then only tests take it, standing in
/// for completion.
pub const EXCLUSIVE: &str = "SELECT pg_advisory_xact_lock(1380057174, 14445)";

/// Proof that the transaction holds the certificate key in shared mode: only
/// [`participate`] and the append's try-lock make one, so a writer that took
/// the key passes it on, and what it calls next (the accounting bump) does not
/// take the key a second time.
#[derive(Debug)]
#[must_use = "the key is held for the transaction that took it"]
pub struct Participating {
    _held: (),
}

fn statement() -> tracing::Span {
    tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64)
}

/// Take the key shared on the caller's transaction: one statement.
///
/// # Errors
/// The wait passed the transaction's `lock_timeout` (`55P03`), or the
/// statement failed; the caller must not commit.
pub async fn participate(connection: &mut PgConnection) -> Result<Participating, sqlx::Error> {
    sqlx::query(PARTICIPATE).execute(connection).instrument(statement()).await?;
    Ok(Participating { _held: () })
}

/// Take the key shared without waiting: `None` while completion holds or waits
/// for it, with nothing else done in the transaction.
pub(crate) async fn try_participate(
    connection: &mut PgConnection,
) -> Result<Option<Participating>, sqlx::Error> {
    let held: bool = sqlx::query_scalar(TRY_PARTICIPATE)
        .fetch_one(connection)
        .instrument(statement())
        .await?;
    Ok(held.then_some(Participating { _held: () }))
}

/// A write that was one autocommit statement, made a participant: `BEGIN`,
/// the key, the statement, `COMMIT`. A failure before `COMMIT` drops the
/// transaction, which SQLx rolls back; an unanswered `COMMIT` may have landed
/// it, as the autocommit statement's unanswered reply could.
///
/// # Errors
/// Any of the four statements failed.
pub async fn execute_participating<'q>(
    pool: &PgPool,
    query: Query<'q, Postgres, PgArguments>,
) -> Result<PgQueryResult, sqlx::Error> {
    let io =
        |kind: u64| tracing::info_span!(target: "robson_operation", "operation_io", io_kind = kind);
    let mut tx = pool.begin().instrument(io(2)).await?;
    let _key = participate(&mut tx).await?;
    let written = query.execute(&mut *tx).instrument(statement()).await?;
    tx.commit().instrument(io(3)).await?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_pinned_and_every_statement_names_it() {
        assert_eq!((CERTIFICATE_KEY_NAMESPACE, CERTIFICATE_KEY), (1_380_057_174, 14_445));
        let key = format!("({CERTIFICATE_KEY_NAMESPACE}, {CERTIFICATE_KEY})");
        assert_eq!(PARTICIPATE, format!("SELECT pg_advisory_xact_lock_shared{key}"));
        assert_eq!(TRY_PARTICIPATE, format!("SELECT pg_try_advisory_xact_lock_shared{key}"));
        assert_eq!(EXCLUSIVE, format!("SELECT pg_advisory_xact_lock{key}"));
    }
}
