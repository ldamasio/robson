//! Native receipt controls covering complete sample evidence and history.
use robson_exec::{
    clock_records::RawClockSample,
    order_journal::JournalBackend,
    venue::VenueId,
    venue_ledger::{ClockEvidence, ClockVerdict},
};

use super::*;
use crate::venue_clock_store::ClockSampleStore;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_persisted_sample_and_a_retired_charge_round_trip(pool: PgPool) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let mut samples = ClockSampleStore::from_committed_epoch(&pool, scope.tenant, &scope.start);
    let send = scope.clock.wall_bracket(scope.epoch)?;
    let receive = scope.clock.wall_bracket(scope.epoch)?;
    let sample = RawClockSample {
        id: Uuid::now_v7(),
        tenant: scope.tenant,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send,
        receive,
        // Local fixture response for storage validation, no venue qualification.
        server_ms: receive.wall_seconds * 1_000 + i64::from(receive.wall_nanoseconds / 1_000_000),
    };
    ensure!(sample.numbers()?.failures == 0, "sample numerical fixture");
    samples
        .persist(1, sample.clone(), None)
        .await
        .map_err(|error| anyhow!("persisting exact sample: {error:?}"))?;

    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let (batch, mut input) = one(&scope, "j2b-sampled")?;
    let mark = input.attempts[0].capture;
    let wall_ms = |raw: RawClockCapture| {
        raw.wall_seconds * 1_000 + i64::from(raw.wall_nanoseconds / 1_000_000)
    };
    let proof_sample = ClockEvidence {
        sample_id: sample.id,
        wall_delta_ms: wall_ms(mark) - wall_ms(receive),
        mono_delta_ms: i64::try_from((mark.after_ns - receive.after_ns) / 1_000_000)?,
        verdict: ClockVerdict::Valid,
    };
    input.attempts[0].sample = Some(proof_sample.clone());
    let attempt = input.attempts[0].attempt_id;
    let active = ClockReceipt {
        attempt_id: attempt,
        daily_units: 1,
        retired: false,
    };
    ensure!(backend.commit_clocked(&batch, &input).await == Ok(vec![active]));
    let stored: (Uuid, i64, i64, String) = sqlx::query_as(
        "SELECT c.clock_sample_id,c.clock_wall_delta_ms,c.clock_mono_delta_ms,c.clock_verdict
         FROM spot_intent_clock c JOIN clock_samples s
           ON (s.sample_id,s.tenant_id,s.venue_id)=(c.clock_sample_id,c.tenant_id,c.venue_id)
         WHERE c.attempt_id=$1 AND s.process_epoch=$2 AND s.numeric_verdict='valid'",
    )
    .bind(attempt)
    .bind(scope.epoch)
    .fetch_one(&pool)
    .await?;
    ensure!(
        stored
            == (
                proof_sample.sample_id,
                proof_sample.wall_delta_ms,
                proof_sample.mono_delta_ms,
                "valid".to_owned(),
            ),
        "all four optional sample fields are bound with their exact source"
    );

    // Stage a legal historical tombstone in the disposable fixture. This tests
    // receipt storage semantics without exercising a live retirement owner.
    let proof = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO budget_expiry_proofs(
            proof_id,venue_id,proof_kind,source_attempt_id,clock_domain,
            start_tick,end_tick,recorded_at)
         SELECT $1,venue_id,'same_domain',attempt_id,budget_clock_domain,
            budget_created_tick,budget_created_tick+86460000000000,CURRENT_TIMESTAMP
         FROM budget_daily_charges WHERE attempt_id=$2",
    )
    .bind(proof)
    .bind(attempt)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE budget_daily_charges SET expiry_proof_id=$1,retired_at=CURRENT_TIMESTAMP
         WHERE attempt_id=$2",
    )
    .bind(proof)
    .bind(attempt)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let expected = ClockReceipt { retired: true, ..active };
    let replay = backend.commit_clocked(&batch, &input).await;
    ensure!(
        replay == Ok(vec![expected]),
        "replay observes the original tombstone: {replay:?}"
    );
    let durable: (i64, bool) = sqlx::query_as(
        "SELECT count(*), bool_and(expiry_proof_id=$2 AND retired_at IS NOT NULL)
         FROM budget_daily_charges WHERE attempt_id=$1",
    )
    .bind(attempt)
    .bind(proof)
    .fetch_one(&pool)
    .await?;
    ensure!(durable == (1, true), "replay retained the original retirement");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_clocked_route_cannot_enrich_legacy_nulls_or_rebind_an_attempt(
    pool: PgPool,
) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let (legacy, old_input) = one(&scope, "j2b-legacy")?;
    backend
        .commit(&legacy)
        .await
        .map_err(|error| anyhow!("legacy write: {error:?}"))?;

    let replay = backend.commit_clocked(&legacy, &old_input).await;
    ensure!(
        matches!(replay, Err(CommitFailure::Known(ref error))
            if error.contains("clock completion receipt has a missing or conflicting row")),
        "clocked replay may not enrich a legacy NULL clock: {replay:?}"
    );
    let (mut foreign, mut input) = one(&scope, "j2b-foreign-attempt")?;
    let original_attempt = legacy.attempts[0].attempt_id;
    foreign.attempts[0].attempt_id = original_attempt;
    input.attempts[0].attempt_id = original_attempt;
    let result = backend.commit_clocked(&foreign, &input).await;
    ensure!(
        matches!(result, Err(CommitFailure::Known(ref error))
            if error.contains("clock completion receipt has a missing or conflicting row")),
        "foreign source identity must be refused after the attempt upsert: {result:?}"
    );
    ensure!(
        intents(&pool, foreign.intent.intent_id).await? == 0,
        "foreign intent rolled back"
    );
    let retained: bool = sqlx::query_scalar(
        "SELECT a.client_order_id=$3 AND d.client_order_id=$3
           AND c.capture_epoch IS NULL AND c.budget_clock_domain IS NULL AND c.budget_created_tick IS NULL
           AND d.budget_clock_domain IS NULL AND d.budget_created_tick IS NULL AND d.daily_units=1
         FROM spot_intent_clock c JOIN budget_daily_charges d USING(attempt_id)
         JOIN submit_attempts a USING(attempt_id) WHERE c.attempt_id=$1 AND c.intent_id=$2",
    )
    .bind(original_attempt)
    .bind(legacy.intent.intent_id)
    .bind(&legacy.intent.client_order_id)
    .fetch_one(&pool)
    .await?;
    ensure!(retained, "legacy parent/charge retained exact identity and NULL timing");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn missing_and_extra_completed_rows_refuse_and_roll_back(pool: PgPool) -> anyhow::Result<()> {
    let scope = scope(&pool).await?;
    let base = CORRUPTING_WRAPPER.replace("r.client_order_id||'-corrupt'", "r.client_order_id");
    let missing = base.replace(") r\n$fn$", ") r WHERE r.attempt_id IS NULL\n$fn$");
    let extra = base
        .replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION")
        .replace(") r\n$fn$", ") r CROSS JOIN (VALUES (1),(2)) AS copies(n)\n$fn$");
    assert_ne!(base, missing);
    assert!(extra.contains("CROSS JOIN (VALUES (1),(2))"));

    wrap_real_helper(&pool, &missing).await?;
    let backend = PgJournalBackend::new(&pool, scope.tenant);
    let (batch, input) = one(&scope, "j2b-missing")?;
    for symptom in ["missing", "extra"] {
        if symptom == "extra" {
            sqlx::query(&extra).execute(&pool).await?;
        }
        let result = backend.commit_clocked(&batch, &input).await;
        ensure!(
            matches!(&result, Err(CommitFailure::Known(why)) if why.starts_with("clock receipt mismatch:")),
            "{symptom} completed rows must be refused by the client: {result:?}"
        );
        ensure!(intents(&pool, batch.intent.intent_id).await? == 0, "nothing committed");
    }
    Ok(())
}
