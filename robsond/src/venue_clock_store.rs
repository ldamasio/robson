//! Real exact sample backend, privately bound at committed epoch startup.
//! No HTTP/native sampler or lifecycle publication exists here. A positive
//! receipt proves only row persistence; the future owner must recheck the
//! original receive age and its synchronized epoch/domain/generation after
//! await.
use std::{sync::Arc, time::Duration};

use robson_exec::{
    clock_records::{RawClockSample, ALGORITHM_VERSION, THETA_MAX, THETA_MIN, TICK_MAX, U2_MAX},
    venue::VenueId,
};
use sqlx::{postgres::PgConnectOptions, ConnectOptions, PgConnection, PgPool, Row};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

pub(crate) mod codec;
#[cfg(test)]
mod codec_tests;
mod completion;
mod order_columns;
mod sample;
mod sample_values;

const BOUND: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreError {
    Invalid,
    RepeatedCycle,
    Expired,
    Backend,
    Unpublished,
}

// Private fields: no public constructor can forge positive persistence. This
// result intentionally has no freshness, native-provenance or send permission.
pub(crate) struct PositiveSample {
    _record: sample::Record,
}

pub(crate) struct ClockSampleStore {
    options: Arc<PgConnectOptions>,
    tenant: Uuid,
    epoch: Uuid,
    connection: Option<PgConnection>,
    last_cycle: Option<u64>,
    statements: u64,
}

// Taking the raw connection out of the store makes cancellation drop its socket
// without graceful close/flush. No Transaction or pool return can queue SQL.
struct Session(Option<PgConnection>);
impl Session {
    fn connection(&mut self) -> &mut PgConnection {
        self.0.as_mut().expect("owned initialized session")
    }
}

/// The store's server bounds, sent as connection startup options so they hold
/// from the session's first statement, the settings statement that confirms
/// them included.
#[cfg_attr(not(test), allow(dead_code))]
const SESSION_BOUNDS: [(&str, &str); 3] = [
    ("lock_timeout", "300ms"),
    ("statement_timeout", "1s"),
    ("idle_in_transaction_session_timeout", "1s"),
];

#[cfg_attr(not(test), allow(dead_code))]
fn bounded_options(pool: &PgPool) -> PgConnectOptions {
    (*pool.connect_options()).clone().options(SESSION_BOUNDS)
}

impl ClockSampleStore {
    /// Only the dormant clock owner and its tests construct a store.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_committed_epoch(
        pool: &PgPool,
        tenant: Uuid,
        start: &crate::process_epoch::EpochStart,
    ) -> Self {
        Self {
            options: Arc::new(bounded_options(pool)),
            tenant,
            epoch: start.epoch.epoch_id,
            connection: None,
            last_cycle: None,
            statements: 0,
        }
    }

    // Future actual sampler owns normal cycle numbering and derives confirm_until
    // from its original native receive-age and turnover. Neither argument grants
    // freshness. This hook is private and has no current transport caller.
    #[allow(dead_code)]
    pub(crate) async fn persist(
        &mut self,
        cycle: u64,
        raw: RawClockSample,
        confirm_until: Option<Instant>,
    ) -> Result<PositiveSample, StoreError> {
        let started = Instant::now();
        let deadline = confirm_until.map_or(started + BOUND, |until| until.min(started + BOUND));
        if self.last_cycle.is_some_and(|last| cycle <= last) {
            return Err(StoreError::RepeatedCycle);
        }
        // Spent before validation/connect/await; cancellation cannot refund it.
        self.last_cycle = Some(cycle);
        if raw.tenant != self.tenant
            || raw.receive.epoch != self.epoch
            || raw.send.epoch != self.epoch
            || raw.venue.as_str() != VenueId::BINANCE_SPOT
        {
            return Err(StoreError::Invalid);
        }
        let candidate = sample::Record::freeze(raw).map_err(|_| StoreError::Invalid)?;
        if Instant::now() >= deadline {
            return Err(StoreError::Expired);
        }
        let mut held = Session(self.connection.take());
        // No INSERT has been attempted during initialization. Its failure must
        // never create a needless confirmation, even for a transport error.
        timeout_at(deadline, self.initialize(&mut held))
            .await
            .map_err(|_| StoreError::Expired)??;
        if Instant::now() >= deadline {
            return Err(StoreError::Expired);
        }
        let first = timeout_at(deadline, self.execute(&mut held, &candidate, true)).await;
        match first {
            Ok(Ok(())) if Instant::now() < deadline => {
                self.connection = held.0.take();
                return Ok(PositiveSample { _record: candidate });
            },
            Ok(Err(error @ (StoreError::Invalid | StoreError::Unpublished))) => return Err(error),
            _ => {},
        }
        // Ambiguity is not absence. Discard the first socket and perform at most
        // one fresh confirmation using the SAME absolute deadline, never INSERT.
        drop(held);
        if confirm_until.is_none() || Instant::now() >= deadline {
            return Err(StoreError::Unpublished);
        }
        let mut fresh = Session(None);
        let confirmed = timeout_at(deadline, async {
            self.initialize(&mut fresh).await?;
            if Instant::now() >= deadline {
                return Err(StoreError::Expired);
            }
            self.execute(&mut fresh, &candidate, false).await
        })
        .await;
        if matches!(confirmed, Ok(Ok(()))) && Instant::now() < deadline {
            self.connection = fresh.0.take();
            Ok(PositiveSample { _record: candidate })
        } else {
            Err(StoreError::Unpublished)
        }
    }

    async fn initialize(&mut self, held: &mut Session) -> Result<(), StoreError> {
        if held.0.is_some() {
            return Ok(());
        }
        held.0 = Some(self.options.connect().await.map_err(completion::classify)?);
        self.statements += 1;
        let row = completion::one(
            held.connection(),
            sqlx::query(
                "SELECT set_config('lock_timeout','300ms',false) AS lock_limit,
            set_config('statement_timeout','1s',false) AS statement_limit,
            set_config('idle_in_transaction_session_timeout','1s',false) AS idle_limit",
            ),
        )
        .await?;
        if row.len() != 3 {
            return Err(StoreError::Invalid);
        }
        for (key, expected) in [
            ("lock_limit", "300ms"),
            ("statement_limit", "1s"),
            ("idle_limit", "1s"),
        ] {
            if sample_values::text(&row, key, expected.len()).map_err(|_| StoreError::Invalid)?
                != expected
            {
                return Err(StoreError::Invalid);
            }
        }
        Ok(())
    }

    async fn execute(
        &mut self,
        held: &mut Session,
        c: &sample::Record,
        insert: bool,
    ) -> Result<(), StoreError> {
        self.statements += 1;
        let row = if insert {
            let r = &c.raw;
            let n = c.numbers;
            let (offset_seconds, offset_nanos) = r.send.domain.offset();
            let numeric =
                |value, lo, hi| codec::decimal(value, lo, hi).map_err(|_| StoreError::Invalid);
            let sql=format!("INSERT INTO clock_samples AS s(sample_id,tenant_id,venue_id,process_epoch,clock_domain,
                send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,
                receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,
                server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at)
                SELECT $1,$2,$3,$4,ROW($5,$6,$7,$8)::budget_clock_domain_v38,$9,$10,$11,$12,$13,$14,$15,$16,
                $17,$18,$19,$20,$21,$22,$23,$24,$25 FROM process_epochs e WHERE e.epoch_id=$4 AND e.tenant_id=$2
                RETURNING {}",sample::FIELDS);
            completion::one(
                held.connection(),
                sqlx::query(&sql)
                    .bind(r.id)
                    .bind(r.tenant)
                    .bind(r.venue.as_str())
                    .bind(self.epoch)
                    .bind("boottime")
                    .bind(r.send.domain.boot())
                    .bind(offset_seconds)
                    .bind(offset_nanos as i32)
                    .bind(r.send.wall_seconds)
                    .bind(r.send.wall_nanoseconds as i32)
                    .bind(numeric(r.send.before_ns, 0, TICK_MAX)?)
                    .bind(numeric(r.send.after_ns, 0, TICK_MAX)?)
                    .bind(r.receive.wall_seconds)
                    .bind(r.receive.wall_nanoseconds as i32)
                    .bind(numeric(r.receive.before_ns, 0, TICK_MAX)?)
                    .bind(numeric(r.receive.after_ns, 0, TICK_MAX)?)
                    .bind(r.server_ms)
                    .bind(numeric(n.theta2, THETA_MIN, THETA_MAX)?)
                    .bind(numeric(n.r_low, 0, TICK_MAX)?)
                    .bind(numeric(n.r_high, 0, TICK_MAX)?)
                    .bind(numeric(n.u2, 0, U2_MAX)?)
                    .bind(if n.failures == 0 { "valid" } else { "invalid" })
                    .bind(n.failures)
                    .bind(ALGORITHM_VERSION)
                    .bind(c.audit),
            )
            .await?
        } else {
            let sql=format!("SELECT {} FROM clock_samples s JOIN process_epochs e ON e.epoch_id=s.process_epoch
                WHERE s.sample_id=$1 AND s.tenant_id=$2 AND s.venue_id=$3 AND s.process_epoch=$4 AND e.tenant_id=$2",sample::FIELDS);
            completion::one(
                held.connection(),
                sqlx::query(&sql)
                    .bind(c.raw.id)
                    .bind(self.tenant)
                    .bind(c.raw.venue.as_str())
                    .bind(self.epoch),
            )
            .await?
        };
        let decoded = sample::decode(&row).map_err(|_| StoreError::Invalid)?;
        if &decoded != c {
            return Err(StoreError::Unpublished);
        }
        Ok(())
    }
}

#[cfg(test)]
mod backend_tests;

#[cfg(test)]
#[path = "../tests/venue_clock_store_fixture/legacy_pg.rs"]
pub(crate) mod legacy_pg;
