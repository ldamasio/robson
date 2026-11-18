//! Retirement for balance-account transactions only. Legacy journal retirement
//! keeps its existing graceful-close behavior.
use sqlx::{pool::PoolConnection, PgConnection, Postgres};

pub(crate) struct LedgerConnection {
    connection: Option<PoolConnection<Postgres>>,
    pub(crate) finished: bool,
}

impl LedgerConnection {
    pub(crate) fn new(connection: PoolConnection<Postgres>) -> Self {
        Self {
            connection: Some(connection),
            finished: false,
        }
    }

    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.connection.as_mut().expect("connection is taken only during drop")
    }
}

impl Drop for LedgerConnection {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(connection) = self.connection.take() {
                // SQLx 0.8.6 queues ROLLBACK when an unfinished Transaction
                // drops, including a rejected COMMIT. Graceful close flushes
                // it and adds an uncounted statement. Detach releases the pool
                // permit; immediately dropping the raw connection discards its
                // buffered SQL and closes its socket without an async flush.
                // Never leak(): that would permanently consume a pool slot.
                drop(connection.detach());
            }
        }
    }
}
