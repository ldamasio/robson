//! PS-2a on PG16: the non-trading bootstrap (5 statements), the stored pause
//! (refused when unresolved, honoured in the same clock domain, re-anchored
//! to its full duration in a new one, never blocked by a backward wall step)
//! and the budget's grant window and unclean barrier on injected ticks. No
//! real time is waited for.
use std::time::Duration;

use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Utc};
use robson_exec::venue_clock::{ClockDomain, NativeClock, TickCapture};
use robson_testkit::db_proxy::DbProxy;
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    bootstrap, restore,
    state::{NonTradingBudget, SpotRead},
    BudgetError, Trace,
};
use crate::process_epoch::{self, EpochStart};

const SECOND: i128 = 1_000_000_000;

/// A start after a predecessor that stopped clean.
async fn clean_start(pool: &PgPool, tenant: Uuid) -> Result<EpochStart> {
    let predecessor = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version, clean_stop_at, \
         marker_version) VALUES ($1, $2, 'ps-2a-predecessor', now(), $3)",
    )
    .bind(predecessor)
    .bind(tenant)
    .bind(process_epoch::MARKER_VERSION)
    .execute(pool)
    .await?;
    let start = process_epoch::start_epoch(pool, tenant, "ps-2a-test").await?;
    ensure!(start.predecessor.unclean_reason().is_none());
    ensure!(start.predecessor_epoch_id == Some(predecessor));
    Ok(start)
}

/// A pool whose connections go through a proxy that traces every statement.
async fn wire(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("no upstream address"))?;
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

fn tick(domain: ClockDomain, nanoseconds: i128) -> Result<TickCapture> {
    Ok(TickCapture::new(
        domain,
        i64::try_from(nanoseconds / SECOND)?,
        i64::try_from(nanoseconds % SECOND)?,
    )?)
}

fn ns(capture: TickCapture) -> Result<i128> {
    Ok(i128::try_from(capture.nanoseconds())?)
}

/// `start + offset` nanoseconds, in the start's domain.
fn at(start: TickCapture, offset: i128) -> Result<TickCapture> {
    tick(start.domain(), ns(start)? + offset)
}

/// No TIME or read reservation at `now` (deadline 10 s later).
fn refused(budget: &NonTradingBudget, now: TickCapture) -> Result<bool> {
    let deadline = at(now, 10 * SECOND)?;
    Ok(matches!(budget.try_reserve_time(now, deadline), Err(BudgetError::Deferred))
        && matches!(
            budget.try_reserve_read(SpotRead::Account, now, deadline),
            Err(BudgetError::Deferred)
        ))
}

/// TIME and a read are both reserved at `now`, then released.
fn granted(budget: &NonTradingBudget, now: TickCapture) -> Result<()> {
    let deadline = at(now, 10 * SECOND)?;
    budget
        .try_reserve_time(now, deadline)
        .map_err(|e| anyhow!("TIME at {now:?}: {e:?}"))?;
    let read = budget
        .try_reserve_read(SpotRead::Account, now, deadline)
        .map_err(|e| anyhow!("ACCOUNT at {now:?}: {e:?}"))?;
    ensure!(read.read() == SpotRead::Account);
    Ok(())
}

async fn bootstrap(
    pool: &PgPool,
    tenant: Uuid,
    start: &EpochStart,
    clock: &NativeClock,
    trace: &Trace,
) -> Result<NonTradingBudget, BudgetError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    bootstrap::non_trading(pool, tenant, start, clock, deadline, trace).await
}

/// A stored pause: bounded in `domain` from `anchor` for `duration`, or
/// unresolved (no domain).
async fn store_pause(
    pool: &PgPool,
    tenant: Uuid,
    domain: Option<ClockDomain>,
    anchor: i128,
    duration: i128,
    recorded_at: DateTime<Utc>,
) -> Result<Uuid> {
    let observation = Uuid::now_v7();
    let (boot, seconds, nanos) = match domain {
        Some(domain) => (Some(domain.boot()), Some(domain.offset().0), Some(domain.offset().1)),
        None => (None, None, None),
    };
    let nanos = nanos.map(i32::try_from).transpose()?;
    sqlx::query(
        "INSERT INTO venue_pauses(tenant_id,venue_id,status,code,pause_state,observation_id,
             original_domain,observed_tick,duration_ns,restoration_domain,restoration_anchor,
             deadline_tick,until_at,recorded_at)
         SELECT $1,'binance:spot',429,NULL,CASE WHEN d IS NULL THEN 'unresolved' ELSE 'bounded' END,
                $2,d,a,n,d,a,a+n,NULL,$8
         FROM (SELECT CASE WHEN $3::uuid IS NULL THEN NULL ELSE
                      ROW('boottime'::text,$3::uuid,$4::bigint,$5::integer)::budget_clock_domain_v38
                      END AS d,
                      CASE WHEN $3::uuid IS NULL THEN NULL ELSE $6::text::numeric END AS a,
                      CASE WHEN $3::uuid IS NULL THEN NULL ELSE $7::text::numeric END AS n) v",
    )
    .bind(tenant)
    .bind(observation)
    .bind(boot)
    .bind(seconds)
    .bind(nanos)
    .bind(anchor.to_string())
    .bind(duration.to_string())
    .bind(recorded_at)
    .execute(pool)
    .await?;
    Ok(observation)
}

/// The stored pause: (state, observation, original boot, observed tick,
/// duration, restoration boot and offsets, anchor, deadline, recorded_at µs).
type StoredPause = (
    String,
    Uuid,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<Uuid>,
    Option<i64>,
    Option<i32>,
    Option<String>,
    Option<String>,
    i64,
);

async fn stored_pause(pool: &PgPool, tenant: Uuid) -> Result<StoredPause> {
    Ok(sqlx::query_as(
        "SELECT pause_state, observation_id, (original_domain).boot_uuid, observed_tick::text,
                duration_ns::text, (restoration_domain).boot_uuid,
                (restoration_domain).offset_seconds, (restoration_domain).offset_nanoseconds,
                restoration_anchor::text, deadline_tick::text,
                (extract(epoch FROM recorded_at)*1000000)::bigint
         FROM venue_pauses WHERE tenant_id = $1 AND venue_id = 'binance:spot'",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await?)
}

/// This epoch's `budget_epochs` row: (predecessor, its cleanliness, boot,
/// start tick).
async fn epoch_row(
    pool: &PgPool,
    epoch: Uuid,
) -> Result<Option<(Option<Uuid>, Option<bool>, Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT predecessor_epoch, predecessor_clean, (clock_domain).boot_uuid, start_tick::text
         FROM budget_epochs WHERE process_epoch = $1 AND venue_id = 'binance:spot'",
    )
    .bind(epoch)
    .fetch_optional(pool)
    .await?)
}

/// The statements a proxy saw, without their `--` comment lines.
fn statements(proxy: &DbProxy) -> Vec<String> {
    proxy
        .query_trace()
        .iter()
        .map(|sql| {
            let lines = sql.lines().filter(|line| !line.trim_start().starts_with("--"));
            lines.collect::<Vec<_>>().join("\n").trim().to_string()
        })
        .collect()
}

/// The statements a proxy saw, each named by what it is.
fn shapes(proxy: &DbProxy) -> Vec<&'static str> {
    statements(proxy)
        .iter()
        .map(|sql| {
            if sql == "BEGIN ISOLATION LEVEL READ COMMITTED;" {
                "begin"
            } else if sql.starts_with("SELECT set_config('lock_timeout','300ms',true)") {
                "bounds"
            } else if sql.starts_with("SELECT p.tenant_id IS NOT NULL AS pause_present")
                && sql.ends_with(
                    "LEFT JOIN venue_pauses p ON p.tenant_id=$1::uuid AND p.venue_id=$2::text;",
                )
            {
                "pause"
            } else if sql.starts_with("INSERT INTO budget_epochs(") {
                "epoch"
            } else if sql.starts_with("INSERT INTO venue_pauses(") {
                "restore"
            } else if sql == "COMMIT;" {
                "commit"
            } else {
                "other"
            }
        })
        .collect()
}

/// PLAN §6.4: unknown history, so no weight before start + 60 s even after
/// a clean predecessor; TIME and the reads at + 61 s.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn no_weight_before_sixty_seconds_at_a_non_trading_start(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    let committed = clock.capture()?;
    let trace = Trace::default();
    let budget = bootstrap(&pool, tenant, &start, &clock, &trace)
        .await
        .map_err(|e| anyhow!("non-trading bootstrap: {e:?}"))?;
    ensure!(trace.issued() == 5, "five bootstrap statements, no restoration");
    let s = budget.start().map_err(|e| anyhow!("{e:?}"))?;
    ensure!(s.elapsed_since(&committed).is_ok(), "start captured after the epoch's COMMIT");
    for offset in [0, SECOND, 30 * SECOND, 60 * SECOND - 1] {
        ensure!(refused(&budget, at(s, offset)?)?, "weight at start + {offset} ns");
    }
    granted(&budget, at(s, 61 * SECOND)?)?;
    let row = epoch_row(&pool, start.epoch.epoch_id).await?;
    ensure!(
        row == Some((
            start.predecessor_epoch_id,
            Some(true),
            s.domain().boot(),
            s.nanoseconds().to_string()
        )),
        "budget_epochs row: {row:?}"
    );
    Ok(())
}

/// An unclean predecessor, or none at all, holds every request until start
/// + 120 s, past the 60 s window.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_unclean_start_waits_120s_before_any_request(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    // The first start (no predecessor), then a start after it, unmarked.
    let first = process_epoch::start_epoch(&pool, tenant, "ps-2a-test").await?;
    ensure!(first.predecessor_epoch_id.is_none());
    held_until_120s(&pool, tenant, &clock, &first, None).await?;
    let second = process_epoch::start_epoch(&pool, tenant, "ps-2a-test").await?;
    ensure!(second.predecessor_epoch_id == Some(first.epoch.epoch_id));
    held_until_120s(&pool, tenant, &clock, &second, Some(false)).await
}

async fn held_until_120s(
    pool: &PgPool,
    tenant: Uuid,
    clock: &NativeClock,
    start: &EpochStart,
    predecessor_clean: Option<bool>,
) -> Result<()> {
    ensure!(start.predecessor.unclean_reason().is_some());
    let budget = bootstrap(pool, tenant, start, clock, &Trace::default())
        .await
        .map_err(|e| anyhow!("non-trading bootstrap: {e:?}"))?;
    let s = budget.start().map_err(|e| anyhow!("{e:?}"))?;
    for offset in [SECOND, 61 * SECOND, 90 * SECOND, 120 * SECOND - 1] {
        ensure!(refused(&budget, at(s, offset)?)?, "request at start + {offset} ns");
    }
    granted(&budget, at(s, 120 * SECOND)?)?;
    let row = epoch_row(pool, start.epoch.epoch_id).await?;
    ensure!(
        row.as_ref().map(|r| (r.0, r.1)) == Some((start.predecessor_epoch_id, predecessor_clean)),
        "budget_epochs row: {row:?}"
    );
    Ok(())
}

/// A bounded pause stored in this clock domain blocks every request until
/// its stored end, with no restoration statement.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stored_pause_blocks_the_restart(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    let now = clock.capture()?;
    let end = ns(now)? + 200 * SECOND;
    store_pause(&pool, tenant, Some(now.domain()), ns(now)?, 200 * SECOND, Utc::now()).await?;
    let before = stored_pause(&pool, tenant).await?;
    let (proxy, wired) = wire(&pool).await?;
    let trace = Trace::default();
    let budget = bootstrap(&wired, tenant, &start, &clock, &trace)
        .await
        .map_err(|e| anyhow!("non-trading bootstrap: {e:?}"))?;
    ensure!(trace.issued() == 5);
    let shape = shapes(&proxy);
    ensure!(shape == ["begin", "bounds", "pause", "epoch", "commit"], "{shape:?}");
    ensure!(
        stored_pause(&pool, tenant).await? == before,
        "the same-domain pause is untouched"
    );
    let s = budget.start().map_err(|e| anyhow!("{e:?}"))?;
    ensure!(refused(&budget, at(s, 61 * SECOND)?)?, "the window has passed, the pause holds");
    ensure!(refused(&budget, tick(now.domain(), end - 1)?)?, "the pause holds to its end");
    granted(&budget, tick(now.domain(), end)?)?;
    Ok(())
}

/// A bounded pause from another clock domain (an earlier boot) is
/// re-anchored here to its full duration by one statement, whatever its old
/// end, and holds every request until then.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_new_clock_domain_reinstalls_the_full_pause(pool: PgPool) -> Result<()> {
    restored_in_a_new_domain(&pool, chrono::Duration::minutes(-5)).await
}

/// PLAN §6.4: a stored `recorded_at` ahead of the wall clock (a backward wall
/// step) never makes the trigger refuse the restoration: the row keeps the
/// stored value, the check accepts it, the start proceeds and no request is
/// sent before the restored end.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_backward_wall_step_never_blocks_the_restoration(pool: PgPool) -> Result<()> {
    restored_in_a_new_domain(&pool, chrono::Duration::hours(1)).await
}

/// A pause stored by an earlier boot with `recorded_at` at now + `skew`.
async fn restored_in_a_new_domain(pool: &PgPool, skew: chrono::Duration) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(pool, tenant).await?;
    let earlier = ClockDomain::new(Uuid::now_v7(), 0, 0)?;
    let duration = 90 * SECOND;
    let recorded_at = DateTime::from_timestamp_micros((Utc::now() + skew).timestamp_micros())
        .ok_or_else(|| anyhow!("recorded_at"))?;
    let observation =
        store_pause(pool, tenant, Some(earlier), 1_000 * SECOND, duration, recorded_at).await?;
    let stored_us = stored_pause(pool, tenant).await?.10;
    ensure!(stored_us == recorded_at.timestamp_micros());
    let (proxy, wired) = wire(pool).await?;
    let trace = Trace::default();
    let wall_before = Utc::now().timestamp_micros();
    let budget = bootstrap(&wired, tenant, &start, &clock, &trace)
        .await
        .map_err(|e| anyhow!("non-trading bootstrap and restoration: {e:?}"))?;
    let wall_after = Utc::now().timestamp_micros();
    let after = clock.capture()?;
    ensure!(trace.issued() == 6, "five bootstrap statements and one restoration");
    let shape = shapes(&proxy);
    ensure!(shape == ["begin", "bounds", "pause", "epoch", "commit", "restore"], "{shape:?}");
    let s = budget.start().map_err(|e| anyhow!("{e:?}"))?;
    let (seconds, nanos) = s.domain().offset();
    let row = stored_pause(pool, tenant).await?;
    let anchor: i128 = row.8.as_deref().unwrap_or_default().parse()?;
    ensure!(
        (row.0.as_str(), row.1, row.2, row.3.clone(), row.4.clone())
            == (
                "bounded",
                observation,
                Some(earlier.boot()),
                Some((1_000 * SECOND).to_string()),
                Some(duration.to_string())
            ),
        "the original observation is kept: {row:?}"
    );
    ensure!(
        (row.5, row.6, row.7)
            == (Some(s.domain().boot()), Some(seconds), Some(i32::try_from(nanos)?)),
        "re-anchored in this clock domain: {row:?}"
    );
    ensure!(ns(s)? <= anchor && anchor <= ns(after)?, "anchored at the restoration: {row:?}");
    ensure!(row.9 == Some((anchor + duration).to_string()), "the full duration: {row:?}");
    if stored_us > wall_after {
        ensure!(row.10 == stored_us, "the stored recorded_at is kept: {}", row.10);
    } else {
        ensure!((wall_before..=wall_after).contains(&row.10), "recorded now: {}", row.10);
    }
    ensure!(refused(&budget, at(s, 61 * SECOND)?)?, "the window has passed, the pause holds");
    ensure!(refused(&budget, tick(s.domain(), anchor + duration - 1)?)?);
    granted(&budget, tick(s.domain(), anchor + duration)?)?;
    Ok(())
}

/// A pause re-anchored by another starter between this start's read and its
/// restoration (here in a third clock domain, keeping a `recorded_at` ahead
/// of the wall clock) is never overwritten: the restoration updates nothing,
/// the start is refused, and the other restoration stays as it landed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_concurrent_restoration_refuses_the_start(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    let earlier = ClockDomain::new(Uuid::now_v7(), 0, 0)?;
    let ahead = (Utc::now() + chrono::Duration::hours(1)).timestamp_micros();
    let ahead = DateTime::from_timestamp_micros(ahead).ok_or_else(|| anyhow!("recorded_at"))?;
    store_pause(&pool, tenant, Some(earlier), 1_000 * SECOND, 90 * SECOND, ahead).await?;
    let trace = Trace::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut rows = bootstrap::observe_non_trading(&pool, tenant, &start, &clock, deadline, &trace)
        .await
        .map_err(|e| anyhow!("non-trading bootstrap: {e:?}"))?;
    sqlx::query(
        "UPDATE venue_pauses SET
             restoration_domain=ROW('boottime'::text,$2::uuid,0::bigint,0::integer)::budget_clock_domain_v38,
             restoration_anchor=5000000000000,deadline_tick=5000000000000+duration_ns,
             recorded_at=GREATEST(now(),recorded_at)
         WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(tenant)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await?;
    let raced = stored_pause(&pool, tenant).await?;
    ensure!(raced.10 == ahead.timestamp_micros(), "the other starter kept recorded_at");
    let refused =
        restore::existing_pause(&pool, tenant, rows.pause.as_mut(), &clock, deadline, &trace).await;
    ensure!(matches!(refused, Err(BudgetError::Unpublished)), "{:?}", refused.err());
    ensure!(trace.issued() == 6, "five bootstrap statements and the refused restoration");
    ensure!(stored_pause(&pool, tenant).await? == raced, "the concurrent restoration stays");
    Ok(())
}

/// PLAN §6.5, §7.1: a stored `unresolved` pause refuses the start inside the
/// bootstrap's transaction: three statements, nothing written.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_unresolved_stored_pause_refuses_the_start(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    store_pause(&pool, tenant, None, 0, 0, Utc::now()).await?;
    let before = stored_pause(&pool, tenant).await?;
    ensure!(before.0 == "unresolved");
    let (proxy, wired) = wire(&pool).await?;
    let trace = Trace::default();
    let refused = bootstrap(&wired, tenant, &start, &clock, &trace).await;
    ensure!(matches!(refused, Err(BudgetError::Deferred)), "{:?}", refused.err());
    ensure!(trace.issued() == 3);
    let shape = shapes(&proxy);
    ensure!(shape == ["begin", "bounds", "pause"], "{shape:?}");
    ensure!(epoch_row(&pool, start.epoch.epoch_id).await?.is_none(), "no budget_epochs row");
    ensure!(stored_pause(&pool, tenant).await? == before);
    Ok(())
}

/// PLAN §6.4, §7.2: five statements, none of which reads or writes C2/2b's
/// state; `budget_venue_state`, the daily charges and the source audits are
/// left exactly as they were (empty), for 8o's audit.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_non_trading_bootstrap_never_touches_c2_state(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    let (proxy, wired) = wire(&pool).await?;
    let trace = Trace::default();
    bootstrap(&wired, tenant, &start, &clock, &trace)
        .await
        .map_err(|e| anyhow!("non-trading bootstrap: {e:?}"))?;
    ensure!(trace.issued() == 5 && proxy.query_trace_dropped() == 0);
    let statements = statements(&proxy);
    let shape = shapes(&proxy);
    ensure!(shape == ["begin", "bounds", "pause", "epoch", "commit"], "{shape:?}");
    for table in [
        "budget_venue_state",
        "budget_daily_charges",
        "budget_source_audits",
        "budget_recovery_cohorts",
        "submit_attempts",
    ] {
        ensure!(
            statements.iter().all(|sql| !sql.contains(table)),
            "{table} named: {statements:?}"
        );
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await?;
        ensure!(count == 0, "{table} written");
    }
    let epochs: i64 = sqlx::query_scalar("SELECT count(*) FROM budget_epochs")
        .fetch_one(&pool)
        .await?;
    ensure!(epochs == 1 && epoch_row(&pool, start.epoch.epoch_id).await?.is_some());
    Ok(())
}

/// Each statement runs under the server bounds and the whole under its
/// deadline: a held lock ends the `budget_epochs` insert at the 300 ms lock
/// bound, long before the 5 s one; an earlier deadline drops the connection,
/// whose server session ends with nothing committed; a later attempt lands.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_non_trading_bootstrap_is_bounded_and_dropped_on_expiry(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let clock = NativeClock::initialize()?;
    let start = clean_start(&pool, tenant).await?;
    let mut holder = pool.begin().await?;
    sqlx::query("LOCK TABLE budget_epochs IN SHARE MODE")
        .execute(&mut *holder)
        .await?;

    let (proxy, wired) = wire(&pool).await?;
    let began = std::time::Instant::now();
    let locked = bootstrap(&wired, tenant, &start, &clock, &Trace::default()).await;
    ensure!(matches!(locked, Err(BudgetError::Invalid)), "{:?}", locked.err());
    ensure!(began.elapsed() < Duration::from_secs(3), "ended by the server's lock bound");
    // The insert waited on the lock (from its parse) and never completed: no
    // COMMIT followed.
    let shape = shapes(&proxy);
    ensure!(shape[..3] == ["begin", "bounds", "pause"] && shape.len() <= 4, "{shape:?}");
    ensure!(!shape.contains(&"commit"), "{shape:?}");

    let began = std::time::Instant::now();
    let deadline = Instant::now() + Duration::from_millis(150);
    let expired =
        bootstrap::non_trading(&pool, tenant, &start, &clock, deadline, &Trace::default()).await;
    ensure!(matches!(expired, Err(BudgetError::Expired)), "{:?}", expired.err());
    ensure!(began.elapsed() < Duration::from_secs(1));
    let waiting = "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
                   AND state <> 'idle' AND query LIKE '-- B05:%'";
    let gone = Instant::now() + Duration::from_secs(3);
    while sqlx::query_scalar::<_, i64>(waiting).fetch_one(&pool).await? != 0 {
        ensure!(Instant::now() < gone, "the dropped bootstrap's session still waits");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    holder.rollback().await?;
    ensure!(epoch_row(&pool, start.epoch.epoch_id).await?.is_none(), "nothing committed");

    let trace = Trace::default();
    bootstrap(&pool, tenant, &start, &clock, &trace)
        .await
        .map_err(|e| anyhow!("a later attempt: {e:?}"))?;
    ensure!(trace.issued() == 5 && epoch_row(&pool, start.epoch.epoch_id).await?.is_some());
    Ok(())
}
