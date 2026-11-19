use anyhow::{anyhow, ensure, Context};
use sqlx::{postgres::PgRow, PgPool, Row};

use super::*;
use crate::{
    process_epoch,
    venue_budget::{self, bootstrap, restore, Trace},
};

#[path = "pg_tests/http.rs"]
mod http;

#[cfg(all(test, feature = "crash-points"))]
#[path = "c8_account_http.rs"]
mod c8_account_http;

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
    let mut account_http = None;
    let mut account_slot: Option<inventory::AccountPreparation> = None;
    let mut supersession_http = [None, None];
    let mut supersession_slots = [None, None];
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
        ensure!(matches!(owner.commission_signing_time(), Err(TimeError::Native)),
            "ACCOUNT has no signing authority before a positively published sample");
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

        // C8 ACCOUNT-only, no capital: the same owner, SharedBudget and
        // positively read-back publication. Listener setup is fixture work.
        account_http = Some(c8_account_http::AccountFixture::start(
            body_deadline + crate::stop::StopTimings::default().deadline,
        ).await?);
        let venue = account_http.as_ref().context("owned ACCOUNT fixture")?;
        let read_deadline = Instant::now() + Duration::from_secs(10);
        let claim = clock.capture()?;
        let mut pending = owner.reserve_account(read_deadline)
            .map_err(|e| anyhow!("ACCOUNT reserve: {e:?}"))?;
        let prepared = owner.prepare_account(&mut account_slot, read_deadline).await
            .map_err(|e| anyhow!("ACCOUNT preparation: {e:?}"))?;
        ensure!(account_slot.is_none(), "completed preparation consumed");
        let signed = owner.commission_signing_time().map_err(|e| anyhow!("signing: {e:?}"))?;
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 1);
        ensure!(venue.connections() == 0, "nothing sent before the admitted first poll");
        let admitted = owner.read_account(&signed, &mut pending, claim, prepared, venue.port, read_deadline)
            .await.map_err(|e| anyhow!("ACCOUNT read: {e:?}"))?;
        ensure!(admitted.sample == sample, "EOF receipt judged on the same published sample");
        ensure!(admitted.send.epoch == start.epoch.epoch_id
            && admitted.receive.epoch == start.epoch.epoch_id
            && admitted.send.wall_agrees(&admitted.receive)?
            && admitted.receive.end_tick()?.elapsed_since(&admitted.send.end_tick()?).is_ok(),
            "original first poll and EOF brackets share the committed epoch and ordered TIME");
        ensure!(venue.signing_stamp()? == signed.timestamp_ms(), "actual wire stamp is the original signed one");
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 2,
            "TIME 1 + ACCOUNT 1 handoffs on the one SharedBudget");
        ensure!(venue.connections() == 1 && fixture.connections() == 1,
            "one physical ACCOUNT connection, no further TIME");
        drop(pending);
        ensure!(budget.handoffs().map_err(|e| anyhow!("handoffs: {e:?}"))? == 2,
            "a consumed reservation refunds nothing and sends nothing");
        let probe_tick = clock.capture()?;
        let probe_due = crate::venue_clock_owner::after(probe_tick, Duration::from_secs(1))
            .map_err(|e| anyhow!("{e:?}"))?;
        let mut probes = Vec::new();
        for _ in 0..18 {
            probes.push(budget.try_reserve_account(probe_tick, probe_due)
                .map_err(|e| anyhow!("steady ACCOUNT budget probe: {e:?}"))?);
        }
        let mut time_probes = Vec::new();
        for _ in 0..19 {
            time_probes.push(budget.try_reserve_time(probe_tick, probe_due)
                .map_err(|e| anyhow!("steady TIME budget probe: {e:?}"))?);
        }
        ensure!(matches!(budget.try_reserve_time(probe_tick, probe_due), Err(BudgetError::Deferred)),
            "TIME and ACCOUNT charged exactly 21 of the common 400 steady weight");
        drop(time_probes);
        drop(probes);
        commission_admission_precedence(owner, &clock)?;
        for (i, replace_generation) in [false, true].into_iter().enumerate() {
            supersession_during_account_body(
                owner, &clock, replace_generation, body_deadline,
                &mut supersession_http[i], &mut supersession_slots[i],
            ).await?;
        }
        ensure!(venue.connections() == 1 && budget.handoffs()
            .map_err(|e| anyhow!("handoffs: {e:?}"))? == 4,
            "one TIME, one accepted ACCOUNT and two charged rejected ACCOUNT reads");
        eprintln!("C8_ACCOUNT_GUARDED time=1 account=3 accepted=1 superseded=2 weight_first_two=21 no_venue_orders=true");
        Ok::<_, anyhow::Error>(())
    }).await.map_err(|_| anyhow!("150s fixture body ceiling elapsed")).and_then(|r| r);

    // Returned errors and the body ceiling all reach teardown. Only a failed
    // primary cancels before spawn: the real worker retires its owned jobs.
    // The happy path stays open until run_stop_with_clock invalidates it.
    let asked_at = Instant::now();
    let deadline = asked_at + crate::stop::StopTimings::default().deadline;
    // The test-owned ACCOUNT preparation retires on every path, before the
    // owner's worker permit is needed by its own retirement.
    let account_retired =
        timeout_at(deadline, ClockOwner::retire_account(&mut account_slot)).await;
    let mut supersession_errors = Vec::new();
    for (fixture, slot) in supersession_http.into_iter().zip(&mut supersession_slots) {
        if let Some(fixture) = fixture {
            if let Err(error) = fixture.finish(deadline).await {
                supersession_errors.push(format!("supersession listener retirement: {error}"));
            }
        }
        let retired = timeout_at(deadline, ClockOwner::retire_account(slot)).await;
        if !matches!(retired, Ok(true)) {
            supersession_errors.push(format!("supersession preparation retirement: {retired:?}"));
        }
    }
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
    let mut failures = supersession_errors;
    if !matches!(account_retired, Ok(true)) {
        failures.push(format!("ACCOUNT preparation retirement: {account_retired:?}"));
    }
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
    if let Some(account_http) = account_http.take() {
        if let Err(e) = account_http.finish(deadline).await {
            failures.push(format!("ACCOUNT HTTP retirement: {e:#}"));
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

fn commission_admission_precedence(
    owner: &ClockOwner,
    clock: &NativeClock,
) -> anyhow::Result<()> {
    let due = Instant::now() + Duration::from_secs(3);
    let old = owner.budget.start().map_err(|e| anyhow!("{e:?}"))?;
    let fresh = clock.capture()?;
    let signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
    let mut pending = owner.reserve_commission(due).map_err(|e| anyhow!("{e:?}"))?;
    let mut foreign = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
    let generation = owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation;
    foreign.state = Arc::new(Mutex::new(ClockState {
        generation,
        closing: false,
        published: None,
        start: old,
        last_valid: None,
        faults: 0,
        active_faults: 0,
    }));
    // The real budget gate is closed to steady work by a waiting protection.
    // The generated budget tests independently cover the recorded 429 latch.
    let waiting = owner.budget.protective_waiter().map_err(|e| anyhow!("{e:?}"))?;
    for (proof, claim, deadline, expected) in [
        (&foreign, old, Instant::now(), BudgetError::Invalid),
        (&signed, old, due, BudgetError::Expired),
        (&signed, fresh, due, BudgetError::Deferred),
    ] {
        let result = owner.poll_commission_first(proof, &mut pending, claim, deadline, || {
            panic!("refused Commission must not poll")
        });
        ensure!(matches!(result, Err(error) if error == expected));
    }
    owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation + 1;
    let results = [Instant::now(), due].map(|deadline| {
        owner.poll_commission_first(&signed, &mut pending, old, deadline, || {
            panic!("stale Commission must not poll")
        })
    });
    owner.state.lock().map_err(|_| anyhow!("clock lock"))?.generation = generation;
    ensure!(matches!(results, [Err(BudgetError::Expired), Err(BudgetError::Deferred)]),
        "command deadline precedes generation, which precedes claim age");
    drop(waiting);
    drop(pending);
    ensure!(owner.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 2);
    Ok(())
}

async fn supersession_during_account_body(
    owner: &ClockOwner,
    clock: &NativeClock,
    replace_generation: bool,
    fixture_deadline: Instant,
    fixture_slot: &mut Option<c8_account_http::AccountFixture>,
    slot: &mut Option<inventory::AccountPreparation>,
) -> anyhow::Result<()> {
    let due = (Instant::now() + Duration::from_secs(5)).min(fixture_deadline);
    ensure!(fixture_slot.is_none() && slot.is_none());
    let (venue, head, release) =
        c8_account_http::AccountFixture::held_body(fixture_deadline).await?;
    let venue = fixture_slot.insert(venue);
    let work = async {
        let claim = clock.capture()?;
        let mut pending = owner.reserve_account(due).map_err(|e| anyhow!("{e:?}"))?;
        let prepared = owner.prepare_account(slot, due).await.map_err(|e| anyhow!("{e:?}"))?;
        let signed = owner.commission_signing_time().map_err(|e| anyhow!("{e:?}"))?;
        let original = {
            let state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
            (state.generation, state.published.as_ref().context("positive sample")?.id)
        };
        let intervene = async {
            head.await.context("ACCOUNT response head")?;
            {
                let mut state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
                if replace_generation {
                    state.generation = original.0 + 1;
                } else {
                    state.published.as_mut().context("positive sample")?.id = Uuid::now_v7();
                }
            }
            release.send(()).map_err(|_| anyhow!("ACCOUNT body release"))?;
            Ok::<_, anyhow::Error>(())
        };
        let read = owner.read_account(&signed, &mut pending, claim, prepared, venue.port, due);
        let (result, intervention) = tokio::join!(read, intervene);
        {
            let mut state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
            state.generation = original.0;
            state.published.as_mut().context("positive sample retained")?.id = original.1;
        }
        intervention?;
        ensure!(matches!(result, Err(TimeError::Native)), "original receipt superseded at EOF");
        ensure!(venue.connections() == 1, "one real charged ACCOUNT without retry");
        Ok::<_, anyhow::Error>(())
    };
    timeout_at(due, work).await.context("ACCOUNT supersession deadline")?
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
