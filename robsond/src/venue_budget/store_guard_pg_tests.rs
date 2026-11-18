//! Direct SQL behavior controls for migration 38 store guards (verdict, replay,
//! tombstone, N5 revision). TEST-ONLY; no production writer authority.
use std::time::Duration;

use anyhow::{anyhow, ensure};
use robson_exec::venue_clock::NativeClock;
use sqlx::{postgres::PgQueryResult, PgPool};
use tokio::time::Instant;
use uuid::Uuid;

use crate::venue_budget::{fixture, Trace};

// Case B (migration 38 SQL 176-213) is intentionally not fabricated: the
// migration-36 venue_orders definition and a valid fixture INSERT are absent.
fn is_check_violation(result: &Result<PgQueryResult, sqlx::Error>) -> bool {
    result
        .as_ref()
        .err()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .as_deref()
        == Some("23514")
}

async fn initialize_budget(pool: &PgPool) -> anyhow::Result<()> {
    let trace = Trace::default();
    let clock = NativeClock::initialize()?;
    fixture::prepare(pool, &clock, Instant::now() + Duration::from_secs(2), &trace)
        .await
        .map_err(|error| anyhow!("budget fixture: {error:?}"))?;
    ensure!(trace.issued() == 7, "existing seven-command budget fixture");
    Ok(())
}

async fn insert_sample(
    pool: &PgPool,
    tenant: Uuid,
    epoch: Uuid,
    boot: Uuid,
    failures: i16,
    verdict: &str,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO clock_samples(
            sample_id,tenant_id,venue_id,process_epoch,clock_domain,
            send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,
            receive_wall_seconds,receive_wall_nanoseconds,
            receive_tick_before,receive_tick_after,server_ms,
            theta2_ns,r_low_ns,r_high_ns,u2_ns,
            numeric_verdict,numeric_failures,algorithm_version,recorded_at)
         VALUES(
            $1,$2,'binance:spot',$3,
            ROW('boottime',$4,0,0)::budget_clock_domain_v38,
            1000,0,1000000000,1000000000,
            1001,0,2000000000,2050000001,1000500,
            0,1000000000,1050000001,1250000001,
            $5,$6,1,CURRENT_TIMESTAMP)",
    )
    .bind(Uuid::from_u128(38))
    .bind(tenant)
    .bind(epoch)
    .bind(boot)
    .bind(verdict)
    .bind(failures)
    .execute(pool)
    .await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn verdict_rejects_high_side_50000001_and_accepts_invalid(
    pool: PgPool,
) -> anyhow::Result<()> {
    let tenant = Uuid::now_v7();
    let start = crate::process_epoch::start_epoch(&pool, tenant, "migration38-verdict").await?;
    let boot = Uuid::from_u128(39);

    // SQL 39-49: wall elapsed is 1_000_000_000 ns; r_low_ns is exact while
    // r_high_ns=1_050_000_001 makes ONLY the high-side discrepancy 50_000_001 ns,
    // so a high-side sub-ms rounding mutant cannot hide behind the low side.
    // theta2_ns=0 and u2_ns=1_250_000_001 satisfy arithmetic/brackets/wall_order.
    let rejected = insert_sample(&pool, tenant, start.epoch.epoch_id, boot, 0, "valid").await;
    ensure!(
        is_check_violation(&rejected),
        "mutant removing clock_samples_verdict would accept 50_000_001 ns as valid: {rejected:?}"
    );

    let accepted = insert_sample(&pool, tenant, start.epoch.epoch_id, boot, 2, "invalid").await?;
    ensure!(accepted.rows_affected() == 1, "matching invalid verdict is accepted");
    let exact = sqlx::query_scalar::<_, bool>(
        "SELECT numeric_failures=2 AND numeric_verdict='invalid'
            AND abs((receive_wall_seconds::numeric-send_wall_seconds::numeric)*1000000000
                +receive_wall_nanoseconds-send_wall_nanoseconds-r_high_ns)=50000001
            AND theta2_ns=0 AND r_low_ns=1000000000 AND r_high_ns=1050000001
            AND u2_ns=1250000001
         FROM clock_samples WHERE sample_id=$1",
    )
    .bind(Uuid::from_u128(38))
    .fetch_one(&pool)
    .await?;
    ensure!(exact, "accepted row retains the exact arithmetic and discrepancy");
    Ok(())
}

async fn project_clock(
    pool: &PgPool,
    attempt: Uuid,
    intent: Uuid,
    tenant: Uuid,
    enrich: bool,
    boot: Uuid,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "SELECT count(*)::bigint AS receipts FROM budget_insert_clocks(
            ARRAY[$1]::uuid[],$2,$3,'binance:spot',
            ARRAY['2024-01-01T00:00:00Z'::timestamptz],
            ARRAY[NULL::uuid],ARRAY[NULL::bigint],ARRAY[NULL::bigint],ARRAY[NULL::text],
            ARRAY[CASE WHEN $4 THEN 'boottime' END]::text[],
            ARRAY[CASE WHEN $4 THEN $5 END]::uuid[],
            ARRAY[CASE WHEN $4 THEN 0::bigint END],
            ARRAY[CASE WHEN $4 THEN 0::integer END],
            ARRAY[CASE WHEN $4 THEN 42::numeric END],
            ARRAY[NULL::uuid],ARRAY[NULL::bigint],ARRAY[NULL::integer],
            ARRAY[NULL::numeric],ARRAY[NULL::numeric],ARRAY[NULL::text],
            ARRAY[NULL::uuid],ARRAY[NULL::bigint],ARRAY[NULL::integer])",
    )
    .bind(attempt)
    .bind(intent)
    .bind(tenant)
    .bind(enrich)
    .bind(boot)
    .execute(pool)
    .await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn replay_preserves_legacy_null_and_charge_tombstone(pool: PgPool) -> anyhow::Result<()> {
    initialize_budget(&pool).await?;
    let tenant = Uuid::now_v7();
    let attempt = Uuid::from_u128(3801);
    let intent = Uuid::from_u128(3802);
    let boot = Uuid::from_u128(3803);
    let proof = Uuid::from_u128(3804);

    // These are the source and accounting parents used by source_audit/pg_tests.rs.
    sqlx::query(
        "INSERT INTO order_intents(
            intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
         VALUES($1,$2,'binance:spot','guard-test','guard-client','exit','{}'::jsonb,
            '2024-01-01T00:00:00Z'::timestamptz)",
    )
    .bind(intent)
    .bind(tenant)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO submit_attempts(
            attempt_id,venue_id,account_scope,client_order_id,state,created_at)
         VALUES($1,'binance:spot','guard-test','guard-client','started',
            '2024-01-01T00:00:00Z'::timestamptz)",
    )
    .bind(attempt)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
         VALUES($1,'binance:spot','net_from_start_non_expanding_v1')",
    )
    .bind(tenant)
    .execute(&pool)
    .await?;

    project_clock(&pool, attempt, intent, tenant, false, boot).await?;

    // SQL 561-571: ON CONFLICT must not enrich the existing clock. The changed
    // coordinates also fail the completion-receipt comparison at SQL 610-646.
    let replayed = project_clock(&pool, attempt, intent, tenant, true, boot).await;
    ensure!(
        is_check_violation(&replayed),
        "mutant removing replay protection would enrich legacy NULL budget coordinates: {replayed:?}"
    );
    let nulls = sqlx::query_scalar::<_, bool>(
        "SELECT c.budget_clock_domain IS NULL AND c.budget_created_tick IS NULL
            AND d.budget_clock_domain IS NULL AND d.budget_created_tick IS NULL
         FROM spot_intent_clock c
         JOIN budget_daily_charges d USING(attempt_id)
         WHERE c.attempt_id=$1",
    )
    .bind(attempt)
    .fetch_one(&pool)
    .await?;
    ensure!(
        nulls,
        "mutant removing replay protection would replace legacy NULL clock coordinates"
    );

    // SQL 342-353 supplies the same-domain expiry-proof parent; SQL 385-400
    // makes an existing retirement/tombstone immutable.
    sqlx::query(
        "INSERT INTO budget_expiry_proofs(
            proof_id,venue_id,proof_kind,source_attempt_id,clock_domain,
            start_tick,end_tick,recorded_at)
         VALUES($1,'binance:spot','same_domain',$2,
            ROW('boottime',$3,0,0)::budget_clock_domain_v38,
            0,86460000000000,CURRENT_TIMESTAMP)",
    )
    .bind(proof)
    .bind(attempt)
    .bind(boot)
    .execute(&pool)
    .await?;
    let retired = sqlx::query(
        "UPDATE budget_daily_charges
         SET expiry_proof_id=$2,retired_at=CURRENT_TIMESTAMP WHERE attempt_id=$1",
    )
    .bind(attempt)
    .bind(proof)
    .execute(&pool)
    .await?;
    ensure!(retired.rows_affected() == 1, "retirement fixture was installed");

    let cleared = sqlx::query(
        "UPDATE budget_daily_charges
         SET expiry_proof_id=NULL,retired_at=NULL WHERE attempt_id=$1",
    )
    .bind(attempt)
    .execute(&pool)
    .await;
    ensure!(
        is_check_violation(&cleared),
        "mutant removing budget_charge_moves_forward would clear a retirement tombstone: {cleared:?}"
    );
    let tombstone = sqlx::query_scalar::<_, bool>(
        "SELECT expiry_proof_id=$2 AND retired_at IS NOT NULL
         FROM budget_daily_charges WHERE attempt_id=$1",
    )
    .bind(attempt)
    .bind(proof)
    .fetch_one(&pool)
    .await?;
    ensure!(
        tombstone,
        "mutant removing budget_charge_moves_forward would erase the persisted tombstone"
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn n5_state_rejects_revision_regression(pool: PgPool) -> anyhow::Result<()> {
    // SQL 651-720 supplies the complete registry/key parent chain.
    sqlx::query(
        "INSERT INTO n5_registry_root(
            registry_id,schema_version,manifest_version,manifest_revision,initialized,
            registered_count,canonical_digest,initialization_id,recovery_generation,
            recovery_state,registered_at)
         VALUES(1,38,1,0,FALSE,1,decode(repeat('11',32),'hex'),$1,0,
            'quarantined',CURRENT_TIMESTAMP)",
    )
    .bind(Uuid::from_u128(3850))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO n5_refresh_keys(
            venue_id,symbol,descriptor,registry_id,registration_id,
            registered_revision,registered_at,provenance)
         VALUES('binance:spot','BTCUSDT','commission',1,$1,1,
            CURRENT_TIMESTAMP,'migration38-guard')",
    )
    .bind(Uuid::from_u128(3851))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO n5_refresh_state(
            venue_id,symbol,descriptor,revision,claim_id,owner_process,content_digest,
            claim_domain,claim_tick,claim_audit_at,eligibility_kind)
         VALUES('binance:spot','BTCUSDT','commission',2,$1,$2,
            decode(repeat('22',32),'hex'),
            ROW('boottime',$3,0,0)::budget_clock_domain_v38,
            10,CURRENT_TIMESTAMP,'first_use')",
    )
    .bind(Uuid::from_u128(3852))
    .bind(Uuid::from_u128(3853))
    .bind(Uuid::from_u128(3854))
    .execute(&pool)
    .await?;

    // SQL 744-783: revision 1 remains nonnegative and otherwise satisfies
    // n5_claim_whole, so only the moves-forward guard rejects this regression.
    let regressed = sqlx::query(
        "UPDATE n5_refresh_state SET revision=1
         WHERE venue_id='binance:spot' AND symbol='BTCUSDT' AND descriptor='commission'",
    )
    .execute(&pool)
    .await;
    ensure!(
        is_check_violation(&regressed),
        "mutant removing n5_state_moves_forward would regress revision 2 to 1: {regressed:?}"
    );
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM n5_refresh_state
         WHERE venue_id='binance:spot' AND symbol='BTCUSDT' AND descriptor='commission'",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        revision == 2,
        "mutant removing n5_state_moves_forward would persist the regressed revision"
    );
    Ok(())
}
