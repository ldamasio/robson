//! PS-2b on PG16: one keeper write per condition of the never-shorten
//! trigger (a)-(e), the statements it sends (4, 5 when skipped, + 1 after a
//! lost COMMIT ACK), the landing rule by outcome, and a database that is down.
use anyhow::{anyhow, ensure, Result};
use robson_exec::venue_clock::{ClockDomain, TickCapture};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::postgres::{PgPoolOptions, PgSslMode};

use super::*;

const SECOND: i128 = 1_000_000_000;

fn tick(domain: ClockDomain, nanoseconds: i128) -> TickCapture {
    TickCapture::new(domain, (nanoseconds / SECOND) as i64, (nanoseconds % SECOND) as i64).unwrap()
}

/// A 429 observed at `at` (ns) in `domain`, pausing for `seconds`.
fn observed(domain: ClockDomain, at: i128, seconds: u64) -> PauseObservation {
    PauseObservation {
        id: Uuid::now_v7(),
        status: 429,
        observed: tick(domain, at),
        seconds,
        end: tick(domain, at + i128::from(seconds) * SECOND),
        until: None,
    }
}

async fn wire(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("no upstream"))?;
    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_exact_query_trace(16);
    let wired = PgPoolOptions::new().max_connections(1).connect_lazy_with(
        (*options)
            .clone()
            .host("127.0.0.1")
            .port(proxy.addr().port())
            .ssl_mode(PgSslMode::Disable),
    );
    Ok((proxy, wired))
}

/// The stored row: (observation, restoration boot, anchor, deadline,
/// duration, recorded_at µs).
type Row = (Uuid, Uuid, String, String, String, i64);

async fn stored_row(pool: &PgPool, tenant: Uuid) -> Result<Option<Row>> {
    Ok(sqlx::query_as(
        "SELECT observation_id, (restoration_domain).boot_uuid, restoration_anchor::text,
                deadline_tick::text, duration_ns::text,
                (extract(epoch FROM recorded_at)*1000000)::bigint
         FROM venue_pauses WHERE tenant_id = $1 AND venue_id = 'binance:spot'",
    )
    .bind(tenant)
    .fetch_optional(pool)
    .await?)
}

/// Another process's row, restored in `domain` at `anchor` for `seconds`.
async fn foreign_row(
    pool: &PgPool,
    tenant: Uuid,
    domain: ClockDomain,
    anchor: i128,
    seconds: u64,
    recorded_at: chrono::DateTime<Utc>,
) -> Result<Uuid> {
    let observation = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO venue_pauses(tenant_id,venue_id,status,code,pause_state,observation_id,
             original_domain,observed_tick,duration_ns,restoration_domain,restoration_anchor,
             deadline_tick,until_at,recorded_at)
         SELECT $1,'binance:spot',429,NULL,'bounded',$2,d,$6::text::numeric,$7::text::numeric,d,
                $6::text::numeric,$6::text::numeric+$7::text::numeric,NULL,$8
         FROM (SELECT ROW('boottime'::text,$3::uuid,$4::bigint,$5::integer)
                      ::budget_clock_domain_v38 AS d) v",
    )
    .bind(tenant)
    .bind(observation)
    .bind(domain.boot())
    .bind(domain.offset().0)
    .bind(i32::try_from(domain.offset().1)?)
    .bind(anchor.to_string())
    .bind((i128::from(seconds) * SECOND).to_string())
    .bind(recorded_at)
    .execute(pool)
    .await?;
    Ok(observation)
}

/// What `persist` sent, each statement named.
fn shapes(proxy: &DbProxy) -> Vec<&'static str> {
    proxy
        .query_trace()
        .iter()
        .map(|sql| {
            let body: String = sql.lines().filter(|l| !l.trim_start().starts_with("--")).collect();
            match body.trim() {
                b if b.starts_with("BEGIN") => "begin",
                b if b.starts_with("SELECT set_config") => "bounds",
                b if b.starts_with("WITH w AS (") => "write",
                b if b.starts_with("SELECT p.tenant_id IS NOT NULL") => "read",
                b if b.starts_with("COMMIT") => "commit",
                _ => "other",
            }
        })
        .collect()
}

fn micros(at: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp_micros(at.timestamp_micros()).unwrap()
}

/// (a) and the landing rule: a fresh observation lands as written in four
/// statements; a longer one replaces it (the conflict target is the
/// tenant's row, never another tenant's).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn condition_a_the_keeper_updates_only_its_tenants_row(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let (tenant, other) = (Uuid::now_v7(), Uuid::now_v7());
    let (proxy, wired) = wire(&pool).await?;
    let first = observed(domain, 1_000 * SECOND, 30);
    let trace = Trace::default();
    ensure!(persist(&wired, tenant, &first, &clock, &trace).await == Ok(true));
    ensure!(shapes(&proxy) == ["begin", "bounds", "write", "commit"], "{:?}", shapes(&proxy));
    ensure!(trace.issued() == 4);
    let longer = observed(domain, 1_010 * SECOND, 60);
    ensure!(persist(&pool, tenant, &longer, &clock, &Trace::default()).await == Ok(true));
    ensure!(stored_row(&pool, tenant).await?.is_some_and(|row| row.0 == longer.id));
    let theirs = observed(domain, 1_020 * SECOND, 5);
    ensure!(persist(&pool, other, &theirs, &clock, &Trace::default()).await == Ok(true));
    ensure!(stored_row(&pool, tenant).await?.is_some_and(|row| row.0 == longer.id));
    ensure!(stored_row(&pool, other).await?.is_some_and(|row| row.0 == theirs.id));
    Ok(())
}

/// (b): a stored `recorded_at` ahead of the wall clock (a backward wall
/// step) never blocks a longer pause from landing; the row keeps the stored
/// maximum and the keeper's check accepts it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_backward_wall_step_never_blocks_a_pause_from_landing(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let tenant = Uuid::now_v7();
    let ahead = micros(Utc::now() + chrono::Duration::hours(1));
    foreign_row(&pool, tenant, domain, 1_000 * SECOND, 30, ahead).await?;
    let longer = observed(domain, 1_010 * SECOND, 60);
    let landed = persist(&pool, tenant, &longer, &clock, &Trace::default()).await;
    ensure!(landed == Ok(true), "{landed:?}");
    let row = stored_row(&pool, tenant).await?.ok_or_else(|| anyhow!("no row"))?;
    ensure!(row.0 == longer.id && row.5 == ahead.timestamp_micros(), "{row:?}");
    Ok(())
}

/// (c): the same observation lands again only with equal identity fields
/// (a replay); with another duration it is skipped (5 statements) and,
/// shorter than what it claims, does not land.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn condition_c_replays_only_an_identical_observation(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let tenant = Uuid::now_v7();
    let first = observed(domain, 1_000 * SECOND, 30);
    ensure!(persist(&pool, tenant, &first, &clock, &Trace::default()).await == Ok(true));
    let before = stored_row(&pool, tenant).await?;
    ensure!(
        persist(&pool, tenant, &first, &clock, &Trace::default()).await == Ok(true),
        "replay"
    );
    let forged = PauseObservation {
        seconds: 90,
        end: tick(domain, 1_090 * SECOND),
        ..first
    };
    let (proxy, wired) = wire(&pool).await?;
    let skipped = persist(&wired, tenant, &forged, &clock, &Trace::default()).await;
    ensure!(skipped == Ok(false), "{skipped:?}");
    ensure!(
        shapes(&proxy) == ["begin", "bounds", "write", "read", "commit"],
        "{:?}",
        shapes(&proxy)
    );
    let after = stored_row(&pool, tenant).await?;
    ensure!(after.as_ref().map(|r| (&r.0, &r.3)) == before.as_ref().map(|r| (&r.0, &r.3)));
    Ok(())
}

/// (d) and the PLAN's shortening killer: a later, shorter 429 in the same
/// clock domain never shortens the row; the update is skipped and the
/// stored row, covering it, counts as landed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_later_shorter_pause_never_shortens_the_row(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let tenant = Uuid::now_v7();
    let long = observed(domain, 1_000 * SECOND, 120);
    ensure!(persist(&pool, tenant, &long, &clock, &Trace::default()).await == Ok(true));
    let short = observed(domain, 1_010 * SECOND, 30);
    let landed = persist(&pool, tenant, &short, &clock, &Trace::default()).await;
    ensure!(landed == Ok(true), "the covering row counts as landed: {landed:?}");
    let row = stored_row(&pool, tenant).await?.ok_or_else(|| anyhow!("no row"))?;
    ensure!(row.0 == long.id && row.3 == (1_120 * SECOND).to_string(), "{row:?}");
    Ok(())
}

/// (e) and the PLAN's skipped-row killer: a row another process stored in
/// another clock domain under another observation, longer and with an older
/// `recorded_at`, is validated on its own and covers a shorter observation:
/// the update is skipped and the pause lands; a longer observation replaces
/// it (its remaining duration is no shorter than the stored one).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_covering_row_from_another_observation_lands_the_pause(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let now = clock.capture()?;
    let here = now.domain();
    let elsewhere = ClockDomain::new(Uuid::now_v7(), 0, 0)?;
    let tenant = Uuid::now_v7();
    let older = micros(Utc::now() - chrono::Duration::minutes(10));
    let theirs = foreign_row(&pool, tenant, elsewhere, 5 * SECOND, 300, older).await?;
    let at = i128::try_from(now.nanoseconds())?;
    let short = observed(here, at, 60);
    let landed = persist(&pool, tenant, &short, &clock, &Trace::default()).await;
    ensure!(landed == Ok(true), "{landed:?}");
    ensure!(stored_row(&pool, tenant).await?.is_some_and(|row| row.0 == theirs));
    let long = observed(here, at, 400);
    ensure!(persist(&pool, tenant, &long, &clock, &Trace::default()).await == Ok(true));
    let row = stored_row(&pool, tenant).await?.ok_or_else(|| anyhow!("no row"))?;
    ensure!(row.0 == long.id && row.1 == here.boot(), "{row:?}");
    Ok(())
}

/// A lost COMMIT ACK: the write landed; one confirmation read on a fresh
/// connection (6 statements in all) finds it and the pause lands. A replay
/// of the same observation afterwards lands as equal.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_commit_ack_is_confirmed_by_one_read(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let tenant = Uuid::now_v7();
    let (proxy, wired) = wire(&pool).await?;
    proxy.set_mode(ProxyMode::DropCommitAck);
    let first = observed(domain, 1_000 * SECOND, 30);
    let trace = Trace::default();
    let landed = persist(&wired, tenant, &first, &clock, &trace).await;
    ensure!(landed == Ok(true), "{landed:?}");
    ensure!(
        shapes(&proxy) == ["begin", "bounds", "write", "commit", "read"],
        "{:?}",
        shapes(&proxy)
    );
    ensure!(trace.issued() == 5, "four statements and the confirmation");
    ensure!(stored_row(&pool, tenant).await?.is_some_and(|row| row.0 == first.id));
    ensure!(
        persist(&pool, tenant, &first, &clock, &Trace::default()).await == Ok(true),
        "replay"
    );
    Ok(())
}

/// A database that answers nothing: the write fails by its 2 s deadline,
/// nothing is stored, the pause does not land.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_database_that_is_down_lands_nothing(pool: PgPool) -> Result<()> {
    let clock = NativeClock::initialize()?;
    let domain = clock.capture()?.domain();
    let tenant = Uuid::now_v7();
    let (proxy, wired) = wire(&pool).await?;
    proxy.set_mode(ProxyMode::Blackhole);
    let began = Instant::now();
    let down =
        persist(&wired, tenant, &observed(domain, 1_000 * SECOND, 30), &clock, &Trace::default())
            .await;
    ensure!(down == Err(BudgetError::Expired), "{down:?}");
    ensure!(began.elapsed() < Duration::from_millis(2_500));
    ensure!(stored_row(&pool, tenant).await?.is_none());
    Ok(())
}
