//! ADR-0051 §1 accounting alarm for an invalid monthly budget basis.
//!
//! MonthlyHalt does not fire on `capital_base <= 0`, and every other reader of
//! the basis needs traffic (a tick on an open position, a close, a signal, a
//! `/status` request), so this worker reads the persisted row on its own timer.
//! The row is the durable state: the first read after a restart raises the
//! alarm again.
//!
//! It shares nothing with the daemon's main loop or the position manager, and
//! it reads on one connection of its own: a timed-out read can leave that
//! connection busy until the database answers, and it must not be able to take
//! the daemon's. So this worker adds no way for a stalled database to hold up
//! market data, trailing stops or operator exits. The daemon's own database
//! calls still wait on it as they always have.

use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use prometheus::Gauge;
use rust_decimal::Decimal;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

const READ_INTERVAL: Duration = Duration::from_secs(60);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Publishes whether the current month's persisted budget basis is invalid.
pub struct BudgetBasisAlarm {
    pool: PgPool,
    invalid: Gauge,
    last_read: Gauge,
    read_interval: Duration,
    read_timeout: Duration,
}

impl BudgetBasisAlarm {
    /// `daemon_pool` only supplies the connection settings.
    pub fn new(daemon_pool: &PgPool) -> Self {
        Self::with_metrics(
            daemon_pool,
            crate::metrics::MONTHLY_BUDGET_BASIS_INVALID.clone(),
            crate::metrics::MONTHLY_BUDGET_BASIS_LAST_READ_TIMESTAMP_SECONDS.clone(),
            READ_TIMEOUT,
        )
    }

    fn with_metrics(
        daemon_pool: &PgPool,
        invalid: Gauge,
        last_read: Gauge,
        read_timeout: Duration,
    ) -> Self {
        // The server enforces the bound too: dropping a timed-out future would
        // leave the statement running and holding its locks.
        let statement_timeout = format!("SET statement_timeout = {}", read_timeout.as_millis());
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(read_timeout)
            .after_connect(move |connection, _| {
                let statement_timeout = statement_timeout.clone();
                Box::pin(async move {
                    connection.execute(statement_timeout.as_str()).await?;
                    Ok(())
                })
            })
            .connect_lazy_with((*daemon_pool.connect_options()).clone());
        Self {
            pool,
            invalid,
            last_read,
            read_interval: READ_INTERVAL,
            read_timeout,
        }
    }

    /// Read the basis on a timer until shutdown, starting immediately.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.read_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => self.refresh(Utc::now()).await,
            }
        }
        // Bounded: a connection a stalled read left busy must not hold up shutdown.
        let _ = tokio::time::timeout(self.read_timeout, self.pool.close()).await;
    }

    /// One bounded read. When it fails nothing is published: the outage says
    /// nothing about the basis, and the last-read timestamp stops advancing.
    async fn refresh(&self, now: DateTime<Utc>) {
        let read = sqlx::query_scalar::<_, Decimal>(
            "SELECT capital_base FROM monthly_state WHERE year = $1 AND month = $2",
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_optional(&self.pool);

        // Twice the server's bound: the backstop for a database that cannot even
        // cancel its statement.
        let capital_base = match tokio::time::timeout(self.read_timeout * 2, read).await {
            Ok(Ok(capital_base)) => capital_base,
            Ok(Err(e)) => {
                warn!(error = %e, "Failed to read the monthly budget basis for its alarm");
                return;
            },
            Err(_) => {
                warn!(
                    timeout_secs = self.read_timeout.as_secs(),
                    "Timed out reading the monthly budget basis for its alarm"
                );
                return;
            },
        };

        // A missing row is not invalid: until the month boundary writes one, the
        // budget snapshot falls back to the configured capital.
        let invalid = capital_base.is_some_and(|base| base <= Decimal::ZERO);
        self.invalid.set(if invalid { 1.0 } else { 0.0 });
        self.last_read.set(now.timestamp() as f64);
        if invalid {
            error!(
                year = now.year(),
                month = now.month(),
                "ADR-0051 §1: monthly budget basis is invalid because capital_base <= 0; new entries denied, open positions kept"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    use super::*;

    fn alarm_with_timeout(pool: &PgPool, read_timeout: Duration) -> BudgetBasisAlarm {
        let mut alarm = BudgetBasisAlarm::with_metrics(
            pool,
            Gauge::new("test_budget_basis_invalid", "Test").unwrap(),
            Gauge::new("test_budget_basis_last_read", "Test").unwrap(),
            read_timeout,
        );
        alarm.read_interval = Duration::from_millis(20);
        alarm
    }

    fn alarm(pool: &PgPool) -> BudgetBasisAlarm {
        alarm_with_timeout(pool, READ_TIMEOUT)
    }

    /// A month no migration seeds a row for.
    fn month_with_no_row() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 0).unwrap()
    }

    async fn set_basis(pool: &PgPool, now: DateTime<Utc>, capital_base: Decimal) {
        sqlx::query(
            r#"
            INSERT INTO monthly_state (
                year, month, capital_base, month_peak_net,
                monthly_budget_model, created_at
            )
            VALUES ($1, $2, $3, 0, 'net_from_start_non_expanding_v1', $4)
            ON CONFLICT (year, month) DO UPDATE SET capital_base = EXCLUDED.capital_base
            "#,
        )
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .bind(capital_base)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn the_alarm_follows_the_persisted_basis(pool: PgPool) {
        let alarm = alarm(&pool);
        let now = month_with_no_row();

        alarm.invalid.set(-1.0);
        alarm.refresh(now).await;
        assert_eq!(alarm.invalid.get(), 0.0, "a missing row is not invalid");
        assert_eq!(alarm.last_read.get(), now.timestamp() as f64);

        set_basis(&pool, now, Decimal::ZERO).await;
        alarm.refresh(now).await;
        assert_eq!(alarm.invalid.get(), 1.0);

        set_basis(&pool, now, dec!(1805.82)).await;
        alarm.refresh(now).await;
        assert_eq!(alarm.invalid.get(), 0.0);
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_restart_raises_the_alarm_again_from_the_row(pool: PgPool) {
        let now = month_with_no_row();
        set_basis(&pool, now, Decimal::ZERO).await;

        let before_restart = alarm(&pool);
        before_restart.refresh(now).await;
        assert_eq!(before_restart.invalid.get(), 1.0);
        drop(before_restart);

        let after_restart = alarm(&pool);
        assert_eq!(after_restart.invalid.get(), 0.0);
        after_restart.refresh(now).await;
        assert_eq!(after_restart.invalid.get(), 1.0);
    }

    /// A process whose reads never worked exports the same 0 as a valid basis.
    /// The timestamp, still 0, is what tells them apart.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_restart_with_failing_reads_shows_as_never_read(pool: PgPool) {
        let now = month_with_no_row();
        set_basis(&pool, now, Decimal::ZERO).await;
        let alarm = alarm(&pool);
        alarm.pool.close().await;

        alarm.refresh(now).await;

        assert_eq!(alarm.invalid.get(), 0.0);
        assert_eq!(alarm.last_read.get(), 0.0);
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn the_timer_raises_the_alarm_with_no_traffic_and_stops_on_shutdown(pool: PgPool) {
        set_basis(&pool, Utc::now(), Decimal::ZERO).await;
        let alarm = alarm(&pool);
        let invalid = alarm.invalid.clone();
        let own_pool = alarm.pool.clone();
        let shutdown = CancellationToken::new();
        let worker = tokio::spawn(alarm.run(shutdown.clone()));

        tokio::time::timeout(Duration::from_secs(10), async {
            while invalid.get() != 1.0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the timer must read the basis with nothing else asking");

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("the worker must stop on shutdown")
            .unwrap();
        assert!(own_pool.is_closed(), "it must give its connection back");
    }

    async fn lock_monthly_state(pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
        let mut stall = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE monthly_state IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *stall)
            .await
            .unwrap();
        stall
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_failed_read_publishes_nothing(pool: PgPool) {
        let now = month_with_no_row();
        set_basis(&pool, now, Decimal::ZERO).await;
        let alarm = alarm_with_timeout(&pool, Duration::from_millis(100));
        alarm.refresh(now).await;
        assert_eq!(alarm.invalid.get(), 1.0);
        let later = now + chrono::Duration::minutes(1);

        // A stalled database: the read of a valid basis waits on this lock.
        set_basis(&pool, now, dec!(1805.82)).await;
        let stall = lock_monthly_state(&pool).await;
        tokio::time::timeout(Duration::from_secs(5), alarm.refresh(later))
            .await
            .expect("the read must give up at its own bound");
        assert_eq!(alarm.invalid.get(), 1.0, "a timed-out read says nothing about the basis");
        assert_eq!(alarm.last_read.get(), now.timestamp() as f64);
        stall.rollback().await.unwrap();

        // Database gone.
        alarm.pool.close().await;
        alarm.refresh(later).await;
        assert_eq!(alarm.invalid.get(), 1.0, "a failed read says nothing about the basis");
        assert_eq!(alarm.last_read.get(), now.timestamp() as f64);
    }

    /// The server gives up as well: a read past its bound keeps no statement
    /// waiting and the connection is good for the next read.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_timed_out_read_leaves_nothing_running_on_the_server(pool: PgPool) {
        let now = month_with_no_row();
        set_basis(&pool, now, Decimal::ZERO).await;
        let alarm = alarm_with_timeout(&pool, Duration::from_millis(100));

        let stall = lock_monthly_state(&pool).await;
        alarm.refresh(now).await;
        assert_eq!(alarm.last_read.get(), 0.0);

        let waiting: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM pg_locks
            WHERE NOT granted
              AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
              AND relation = 'monthly_state'::regclass
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(waiting, 0, "the timed-out statement must be gone");
        stall.rollback().await.unwrap();

        alarm.refresh(now).await;
        assert_eq!(alarm.invalid.get(), 1.0);
    }

    /// Reads that time out hold their connection until the database answers.
    /// However many pile up, the daemon's pool keeps its capacity.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn stalled_reads_take_no_connection_from_the_daemons_pool(pool: PgPool) {
        let now = month_with_no_row();
        let daemon_pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let alarm = alarm_with_timeout(&daemon_pool, Duration::from_millis(50));

        let stall = lock_monthly_state(&daemon_pool).await;
        for _ in 0..4 {
            alarm.refresh(now).await;
        }

        assert!(alarm.pool.size() <= 1, "{}", alarm.pool.size());
        let answer: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&daemon_pool)
            .await
            .expect("the daemon's last free connection must still be free");
        assert_eq!(answer, 1);
        stall.rollback().await.unwrap();
    }
}
