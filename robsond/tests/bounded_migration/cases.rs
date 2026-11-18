use std::time::{Duration, Instant};

use sqlx::{migrate::MigrationType, PgPool};

use super::{helpers::*, *};

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_restores_same_session_and_preserves_legacy(pool: PgPool) {
    let mut f = Fixture::new(&pool).await.unwrap();
    let expected_legacy = f.legacy_before.clone();
    let old_catalog = f.catalog_before.clone();
    let started = Instant::now();
    let run = apply_in_windows(&mut f).await;
    let applied_ok = run.as_ref().is_ok_and(|(result, _, _)| result.is_ok());
    let duration = started.elapsed();
    let status = f.adapter.status();
    let statements = f.proxy.stats().statements - f.baseline.statements;
    let after = if applied_ok {
        Some(match f.adapter.connection() {
            Ok(connection) => sqlx::query_as::<_, (i32, String, String, String)>(
                "SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')",
            ).fetch_one(connection).await.map_err(MigrateError::from),
            Err(error) => Err(error),
        })
    } else {
        None
    };
    let same_pid = f.pid;
    // An ordinary later version is a test witness, not production migration39.
    let next = Migration::new(
        VERSION + 1,
        "ordinary same-session witness".into(),
        MigrationType::Simple,
        "CREATE TABLE public52_next_witness (id INTEGER)".into(),
        false,
    );
    let next_result = if applied_ok {
        Some(f.adapter.apply(&next).await)
    } else {
        None
    };
    let finished = f.finish().await;
    eprintln!(
        "primary apply={:?} duration={duration:?} status={status:?}",
        run.as_ref().map(|(applied, _, producer_duration)| (applied, producer_duration))
    );
    let (applied, windows, _producer_duration) = run.expect("initial private stage fixture setup");
    applied.expect("actual original52 must complete under the production200ms bound");
    let finished = finished.expect("cleanup/observation after original producer result");
    assert!(finished.retired, "actual52_backend_not_retired");
    assert!(duration < Duration::from_millis(200), "actual52_global200");
    assert_eq!(statements, 58, "actual52_apply_inventory");
    assert_eq!(status.phase, Phase::Restored);
    assert_eq!(status.failure, None);
    assert_eq!(
        after.unwrap().unwrap(),
        (same_pid, "310ms".into(), "410ms".into(), "510ms".into()),
        "actual52_same_session_settings_restored"
    );
    next_result.unwrap().expect("ordinary39 only after successful restoration");
    assert_eq!(finished.legacy, expected_legacy, "actual52_changed_legacy_or_history");
    assert_ne!(finished.catalog, old_catalog, "actual52_no_schema_effect");
    let (description, success, checksum, elapsed) = finished.metadata.unwrap();
    assert_eq!(description, migration().description.as_ref());
    assert!(success);
    assert_eq!(checksum, migration().checksum.as_ref());
    assert!(elapsed >= 0);
    let windows = windows.expect("all four received windows must be observed");
    assert_eq!(windows.len(), 4);
    let chunks = fixed52::chunks(migration()).unwrap();
    let mut actual_ddl = Vec::with_capacity(52);
    for (n, window) in windows.iter().enumerate() {
        assert_eq!(window.dropped, 0, "received_window_dropped");
        assert_eq!(window.statements, [18, 34, 50, 58][n]);
        assert_eq!(window.text.len(), [18, 16, 16, 8][n]);
        let range = match n {
            0 => 2..18,
            1 | 2 => 0..16,
            _ => 0..4,
        };
        actual_ddl
            .extend(window.text[range].iter().map(|sql| sql.trim().trim_end_matches(';').trim()));
    }
    let expected: Vec<_> =
        chunks.iter().map(|sql| sql.trim().trim_end_matches(';').trim()).collect();
    assert_eq!(actual_ddl, expected, "original52_received_order_or_text_changed");
    assert_eq!(windows[0].text[1], "BEGIN");
    assert!(windows[0].text[0].starts_with("WITH old AS MATERIALIZED"));
    let final_window = &windows[3].text;
    assert!(final_window[4].contains("INSERT INTO _sqlx_migrations"));
    assert_eq!(final_window[5], "COMMIT");
    assert!(final_window[6].contains("UPDATE _sqlx_migrations"));
    assert!(final_window[7].starts_with("SELECT set_config('lock_timeout',$1,false)"));
    // Only received text and actual public call returns are established here.
    assert_eq!(finished.dropped, 0);
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_metadata_failure_rolls_back_and_refuses_39(pool: PgPool) {
    let mut f = Fixture::new(&pool).await.unwrap();
    let prepared = async {
        sqlx::raw_sql("CREATE FUNCTION public52_metadata_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=20240101000038 THEN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='public52 metadata rejected'; END IF; RETURN NEW; END $$; CREATE TRIGGER public52_metadata_fail BEFORE INSERT ON _sqlx_migrations FOR EACH ROW EXECUTE FUNCTION public52_metadata_fail()")
        .execute(&pool).await?;
        f.refreshed_catalog().await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Err(error) = prepared {
        let cleanup = f.finish().await;
        panic!("metadata fixture setup failed: {error}; cleanup_error={:?}", cleanup.err());
    }
    let before = f.catalog_before.clone();
    let legacy = f.legacy_before.clone();
    let baseline = f.baseline.statements;
    let started = Instant::now();
    let result = f.adapter.apply(migration()).await;
    let duration = started.elapsed();
    let status = f.adapter.status();
    let prefix = f.proxy.stats().statements - f.baseline.statements;
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats().statements - f.baseline.statements;
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?}");
    let error = result.unwrap_err();
    let database = match &error {
        MigrateError::Execute(e) | MigrateError::ExecuteMigration(e, _) => e.as_database_error(),
        _ => None,
    }
    .expect("actual server error, not a timeout/setup refusal");
    assert_eq!(database.code().as_deref(), Some("23514"));
    assert_eq!(database.message(), "public52 metadata rejected");
    let finished = finished.expect("metadata-failure cleanup observation");
    assert!(finished.retired, "metadata_failure_backend_not_retired");
    assert_eq!(prefix, 55, "metadata_failure_original52_prefix");
    assert_eq!(after_refusals, prefix, "retired_entry_sent_later_sql_or39");
    assert!(refused, "retired_adapter_reused");
    assert_eq!(
        status.failure,
        Some(Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted
        })
    );
    assert_eq!(finished.status, status);
    assert_eq!(
        finished.stats.statements - baseline,
        after_refusals,
        "retirement_flushed_cleanup_sql"
    );
    assert_eq!(finished.metadata, None, "failed_metadata_committed");
    assert_eq!(finished.catalog, before, "failed_metadata_left_partial52_schema");
    assert_eq!(finished.legacy, legacy, "failed_metadata_changed_legacy");
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn actual52_lost_commit_ack_is_uncertain_and_durable(pool: PgPool) {
    let mut f = Fixture::new(&pool).await.unwrap();
    let legacy = f.legacy_before.clone();
    f.proxy.set_mode(robson_testkit::db_proxy::ProxyMode::DropCommitAck);
    let started = Instant::now();
    let result = f.adapter.apply(migration()).await;
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    let prefix = stats.statements - f.baseline.statements;
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats().statements;
    let finished = f.finish().await;
    eprintln!("primary apply={result:?} duration={duration:?} status={status:?}");
    assert!(result.is_err(), "lost_commit_ack_reported_success");
    let finished = finished.expect("lost-ACK cleanup observation");
    assert!(finished.retired, "lost_commit_backend_not_retired");
    assert_eq!(
        status.failure,
        Some(Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted
        })
    );
    assert_eq!(prefix, 56, "lost_commit_sent_elapsed_restore_or39");
    assert_eq!(stats.commit_acks_dropped, 1, "commit_fault_did_not_fire");
    assert!(refused);
    assert_eq!(after_refusals, stats.statements);
    let (description, success, checksum, elapsed) = finished.metadata.as_ref().unwrap();
    assert_eq!(description, migration().description.as_ref());
    assert!(*success);
    assert_eq!(checksum.as_slice(), migration().checksum.as_ref());
    assert_eq!(*elapsed, -1, "lost_commit_ran_postcommit_elapsed");
    assert_eq!(finished.legacy, legacy);
    assert!(finished.metadata.is_some());
}
