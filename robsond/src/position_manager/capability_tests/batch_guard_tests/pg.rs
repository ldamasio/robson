//! Full caller wire counts include lifecycle snapshots and SQLx cleanup.
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::postgres::{PgPoolOptions, PgSslMode};

use super::*;

struct FinalAuditProxy {
    delegate: crate::query_engine::EventLogQueryRecorder,
    proxy: Arc<DbProxy>,
    lose_final_ack: bool,
}

#[async_trait]
impl QueryRecorder for FinalAuditProxy {
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        cause: &str,
    ) -> Result<(), QueryRecorderError> {
        if self.lose_final_ack && cause == "failed" {
            self.proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO event_log"));
        }
        self.delegate.record_transition(query, cause).await
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_refusal_full_caller_sql_bounds_include_failed_commit_cleanup(pool: sqlx::PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = Arc::new(DbProxy::start(upstream).await.unwrap());
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .test_before_acquire(false)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_with(
            options
                .as_ref()
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(PgSslMode::Disable),
        )
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION batch_audit_fault() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE observed BIGINT; BEGIN SELECT count(*) INTO observed FROM event_log WHERE tenant_id = NEW.tenant_id; IF observed >= TG_ARGV[0]::BIGINT THEN RAISE EXCEPTION 'batch audit fixture fault' USING ERRCODE = '23514'; END IF; RETURN NEW; END $$").execute(&pool).await.unwrap();
    // (caller, failure stage, committed snapshots before target append, legs)
    let mut cases = Vec::new();
    for kind in ["close", "tick", "panic"] {
        for stage in [
            "success",
            "sequence",
            "idempotency",
            "event",
            "stream_update",
            "deferred_commit",
            "lost_commit",
        ] {
            cases.push((kind, stage, if kind == "tick" { 0 } else { 3 }, 1));
        }
    }
    for kind in ["close", "panic"] {
        for prefix in 0..3 {
            cases.push((kind, "earlier_commit", prefix, 1));
        }
    }
    cases.push(("panic", "success", 3, 3));
    assert_eq!(cases.len(), 28);
    for (kind, stage, prefix, legs) in cases {
        let tenant = Uuid::now_v7();
        let stream = format!("query:balance-batch:{tenant}");
        let threshold = if matches!(stage, "stream_update" | "deferred_commit" | "earlier_commit") {
            prefix + 1
        } else {
            prefix
        };
        let ddl=match stage {
            "sequence"=>format!("CREATE TRIGGER batch_fail_insert BEFORE INSERT ON stream_state FOR EACH ROW EXECUTE FUNCTION batch_audit_fault('{threshold}'); CREATE TRIGGER batch_fail_update BEFORE UPDATE OF last_seq ON stream_state FOR EACH ROW WHEN (NEW.last_seq IS DISTINCT FROM OLD.last_seq) EXECUTE FUNCTION batch_audit_fault('{threshold}')"),
            "idempotency"=>format!("CREATE TRIGGER batch_fail BEFORE INSERT ON event_idempotency FOR EACH ROW EXECUTE FUNCTION batch_audit_fault('{threshold}')"),
            "event"=>format!("CREATE TRIGGER batch_fail BEFORE INSERT ON event_log FOR EACH ROW EXECUTE FUNCTION batch_audit_fault('{threshold}')"),
            "stream_update"=>format!("CREATE TRIGGER batch_fail BEFORE UPDATE OF last_event_id ON stream_state FOR EACH ROW EXECUTE FUNCTION batch_audit_fault('{threshold}')"),
            "deferred_commit"|"earlier_commit"=>format!("CREATE CONSTRAINT TRIGGER batch_fail AFTER INSERT ON event_log DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION batch_audit_fault('{threshold}')"),
            _=>String::new(),
        };
        if !ddl.is_empty() {
            sqlx::raw_sql(&ddl).execute(&pool).await.unwrap();
        }
        let recorder = Arc::new(Recorder {
            delegate: Some(Arc::new(FinalAuditProxy {
                delegate: crate::query_engine::EventLogQueryRecorder::new(
                    proxied.clone(),
                    tenant,
                    stream.clone(),
                ),
                proxy: proxy.clone(),
                lose_final_ack: stage == "lost_commit",
            })),
            ..Recorder::default()
        });
        let mut f = fixture_with_recorder(recorder);
        // Configure the real domain writer too: an accidental batch event
        // cannot hide behind an absent event-log pool in this count fixture.
        f.manager = Arc::new(
            Arc::try_unwrap(f.manager).ok().unwrap().with_event_log(proxied.clone(), tenant),
        );
        let mut positions = Vec::new();
        for _ in 0..legs {
            positions.push(active(&f).await);
        }
        proxy.enable_query_trace(256);
        let before = proxy.stats().statements;
        let result = call_exit(&f, &positions[0].0, positions[0].1, kind).await;
        let successful_panic = kind == "panic" && !(stage == "earlier_commit" && prefix == 0);
        if successful_panic {
            assert!(result.is_ok(), "{kind}/{stage}/{prefix}: {result:?}");
        } else if stage == "success" {
            assert!(
                matches!(result, Err(DaemonError::Exec(ExecError::BalanceOwnerRequired))),
                "{kind}: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(DaemonError::QueryAudit(_))),
                "{kind}/{stage}/{prefix}: {result:?}"
            );
        }
        // Acquiring the sole slot waits for queued SQLx ROLLBACK, including
        // after a server-rejected COMMIT. No measurement-only SELECT is sent.
        let ready = tokio::time::timeout(std::time::Duration::from_secs(3), proxied.acquire())
            .await
            .unwrap()
            .unwrap();
        let tail = match stage {
            "sequence" => 3,
            "idempotency" => 4,
            "event" => 5,
            "stream_update" => 6,
            "deferred_commit" => 7,
            "earlier_commit" => {
                if prefix == 0 {
                    7
                } else {
                    14
                }
            },
            _ => 6,
        };
        let expected = if stage == "earlier_commit" {
            prefix * 6 + tail
        } else {
            (prefix * 6 + tail) * legs
        };
        let trace = proxy.query_trace();
        assert_eq!(proxy.query_trace_dropped(), 0);
        assert_eq!(
            proxy.stats().statements - before,
            expected,
            "{kind}/{stage}/{prefix}/{legs}: {trace:?}"
        );
        drop(ready);
        for (p, _) in &positions {
            unchanged(&f, p).await;
            assert!(f.manager.store.events().find_by_position(p.id).await.unwrap().is_empty());
        }
        assert!(f.manager.exit_retry_after.read().await.is_empty());
        assert!(f.manager.exit_overtaken_suspects.read().await.is_empty());
        let expected_failed = if stage == "earlier_commit" && prefix == 0 {
            0
        } else {
            legs
        };
        assert_eq!(
            f.recorder
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|cause| cause.as_str() == "failed")
                .count(),
            expected_failed as usize
        );
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log WHERE tenant_id=$1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
        let expected_rows = if matches!(stage, "success" | "lost_commit") {
            (prefix + 1) * legs
        } else {
            prefix
        };
        assert_eq!(rows, expected_rows as i64, "{kind}/{stage}/{prefix}");
        let domains: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type <> $2",
        )
        .bind(tenant)
        .bind(robson_eventlog::QUERY_STATE_CHANGED_EVENT_TYPE)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(domains, 0);
        f.manager.shutdown_token.cancel();
        for table in ["stream_state", "event_idempotency", "event_log"] {
            for name in ["batch_fail", "batch_fail_insert", "batch_fail_update"] {
                sqlx::raw_sql(&format!("DROP TRIGGER IF EXISTS {name} ON {table}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
    }
    proxied.close().await;
}
