//! Native controls for the private clocked Spot journal route (J2b) against
//! the real migration-38 helper. TEST-ONLY; no production writer authority.
use std::time::Duration;

use anyhow::{anyhow, ensure};
use chrono::DateTime;
use robson_exec::{
    clock_records::RawClockCapture,
    order_journal::{AttemptRow, BalanceBatch, CommitFailure, IntentKind, IntentRow, JournalBatch},
    venue_clock::{NativeClock, TickCapture},
};
use sqlx::PgPool;
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    clocked::{ClockReceipt, ClockedAttempt, ClockedInput},
    PgJournalBackend,
};
use crate::venue_budget::{fixture, Trace};

struct Scope {
    tenant: Uuid,
    epoch: Uuid,
    clock: NativeClock,
}

async fn scope(pool: &PgPool) -> anyhow::Result<Scope> {
    let clock = NativeClock::initialize()?;
    let trace = Trace::default();
    fixture::prepare(pool, &clock, Instant::now() + Duration::from_secs(2), &trace)
        .await
        .map_err(|error| anyhow!("budget fixture: {error:?}"))?;
    let tenant = Uuid::now_v7();
    let start = crate::process_epoch::start_epoch(pool, tenant, "j2b-clocked-journal").await?;
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
         VALUES($1,'binance:spot','net_from_start_non_expanding_v1')",
    )
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(Scope {
        tenant,
        epoch: start.epoch.epoch_id,
        clock,
    })
}

/// One actual native same-wall capture with a submicrosecond remainder, then
/// the independent budget tick. Selection only; nothing is retimed.
fn capture(scope: &Scope) -> anyhow::Result<(RawClockCapture, TickCapture)> {
    for _ in 0..8 {
        let raw = scope.clock.wall_bracket(scope.epoch)?;
        if raw.wall_nanoseconds % 1_000 != 0 {
            return Ok((raw, scope.clock.capture()?));
        }
    }
    Err(anyhow!("no submicrosecond native wall capture in eight reads"))
}

fn batch(
    intent_id: Uuid,
    client: &str,
    attempts: Vec<AttemptRow>,
    created_at: DateTime<chrono::Utc>,
) -> JournalBatch {
    JournalBatch {
        intent: IntentRow {
            intent_id,
            venue_id: "binance:spot".to_owned(),
            account_scope: "j2b-test".to_owned(),
            client_order_id: client.to_owned(),
            position_id: None,
            kind: IntentKind::Entry,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at,
        },
        attempts,
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    }
}

fn clocked(
    scope: &Scope,
    batch: &JournalBatch,
    raw: RawClockCapture,
    budget: TickCapture,
) -> ClockedInput {
    let intent = &batch.intent;
    ClockedInput {
        process_epoch: scope.epoch,
        attempts: batch
            .attempts
            .iter()
            .map(|attempt| ClockedAttempt {
                attempt_id: attempt.attempt_id,
                intent_id: intent.intent_id,
                tenant_id: scope.tenant,
                venue_id: intent.venue_id.clone(),
                account_scope: intent.account_scope.clone(),
                client_order_id: intent.client_order_id.clone(),
                kind: intent.kind,
                capture: raw,
                sample: None,
                budget,
            })
            .collect(),
    }
}

/// A batch of one attempt created at its own captured wall instant.
fn one(scope: &Scope, client: &str) -> anyhow::Result<(JournalBatch, ClockedInput)> {
    let (raw, budget) = capture(scope)?;
    let wall = DateTime::from_timestamp(raw.wall_seconds, raw.wall_nanoseconds)
        .ok_or_else(|| anyhow!("wall"))?;
    let attempt = AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: wall,
        rejected_at: None,
    };
    let batch = batch(Uuid::now_v7(), client, vec![attempt], wall);
    let input = clocked(scope, &batch, raw, budget);
    Ok((batch, input))
}

async fn intents(pool: &PgPool, intent: Uuid) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*)::bigint FROM order_intents WHERE intent_id=$1")
            .bind(intent)
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn one_native_attempt_round_trips_and_replays_without_a_new_charge(
    pool: PgPool,
) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let (batch, input) = one(&scope, "j2b-happy")?;
    let attempt = batch.attempts[0].attempt_id;
    let raw = input.attempts[0].capture;

    let receipts = backend.commit_clocked(&batch, &input).await;
    let expected = vec![ClockReceipt {
        attempt_id: attempt,
        daily_units: 1,
        retired: false,
    }];
    ensure!(receipts.as_ref() == Ok(&expected), "one exact receipt: {receipts:?}");

    // PostgreSQL keeps whole microseconds; the capture keeps its nanoseconds.
    let (stored, nanos, before, after, tick, units, budget_ns): (
        DateTime<chrono::Utc>,
        i32,
        String,
        String,
        String,
        i16,
        i64,
    ) = sqlx::query_as(
            "SELECT c.attempt_created_at,c.capture_wall_nanoseconds,c.capture_tick_before::text,c.capture_tick_after::text,
                c.budget_created_tick::text,d.daily_units,
                ((c.budget_clock_domain).offset_nanoseconds)::bigint
             FROM spot_intent_clock c JOIN budget_daily_charges d USING(attempt_id)
             WHERE c.attempt_id=$1",
        )
        .bind(attempt)
        .fetch_one(&pool)
        .await?;
    ensure!(
        Some(stored)
            == DateTime::from_timestamp(raw.wall_seconds, raw.wall_nanoseconds / 1_000 * 1_000),
        "timestamptz is the truncated microsecond projection"
    );
    ensure!(
        nanos as u32 == raw.wall_nanoseconds && nanos % 1_000 != 0,
        "full raw nanoseconds"
    );
    ensure!(
        before == raw.before_ns.to_string() && after == raw.after_ns.to_string(),
        "exact ticks"
    );
    ensure!(
        tick == input.attempts[0].budget.nanoseconds().to_string(),
        "independent budget tick, not a capture coordinate"
    );
    ensure!(
        units == 1 && budget_ns == i64::from(input.attempts[0].budget.domain().offset().1),
        "charge"
    );

    // All-conflict same-value replay: same receipt, still one charge.
    let replay = backend.commit_clocked(&batch, &input).await;
    ensure!(replay.as_ref() == Ok(&expected), "exact replay receipt: {replay:?}");
    let charges: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM budget_daily_charges WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_one(&pool)
            .await?;
    ensure!(charges == 1, "replay adds no charge");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn zero_attempts_issue_no_helper_and_commit(pool: PgPool) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let batch = batch(Uuid::now_v7(), "j2b-zero", Vec::new(), chrono::Utc::now());
    let input = ClockedInput {
        process_epoch: scope.epoch,
        attempts: Vec::new(),
    };
    let receipts = backend.commit_clocked(&batch, &input).await;
    ensure!(receipts == Ok(Vec::new()), "no receipt: {receipts:?}");
    ensure!(intents(&pool, batch.intent.intent_id).await? == 1, "intent committed");
    let clocks: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM spot_intent_clock")
        .fetch_one(&pool)
        .await?;
    ensure!(clocks == 0, "no helper call");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn mismatched_evidence_refuses_before_sql(pool: PgPool) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let refused = |result: &Result<Vec<ClockReceipt>, CommitFailure>| matches!(result, Err(CommitFailure::Known(why)) if why.starts_with("clocked journal refused"));

    // Two attempts for one client order id.
    let (mut two, mut input) = one(&scope, "j2b-two")?;
    let mut second = two.attempts[0].clone();
    second.attempt_id = Uuid::now_v7();
    two.attempts.push(second.clone());
    let mut extra = input.attempts[0].clone();
    extra.attempt_id = second.attempt_id;
    input.attempts.push(extra);
    let result = backend.commit_clocked(&two, &input).await;
    ensure!(refused(&result), "two attempts refuse: {result:?}");
    ensure!(intents(&pool, two.intent.intent_id).await? == 0, "nothing written");

    // An unrelated wall instant in the same microsecond.
    let (mut shifted, input) = one(&scope, "j2b-wall")?;
    let wall = shifted.attempts[0].created_at;
    shifted.attempts[0].created_at = if wall.timestamp_subsec_nanos() % 1_000 == 999 {
        wall - chrono::TimeDelta::nanoseconds(1)
    } else {
        wall + chrono::TimeDelta::nanoseconds(1)
    };
    let result = backend.commit_clocked(&shifted, &input).await;
    ensure!(refused(&result), "same-microsecond wall mismatch refuses: {result:?}");
    ensure!(intents(&pool, shifted.intent.intent_id).await? == 0, "nothing written");

    // An independently wrong process epoch.
    let (foreign, mut input) = one(&scope, "j2b-epoch")?;
    input.process_epoch = Uuid::now_v7();
    let result = backend.commit_clocked(&foreign, &input).await;
    ensure!(refused(&result), "wrong epoch refuses: {result:?}");
    ensure!(intents(&pool, foreign.intent.intent_id).await? == 0, "nothing written");
    Ok(())
}

/// A fixed local wrapper that returns the real helper's completed rows with one
/// client order id changed. Reached only by the actual clocked writer inside
/// the disposable migration fixture.
const CORRUPTING_WRAPPER: &str = r#"
CREATE FUNCTION public.budget_insert_clocks(
    uuid[],uuid,uuid,text,timestamptz[],uuid[],bigint[],bigint[],text[],text[],uuid[],bigint[],
    integer[],numeric[],uuid[],bigint[],integer[],numeric[],numeric[],text[],uuid[],bigint[],integer[])
RETURNS TABLE(
    attempt_id UUID,intent_id UUID,tenant_id UUID,venue_id TEXT,attempt_created_at TIMESTAMPTZ,
    clock_sample_id UUID,clock_wall_delta_ms BIGINT,clock_mono_delta_ms BIGINT,clock_verdict TEXT,
    budget_kind TEXT,budget_boot UUID,budget_offset_seconds BIGINT,budget_offset_nanoseconds INTEGER,
    budget_created_tick NUMERIC,budget_recorded_at TIMESTAMPTZ,capture_epoch UUID,
    capture_wall_seconds BIGINT,capture_wall_nanoseconds INTEGER,capture_tick_before NUMERIC,
    capture_tick_after NUMERIC,capture_kind TEXT,capture_boot UUID,capture_offset_seconds BIGINT,
    capture_offset_nanoseconds INTEGER,account_scope TEXT,client_order_id TEXT,kind TEXT,
    daily_units SMALLINT,expiry_proof_id UUID,retired_at TIMESTAMPTZ,budget_created_tick_scale INTEGER,
    budget_created_tick_fits BOOLEAN,capture_tick_before_scale INTEGER,capture_tick_before_fits BOOLEAN,
    capture_tick_after_scale INTEGER,capture_tick_after_fits BOOLEAN)
LANGUAGE sql VOLATILE AS $fn$
    SELECT r.attempt_id,r.intent_id,r.tenant_id,r.venue_id,r.attempt_created_at,r.clock_sample_id,
        r.clock_wall_delta_ms,r.clock_mono_delta_ms,r.clock_verdict,r.budget_kind,r.budget_boot,
        r.budget_offset_seconds,r.budget_offset_nanoseconds,r.budget_created_tick,r.budget_recorded_at,
        r.capture_epoch,r.capture_wall_seconds,r.capture_wall_nanoseconds,r.capture_tick_before,
        r.capture_tick_after,r.capture_kind,r.capture_boot,r.capture_offset_seconds,
        r.capture_offset_nanoseconds,r.account_scope,r.client_order_id||'-corrupt',r.kind,r.daily_units,
        r.expiry_proof_id,r.retired_at,r.budget_created_tick_scale,r.budget_created_tick_fits,
        r.capture_tick_before_scale,r.capture_tick_before_fits,r.capture_tick_after_scale,
        r.capture_tick_after_fits
    FROM public.j2b_fixture_original_insert_clocks($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
        $18,$19,$20,$21,$22,$23) r
$fn$"#;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_corrupted_completed_receipt_is_refused_before_commit(
    pool: PgPool,
) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    // Preserve the original M38 function and corrupt only its returned
    // client identity. The production-style call remains schema-qualified.
    sqlx::query(
        "ALTER FUNCTION public.budget_insert_clocks(
            uuid[],uuid,uuid,text,timestamptz[],uuid[],bigint[],bigint[],text[],text[],uuid[],bigint[],
            integer[],numeric[],uuid[],bigint[],integer[],numeric[],numeric[],text[],uuid[],bigint[],integer[])
         RENAME TO j2b_fixture_original_insert_clocks",
    )
    .execute(&pool)
    .await?;
    sqlx::query(CORRUPTING_WRAPPER).execute(&pool).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let (batch, input) = one(&scope, "j2b-corrupt")?;
    let result = backend.commit_clocked(&batch, &input).await;
    ensure!(
        matches!(&result, Err(CommitFailure::Known(why))
            if why == "clock receipt mismatch: charge route, client, kind or units"),
        "the client sees the corrupted receipt before COMMIT: {result:?}"
    );
    ensure!(intents(&pool, batch.intent.intent_id).await? == 0, "no intent committed");
    let clocks: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM public.spot_intent_clock")
        .fetch_one(&pool)
        .await?;
    ensure!(clocks == 0, "no clock or charge committed");
    Ok(())
}
