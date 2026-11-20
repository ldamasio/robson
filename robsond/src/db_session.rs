//! Server-side session bounds for a balance-account daemon (ADR-0056 step 8).
//!
//! A balance-account daemon's root pool starts every session with
//! `lock_timeout`, `statement_timeout` and
//! `idle_in_transaction_session_timeout` as Startup options. The journal and
//! alarm pools clone its connect options, so they inherit them;
//! transaction-local bounds (journal, ledger) and the alarm's own `SET
//! statement_timeout` still override them. A dropped client future then never
//! leaves a server statement, lock wait or open transaction running without a
//! deadline. A position account's pool is built exactly as before.
use std::fmt;

use robson_exec::venue::AccountKind;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use tracing::info;

/// The session defaults of a balance-account daemon, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBounds {
    pub lock_timeout_ms: u64,
    pub statement_timeout_ms: u64,
    pub idle_in_transaction_ms: u64,
}

impl SessionBounds {
    /// PLAN r4 values, kept only while its C1 acceptance rule holds: a lock
    /// wait budget of 2 s, 5 s per statement and 5 s idle in a transaction.
    pub const BALANCE: SessionBounds = SessionBounds {
        lock_timeout_ms: 2_000,
        statement_timeout_ms: 5_000,
        idle_in_transaction_ms: 5_000,
    };

    /// Each GUC with its value, as PostgreSQL displays it in `current_setting`.
    fn settings(self) -> [(&'static str, String); 3] {
        [
            ("lock_timeout", display(self.lock_timeout_ms)),
            ("statement_timeout", display(self.statement_timeout_ms)),
            ("idle_in_transaction_session_timeout", display(self.idle_in_transaction_ms)),
        ]
    }
}

/// PostgreSQL shows a time GUC in the largest unit that keeps it exact.
fn display(ms: u64) -> String {
    match ms {
        0 => "0".to_string(),
        ms if ms % 60_000 == 0 => format!("{}min", ms / 60_000),
        ms if ms % 1_000 == 0 => format!("{}s", ms / 1_000),
        ms => format!("{ms}ms"),
    }
}

/// Append the bounds to whatever Startup options `base` already carries
/// (`PGOPTIONS`, the URL's `options`): later `-c` settings win, earlier ones
/// are kept.
pub fn balance_session_options(base: PgConnectOptions, bounds: SessionBounds) -> PgConnectOptions {
    base.options(bounds.settings())
}

/// The daemon's root pool. A position account's is `PgPool::connect`, as it
/// always was; a balance account's starts every session with
/// [`SessionBounds::BALANCE`].
pub async fn connect_daemon_pool(
    database_url: &str,
    account_kind: AccountKind,
) -> sqlx::Result<PgPool> {
    connect_daemon_pool_with(database_url, account_kind, SessionBounds::BALANCE).await
}

/// [`connect_daemon_pool`] with explicit bounds, for tests that need shorter
/// ones; production passes [`SessionBounds::BALANCE`].
#[doc(hidden)]
pub async fn connect_daemon_pool_with(
    database_url: &str,
    account_kind: AccountKind,
    bounds: SessionBounds,
) -> sqlx::Result<PgPool> {
    match account_kind {
        AccountKind::PositionAccount => sqlx::PgPool::connect(database_url).await,
        AccountKind::BalanceAccount => {
            let options: PgConnectOptions = database_url.parse()?;
            PgPoolOptions::new()
                .connect_with(balance_session_options(options, bounds))
                .await
        },
    }
}

/// Why a balance-account daemon refused to start on its pool.
#[derive(Debug)]
pub enum SessionBoundsError {
    Query(sqlx::Error),
    /// A component on the path dropped or changed the Startup options.
    Mismatch {
        expected: [String; 3],
        observed: [String; 3],
    },
}

impl fmt::Display for SessionBoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query(error) => write!(f, "session bounds could not be read: {error}"),
            Self::Mismatch { expected, observed } => write!(
                f,
                "session bounds not in effect: expected lock_timeout={}, statement_timeout={}, \
                 idle_in_transaction_session_timeout={}; observed {}, {}, {}",
                expected[0], expected[1], expected[2], observed[0], observed[1], observed[2]
            ),
        }
    }
}

impl std::error::Error for SessionBoundsError {}

/// Startup self-check of a balance-account daemon: the bounds are in effect on
/// a pooled connection, or the daemon does not start. One statement per start.
pub async fn verify_session_bounds(
    pool: &PgPool,
    expected: SessionBounds,
) -> Result<(), SessionBoundsError> {
    let (lock, statement, idle): (String, String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'), current_setting('statement_timeout'), \
         current_setting('idle_in_transaction_session_timeout')",
    )
    .fetch_one(pool)
    .await
    .map_err(SessionBoundsError::Query)?;
    let observed = [lock, statement, idle];
    let expected = expected.settings().map(|(_, value)| value);
    if observed != expected {
        return Err(SessionBoundsError::Mismatch { expected, observed });
    }
    info!(
        lock_timeout = %observed[0],
        statement_timeout = %observed[1],
        idle_in_transaction_session_timeout = %observed[2],
        "Balance-account database session bounds in effect"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_shown_the_way_postgres_shows_them() {
        assert_eq!(SessionBounds::BALANCE.settings().map(|(_, value)| value), [
            "2s".to_string(),
            "5s".to_string(),
            "5s".to_string()
        ]);
        assert_eq!(display(100), "100ms");
        assert_eq!(display(1_500), "1500ms");
        assert_eq!(display(120_000), "2min");
        assert_eq!(display(0), "0");
    }
}
