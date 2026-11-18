use anyhow::{anyhow, ensure, Context};
use sqlx::{postgres::PgRow, PgPool, Row};

use super::*;
use crate::{
    process_epoch,
    venue_budget::{self, bootstrap, restore, Trace},
};

#[path = "pg_tests/http.rs"]
mod http;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn actual_bootstrap_time_sample_and_closing(pool: PgPool) -> anyhow::Result<()> {
    // Fixture ceiling only: the real native 120s barrier, HTTP 10s, B2 <=2s,
    // cadence 60s and production prerequisites below remain unchanged.
    let body_deadline = Instant::now() + Duration::from_secs(150);
    let tenant = Uuid::now_v7();
    let mut epoch = None;
    let mut owner = None;
    let mut http = None;
    let primary = timeout_at(body_deadline, async {
        ensure!(matches!(bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
        let clock = Arc::new(NativeClock::initialize()?);
        epoch = Some(process_epoch::start_epoch(&pool, tenant, "I1-controlled-test").await?);
        let start = epoch.as_ref().context("committed test epoch")?;
        let setup = Trace::default();
        // Seven actual SQL commands in an empty sqlx macro DB; this sealed
        // descriptive fixture is not the missing production C2 producer.
        let input = venue_budget::fixture::prepare(
            &pool, &clock, Instant::now() + Duration::from_secs(2), &setup,
        ).await.map_err(|e| anyhow!("fixture: {e:?}"))?;
        ensure!(setup.issued() == 7, "fixture SQL count");
        let trace = Trace::default();
        let mut rows = bootstrap::observe_bootstrap(
            &pool, tenant, start, &input, Instant::now() + Duration::from_secs(2), &trace,
        ).await.map_err(|e| anyhow!("bootstrap: {e:?}"))?;
        ensure!(trace.issued() == 6, "bootstrap SQL count");
        restore::existing_pause(
            &pool, tenant, &mut rows, &clock, Instant::now() + Duration::from_secs(2), &trace,
        ).await.map_err(|e| anyhow!("restore: {e:?}"))?;
        ensure!(trace.issued() == 6, "absent pause issues no SQL");
        let budget = SharedBudget::from_rows(rows, start.predecessor.unclean_reason().is_some())
            .map_err(|e| anyhow!("budget: {e:?}"))?;
        // The listener also covers the unchanged shared stop deadline if
        // the primary reaches its fixture ceiling. No extra request allowance.
        http = Some(http::Fixture::start(
            body_deadline + crate::stop::StopTimings::default().deadline,
        ).await?);
        let fixture = http.as_ref().context("owned HTTP fixture")?;
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            fixture.base_url.clone(), String::new(), String::new(),
        ));
        let store = ClockSampleStore::from_committed_epoch(&pool, tenant, start);
        owner = Some(ClockOwner::controlled(
            Arc::clone(&clock), client, store, budget.clone(), tenant, start.epoch.epoch_id,
        ).map_err(|e| anyhow!("owner: {e:?}"))?);
        let owner = owner.as_mut().context("owned TIME component")?;
        let initial: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM clock_samples WHERE process_epoch=$1",
        ).bind(start.epoch.epoch_id).fetch_one(&pool).await?;
        ensure!(initial == 0, "no initial sample row");
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))),
            "no TIME before the real unclean barrier");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 0);
        ensure!(fixture.connections() == 0, "no physical connection before native barrier");
        // Real elapsed time, with a real native comparison after waking.
        tokio::time::sleep(Duration::from_secs(120)).await;
        let now = clock.capture()?;
        ensure!(now.elapsed_since(&budget.start().map_err(|e| anyhow!("start: {e:?}"))?)?
            >= Duration::from_secs(120));
        let waiting = budget.protective_waiter().map_err(|e| anyhow!("waiter: {e:?}"))?;
        ensure!(matches!(budget.try_reserve_time(now, after(now, Duration::from_secs(10))
            .map_err(|e| anyhow!("deadline: {e:?}"))?), Err(BudgetError::Deferred)));
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 0);
        ensure!(fixture.connections() == 0, "protective waiter still permits no physical TIME");
        drop(waiting);
        let before = clock.capture()?;
        let sample = owner.run_cycle().await.map_err(|e| anyhow!("TIME cycle: {e:?}"))?;
        let after = clock.capture()?;
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(fixture.connections() == 1, "actual physical 0 -> 1 connection");
        ensure!(owner.preparation.is_none() && owner.parser.is_none(), "completed jobs consumed");
        // Independent readback is fixture work, outside the TIME operation.
        let rows = sqlx::query("SELECT sample_id,tenant_id,venue_id,process_epoch,server_ms,send_wall_seconds,send_wall_nanoseconds,send_tick_before::text AS sb,send_tick_after::text AS sa,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before::text AS rb,receive_tick_after::text AS ra,(clock_domain).boot_uuid AS boot,(clock_domain).offset_seconds AS os,(clock_domain).offset_nanoseconds AS ons,theta2_ns::text AS theta,r_low_ns::text AS low,r_high_ns::text AS high,u2_ns::text AS bound,numeric_failures FROM clock_samples WHERE process_epoch=$1")
            .bind(start.epoch.epoch_id).fetch_all(&pool).await?;
        ensure!(rows.len() == 1, "exactly one persisted original row");
        verify_row(&rows[0], sample, tenant, start.epoch.epoch_id, before, after,
            fixture.server_ms()?)?;
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))),
            "the existing 60s cadence prevents immediate replay");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(owner.state.lock().map_err(|_| anyhow!("state lock poisoned"))?
            .published.as_ref().is_some_and(|p| p.id == sample), "actual publication before Closing");
        ensure!(!owner.state.lock().map_err(|_| anyhow!("state lock poisoned"))?.closing,
            "positive path remains open until the real stop");
        Ok::<_, anyhow::Error>(())
    }).await.map_err(|_| anyhow!("150s fixture body ceiling elapsed")).and_then(|r| r);

    // Returned errors and the body ceiling all reach teardown. Only a failed
    // primary cancels before spawn: the real worker retires its owned jobs.
    // The happy path stays open until run_stop_with_clock invalidates it.
    let asked_at = Instant::now();
    let deadline = asked_at + crate::stop::StopTimings::default().deadline;
    let closing = owner.take().map(|owner| {
        // No await or spawned owner can advance this original baseline.
        let expected_handoffs = owner.budget.handoffs();
        if primary.is_err() {
            owner.cancel.cancel();
        }
        let state = Arc::clone(&owner.state);
        let budget = owner.budget.clone();
        let closing = owner.spawn();
        (closing, state, budget, expected_handoffs)
    });
    let mut failures = Vec::new();
    if let Some((closing, state, budget, expected_handoffs)) = closing {
        let result =
            close_owner(&closing, &state, &budget, expected_handoffs, epoch.as_ref(), asked_at)
                .await;
        if let Err(e) = result {
            failures.push(format!("owner retirement: {e:#}"));
        }
    }
    // Keep accepting/detecting physical requests through all owner retirement.
    if let Some(http) = http.take() {
        if let Err(e) = http.finish(deadline).await {
            failures.push(format!("HTTP retirement: {e:#}"));
        }
    }
    // Check the real marker even when the primary sample path failed.
    if let Some(start) = epoch.as_ref() {
        let unmarked = timeout_at(deadline, sqlx::query_scalar::<_, bool>(
            "SELECT clean_stop_at IS NULL AND marker_version IS NULL FROM process_epochs WHERE epoch_id=$1",
        ).bind(start.epoch.epoch_id).fetch_one(&pool)).await;
        if !matches!(unmarked, Ok(Ok(true))) {
            failures.push(format!("real epoch must remain unmarked: {unmarked:?}"));
        }
    }
    if timeout_at(deadline, pool.close()).await.is_err() {
        failures.push("pool close exceeded the shared original stop deadline".into());
    }
    let cleanup = if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(failures.join("; ")))
    };
    eprintln!(
        "I1_FINAL primary={:?} cleanup={:?}",
        primary.as_ref().err(),
        cleanup.as_ref().err()
    );
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(p), Ok(())) => Err(p),
        (Ok(()), Err(c)) => Err(c),
        (Err(p), Err(c)) => Err(anyhow!("primary: {p:#}; cleanup: {c:#}")),
    }
}

async fn close_owner(
    closing: &ClockClosing,
    state: &Mutex<ClockState>,
    budget: &SharedBudget,
    expected_handoffs: Result<u64, BudgetError>,
    epoch: Option<&process_epoch::EpochStart>,
    asked_at: Instant,
) -> anyhow::Result<()> {
    let Some(start) = epoch else {
        closing.begin_closing();
        let joined = closing.join(asked_at + crate::stop::StopTimings::default().deadline).await;
        return Err(anyhow!("owner without committed epoch; actual join={joined}"));
    };
    let backend = Arc::new(robson_exec::order_journal::StubJournalBackend::default());
    let journal =
        robson_exec::order_journal::OrderJournal::new(backend, &VenueId::new("test:venue"));
    journal.gate().begin_epoch(
        robson_exec::gate::EpochView {
            epoch_seq: start.epoch.epoch_seq,
            unclean_reason: start.predecessor.unclean_reason(),
        },
        start.unclean_generation,
        start.open_incidents.clone(),
    );
    let marker = UncalledMarker(std::sync::atomic::AtomicUsize::new(0));
    let invalidated = std::sync::atomic::AtomicBool::new(false);
    let report = crate::stop::run_stop_with_clock(
        &journal,
        asked_at,
        || {
            invalidated.store(
                state.lock().is_ok_and(|s| s.closing && s.published.is_none()),
                std::sync::atomic::Ordering::SeqCst,
            );
        },
        Some(&marker),
        Some(closing),
        crate::stop::StopTimings::default(),
    )
    .await;
    // Preserve the real stop callback oracle before any cleanup repair.
    let stop_invalidated = invalidated.load(std::sync::atomic::Ordering::SeqCst);
    closing.begin_closing();
    let final_joined = closing.join(asked_at + crate::stop::StopTimings::default().deadline).await;
    // Even a wrong report/callback must not skip the final owned join attempt.
    ensure!(
        report.result == crate::stop::StopResult::IncidentOpen && final_joined,
        "actual stop: {report:?}; final owner join={final_joined}"
    );
    ensure!(report.marker == crate::stop::MarkerWrite::NotAttempted);
    ensure!(marker.0.load(std::sync::atomic::Ordering::SeqCst) == 0);
    ensure!(stop_invalidated, "the original stop callback observed no invalidation");
    ensure!(
        budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))?
            == expected_handoffs.map_err(|e| anyhow!("initial handoffs: {e:?}"))?,
        "Closing never starts another TIME"
    );
    ensure!(
        closing.worker.try_lock().map_err(|e| anyhow!("worker slot: {e}"))?.is_none(),
        "owner, preparation and parser must actually retire"
    );
    ensure!(
        closing
            .watchdog
            .try_lock()
            .map_err(|e| anyhow!("watchdog slot: {e}"))?
            .is_none(),
        "independent watchdog must actually retire"
    );
    Ok(())
}

fn verify_row(
    row: &PgRow,
    sample: Uuid,
    tenant: Uuid,
    epoch: Uuid,
    before: TickCapture,
    after: TickCapture,
    server_ms: i64,
) -> anyhow::Result<()> {
    ensure!(row.try_get::<Uuid, _>("sample_id")? == sample);
    ensure!(row.try_get::<Uuid, _>("tenant_id")? == tenant);
    ensure!(row.try_get::<Uuid, _>("process_epoch")? == epoch);
    ensure!(row.try_get::<String, _>("venue_id")? == VenueId::BINANCE_SPOT);
    ensure!(
        row.try_get::<i64, _>("server_ms")? == server_ms,
        "retain original actual HTTP reply"
    );
    let domain = before.domain();
    ensure!(after.domain() == domain);
    ensure!(row.try_get::<Uuid, _>("boot")? == domain.boot());
    ensure!(row.try_get::<i64, _>("os")? == domain.offset().0);
    ensure!(u32::try_from(row.try_get::<i32, _>("ons")?)? == domain.offset().1);
    let mark = |wall, nanos, lo, hi| -> anyhow::Result<RawClockCapture> {
        Ok(RawClockCapture {
            epoch,
            domain,
            wall_seconds: row.try_get(wall)?,
            wall_nanoseconds: u32::try_from(row.try_get::<i32, _>(nanos)?)?,
            before_ns: row.try_get::<String, _>(lo)?.parse()?,
            after_ns: row.try_get::<String, _>(hi)?.parse()?,
        })
    };
    let raw = RawClockSample {
        id: sample,
        tenant,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send: mark("send_wall_seconds", "send_wall_nanoseconds", "sb", "sa")?,
        receive: mark("receive_wall_seconds", "receive_wall_nanoseconds", "rb", "ra")?,
        server_ms,
    };
    ensure!(i128::try_from(before.nanoseconds())? <= raw.send.before_ns);
    ensure!(raw.send.before_ns <= raw.send.after_ns && raw.send.after_ns <= raw.receive.before_ns);
    ensure!(raw.receive.before_ns <= raw.receive.after_ns);
    ensure!(raw.receive.after_ns <= i128::try_from(after.nanoseconds())?);
    let numbers = raw.numbers()?;
    for (field, expected) in [
        ("theta", numbers.theta2),
        ("low", numbers.r_low),
        ("high", numbers.r_high),
        ("bound", numbers.u2),
    ] {
        ensure!(
            row.try_get::<String, _>(field)? == expected.to_string(),
            "original arithmetic {field}"
        );
    }
    ensure!(row.try_get::<i16, _>("numeric_failures")? == numbers.failures);
    Ok(())
}

struct UncalledMarker(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl crate::stop::StopMarker for UncalledMarker {
    async fn mark_clean(&self) -> Result<(), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("actual startup incident remains open".into())
    }
}
