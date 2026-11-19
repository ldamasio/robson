//! Bounded cold N5 storage observation. Fee validation and selection belong
//! to the consumer of an actual completed native read.
use std::{
    collections::HashSet,
    fmt,
    future::poll_fn,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use sqlx::{
    postgres::PgRow, ConnectOptions, Decode, Either, Executor, PgPool, Postgres, Row, Type,
};
use tokio::time::{timeout_at, Instant};

const SQL: &str = include_str!("n5_cold_load.sql");
const STORE_LIMIT: Duration = Duration::from_secs(2);
pub const MAX_KEYS: usize = 4096;
pub const MAX_ROWS: usize = MAX_KEYS + 1;
pub const MAX_ACTIVE: usize = 64;
pub const MAX_PAYLOAD_BYTES: usize = 16_384;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COMPACT_WIRE_BYTES: usize = 1536;
pub const MAX_ACTIVE_METADATA_WIRE_BYTES: usize = 1024;
pub const MAX_MESSAGE_METADATA_WIRE_BYTES: usize = 8192;

// The 55-column binary DataRow has 227 bytes of framing. The largest compact
// values are bounded by 86 root bytes, 236 registration bytes, and 22 state
// bytes. Active rows add at most 549 bytes of metadata and 16 KiB of payload.
// The reserves below also cover message descriptions and completion. The
// lifetime sentinel counts as a compact row.
const _: () = assert!(
    MAX_ROWS * MAX_COMPACT_WIRE_BYTES
        + MAX_ACTIVE * (MAX_ACTIVE_METADATA_WIRE_BYTES + MAX_PAYLOAD_BYTES)
        + MAX_MESSAGE_METADATA_WIRE_BYTES
        <= MAX_BYTES
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("the native N5 read or data shape is invalid")]
    Invalid,
    #[error("the native N5 read backend failed")]
    Backend,
    #[error("the native N5 read has no complete bounded result")]
    Unpublished,
    #[error("the native N5 read expired")]
    Expired,
}

fn classify(error: sqlx::Error) -> ReadError {
    match error {
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => ReadError::Backend,
        _ => ReadError::Invalid,
    }
}

/// Evidence of completing the specific bounded native query. Callers can
/// inspect rows but cannot construct this value from a raw row or timestamp.
///
/// ```compile_fail
/// use robson_store::native_n5::Observation;
/// let _forged = Observation { rows: Vec::new() };
/// ```
#[must_use]
pub struct Observation {
    rows: Vec<ObservedRow>,
}

impl Observation {
    pub fn rows(&self) -> &[ObservedRow] {
        &self.rows
    }
}

impl fmt::Debug for Observation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Observation").field("rows", &self.rows.len()).finish()
    }
}

/// Immutable typed access to an observed row. Getter errors cannot become
/// positive N5 results.
pub struct ObservedRow(PgRow);

impl ObservedRow {
    pub fn try_get<'r, T>(&'r self, column: &str) -> Result<T, ReadError>
    where
        T: Decode<'r, Postgres> + Type<Postgres>,
    {
        self.0.try_get(column).map_err(classify)
    }
}

/// Read once on a raw owned connection. The statement counter observes the
/// one issue boundary and supplies no input to the returned observation.
pub async fn read(
    pool: &PgPool,
    symbols: &[String],
    deadline: Instant,
    statements_issued: &AtomicU64,
) -> Result<Observation, ReadError> {
    if symbols.is_empty()
        || symbols.len() > MAX_ACTIVE
        || symbols.iter().any(|symbol| symbol.is_empty() || symbol.len() > 64)
        || symbols.iter().collect::<HashSet<_>>().len() != symbols.len()
    {
        return Err(ReadError::Invalid);
    }
    let deadline = deadline.min(Instant::now() + STORE_LIMIT);
    let work = async {
        if Instant::now() >= deadline {
            return Err(ReadError::Expired);
        }
        let options = (*pool.connect_options()).clone().options([
            ("statement_timeout", "1s"),
            ("idle_in_transaction_session_timeout", "1s"),
        ]);
        // Cancelled or partial work drops this raw connection. No pool return,
        // Transaction destructor or graceful close can flush extra protocol.
        let mut connection = options.connect().await.map_err(classify)?;
        if Instant::now() >= deadline {
            return Err(ReadError::Expired);
        }
        statements_issued.fetch_add(1, Ordering::Relaxed);
        let query = sqlx::query(SQL).bind(symbols);
        let mut stream = connection.fetch_many(query);
        let mut rows = Vec::with_capacity(MAX_ROWS);
        let mut command = None;
        while let Some(next) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            match next.map_err(classify)? {
                Either::Right(row) if command.is_none() && rows.len() < MAX_ROWS => {
                    rows.push(ObservedRow(row));
                },
                Either::Left(result) if command.is_none() => {
                    command = Some(result.rows_affected());
                },
                _ => return Err(ReadError::Unpublished),
            }
        }
        if command != Some(rows.len() as u64) || rows.is_empty() || rows.len() > MAX_KEYS {
            return Err(ReadError::Unpublished);
        }
        if Instant::now() >= deadline {
            return Err(ReadError::Expired);
        }
        Ok(Observation { rows })
    };
    timeout_at(deadline, work).await.map_err(|_| ReadError::Expired)?
}
