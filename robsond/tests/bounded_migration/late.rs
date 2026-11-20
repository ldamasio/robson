//! Actual late public-call faults; app gates never claim wire completion.
use std::time::Instant;

use robson_testkit::db_proxy::ProxyMode;
use sqlx::PgPool;

use super::{
    super::stages::{self, Edge, Stage},
    helpers::*,
    *,
};

async fn late_ack_loss(
    pool: PgPool,
    stage: Stage,
    needle: &'static str,
    prefix: u64,
    expected: Failure,
) {
    let mut f = Fixture::new(&pool).await.unwrap();
    let legacy = f.legacy_before.clone();
    let before_catalog = f.catalog_before.clone();
    let baseline = f.baseline;
    let armed = stages::arm(&f.adapter.gate, stage, Edge::BeforeCall);
    let (hit, release) = match armed {
        Ok(gate) => gate,
        Err(error) => {
            let cleanup = f.finish().await;
            panic!("late gate setup failed: {error}; cleanup_error={:?}", cleanup.err());
        },
    };
    let ((primary, duration), gate) = {
        let adapter = &mut f.adapter;
        let proxy = &f.proxy;
        let producer = async {
            let started = Instant::now();
            let result = adapter.apply(migration()).await;
            (result, started.elapsed())
        };
        let controller = async {
            hit.await.map_err(|_| refusal("late stage not reached"))?;
            let before = proxy.stats().statements - baseline.statements;
            proxy.set_mode(ProxyMode::DropAutocommitAckOf(needle));
            release.send(()).map_err(|_| refusal("late stage release lost"))?;
            Ok::<_, MigrateError>(before)
        };
        tokio::join!(producer, tokio::time::timeout(CONTROLLER_CEILING, controller))
    };
    // Preserve producer/counter facts before any assertion and before cleanup.
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let drops = f.proxy.autocommit_acks_dropped();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats().statements;
    let finished = f.finish().await;
    eprintln!("primary apply={primary:?} duration={duration:?} status={status:?} gate={gate:?}");
    assert!(primary.is_err(), "late_ack_loss_reported_success");
    let before = gate.expect("late stage observer deadline").expect("actual late stage");
    let finished = finished.expect("late-ACK cleanup observation");
    assert_eq!(before, prefix - 1, "late_mode_armed_at_wrong_call");
    assert_eq!(drops, 1, "actual_autocommit_ack_loss_did_not_fire");
    assert_eq!(stats.statements - baseline.statements, prefix, "late_ack_wrong_prefix");
    assert_eq!(stats.commits - baseline.commits, 1, "late_ack_original_commit_missing");
    assert_eq!(stats.commit_acks_dropped - baseline.commit_acks_dropped, 0);
    assert_eq!(status.failure, Some(expected), "late_ack_wrong_runtime_outcome");
    assert_eq!(status.phase, Phase::Retired);
    assert_eq!(finished.status, status);
    assert!(finished.retired, "late_ack_original_backend_not_retired");
    assert!(refused, "late_ack_retired_adapter_reused");
    assert_eq!(after_refusals, stats.statements, "late_ack_refusal_sent_sql_or39");
    assert_eq!(finished.stats.statements, after_refusals, "late_ack_retirement_flushed_sql");
    assert!(finished.tail.last().is_some_and(|sql| sql.contains(needle)));
    let (description, success, checksum, elapsed) = finished.metadata.unwrap();
    assert_eq!(description, migration().description.as_ref());
    assert!(success);
    assert_eq!(checksum.as_slice(), migration().checksum.as_ref());
    assert!(elapsed >= 0, "late_ack_durable_elapsed_missing");
    assert_eq!(finished.legacy, legacy, "late_ack_changed_legacy");
    assert_ne!(finished.catalog, before_catalog, "late_ack_schema_effect_missing");
    // Changed catalog is not exact136/composite/native-RI equivalence.
    // A retired backend's restored settings are not inferred from another PID.
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_elapsed_ack_loss_retires_uncertain(pool: PgPool) {
    late_ack_loss(pool, Stage::Elapsed, "UPDATE _sqlx_migrations", 57, Failure {
        at: Phase::DelegatedApply,
        outcome: Outcome::MayHaveCommitted,
    })
    .await;
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_restore_ack_loss_blocks_reuse(pool: PgPool) {
    late_ack_loss(
        pool,
        Stage::Restore,
        "SELECT set_config('lock_timeout',$1,false)",
        58,
        Failure {
            at: Phase::RestoreStarted,
            outcome: Outcome::AppliedButSessionUnrestored,
        },
    )
    .await;
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_cancel_before_restore_retires_original(pool: PgPool) {
    let mut f = Fixture::new(&pool).await.unwrap();
    let legacy = f.legacy_before.clone();
    let before_catalog = f.catalog_before.clone();
    let baseline = f.baseline;
    let armed = stages::arm(&f.adapter.gate, Stage::Restore, Edge::BeforeCall);
    let (hit, release) = match armed {
        Ok(gate) => gate,
        Err(error) => {
            let cleanup = f.finish().await;
            panic!("cancel gate setup failed: {error}; cleanup_error={:?}", cleanup.err());
        },
    };
    let (primary, reached, duration) = {
        let started = Instant::now();
        // Migrate::apply returns the owned boxed future. Dropping a pinned
        // reference would not cancel it; drop this original owning value.
        let mut producer = f.adapter.apply(migration());
        let (primary, reached) = tokio::select! {
            result = &mut producer => (Some(result), None),
            reached = hit => (None, Some(reached)),
        };
        drop(producer);
        drop(release);
        (primary, reached, started.elapsed())
    };
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let drops = f.proxy.autocommit_acks_dropped();
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats().statements;
    let finished = f.finish().await;
    eprintln!(
        "primary return={primary:?} reached={reached:?} duration={duration:?} status={status:?}"
    );
    assert!(primary.is_none(), "cancel_producer_returned_before_the_cut");
    assert!(reached.is_some_and(|result| result.is_ok()), "cancel_actual_stage_missing");
    assert!(duration < APPLY_BOUND, "cancel_cut_missed_apply_deadline");
    let finished = finished.expect("cancel cleanup observation");
    assert_eq!(stats.statements - baseline.statements, 57, "cancel_polled_restore_or39");
    assert_eq!(stats.commits - baseline.commits, 1, "cancel_original_commit_missing");
    assert_eq!(drops, 0, "cancel_used_a_transport_ACK_fault");
    assert_eq!(stats.commit_acks_dropped - baseline.commit_acks_dropped, 0);
    assert_eq!(
        status.failure,
        Some(Failure {
            at: Phase::RestoreStarted,
            outcome: Outcome::AppliedButSessionUnrestored,
        }),
        "cancel_wrong_runtime_outcome"
    );
    assert_eq!(status.phase, Phase::Retired);
    assert_eq!(finished.status, status);
    assert!(finished.retired, "cancel_original_backend_not_retired");
    assert!(refused, "cancel_retired_adapter_reused");
    assert_eq!(after_refusals, stats.statements, "cancel_refusal_sent_sql_or39");
    assert_eq!(finished.stats.statements, after_refusals, "cancel_retirement_flushed_sql");
    assert!(finished.tail.last().is_some_and(|sql| sql.contains("UPDATE _sqlx_migrations")));
    let (description, success, checksum, elapsed) = finished.metadata.unwrap();
    assert_eq!(description, migration().description.as_ref());
    assert!(success);
    assert_eq!(checksum.as_slice(), migration().checksum.as_ref());
    assert!(elapsed >= 0, "cancel_durable_elapsed_missing");
    assert_eq!(finished.legacy, legacy, "cancel_changed_legacy");
    assert_ne!(finished.catalog, before_catalog, "cancel_schema_effect_missing");
    // This cut is at an app gate before restore, not a held SQLx response.
}
