use super::*;

async fn recreate_index(pool: &PgPool, definition: &str) {
    sqlx::query(&format!("DROP INDEX IF EXISTS {SPOT_INDEX}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(definition).execute(pool).await.unwrap();
}
fn correct_index() -> String {
    format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='binance:spot'")
}
async fn no_temp_objects(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(3),async {loop {
        let objects=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_class WHERE relname LIKE 'robson_expected_%'").fetch_one(pool).await.unwrap();
        let transactions=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND state LIKE 'idle in transaction%'").fetch_one(pool).await.unwrap();
        if objects==0 && transactions==0 {break;}
        tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.expect("temporary objects or an idle transaction survived cleanup");
}
async fn wait_until_retired(pool: &PgPool, pid: i32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if !exists {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server retained the retired transaction");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn readiness_compares_the_same_servers_expected_predicate(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, Uuid::now_v7());
    backend.check_ready().await.unwrap();
    for _ in 0..3 {
        proxy.enable_query_trace(16);
        let before = proxy.stats().statements;
        backend.check_balance_schema(SPOT, true).await.unwrap();
        assert_eq!(proxy.stats().statements - before, 7);
        let trace = proxy.query_trace();
        assert_eq!(trace.len(), 7);
        assert_eq!(trace[0], "BEGIN");
        assert_eq!(trace[6], "ROLLBACK");
        assert!(trace[1].contains("idle_in_transaction_session_timeout"));
        assert!(trace[2].contains("pg_get_expr"));
        assert!(trace[3].starts_with("CREATE TEMP TABLE"));
        assert!(trace[3].contains("LIKE"));
        assert!(trace[4].contains("WHERE venue_id = 'binance:spot'"));
        assert!(trace[5].contains("pg_get_expr"));
        assert!(!trace.iter().any(|sql| sql.contains("SELECT * FROM venue_orders")
            || sql.contains("SELECT * FROM submit_attempts")));
        no_temp_objects(&pool).await;
    }
    recreate_index(&pool,&format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='other:spot'")).await;
    let before = proxy.stats().statements;
    assert!(backend
        .check_balance_schema(SPOT, true)
        .await
        .unwrap_err()
        .contains("predicate"));
    assert_eq!(proxy.stats().statements - before, 7);
    no_temp_objects(&pool).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_one_attempt_journal_is_not_ready_with_a_wrong_definition(pool: PgPool) {
    sqlx::query(
        "CREATE TABLE other_attempts (venue_id text,account_scope text,client_order_id text)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    let definitions=[
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (attempt_id)"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='other'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (account_scope,venue_id,client_order_id) WHERE venue_id='binance:spot'"),
        format!("CREATE INDEX {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='binance:spot'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts ((venue_id||''),account_scope,client_order_id) WHERE venue_id='binance:spot'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON other_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='binance:spot'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id DESC) WHERE venue_id='binance:spot'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id COLLATE \"C\",account_scope,client_order_id) WHERE venue_id='binance:spot'"),
        format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts (venue_id text_pattern_ops,account_scope,client_order_id) WHERE venue_id='binance:spot'"),
    ];
    for definition in definitions {
        recreate_index(&pool, &definition).await;
        assert!(backend.check_balance_schema(SPOT, true).await.is_err(), "accepted {definition}");
        no_temp_objects(&pool).await;
    }
    recreate_index(&pool, &correct_index()).await;
    backend.check_balance_schema(SPOT, true).await.unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_one_attempt_journal_is_not_ready_with_an_invalid_index(pool: PgPool) {
    sqlx::query(&format!("DROP INDEX {SPOT_INDEX}")).execute(&pool).await.unwrap();
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let backend = PgJournalBackend::new(&pool, tenant);
    let mut batch = balance_batch("duplicate", IntentKind::Exit);
    backend.commit(&batch).await.unwrap();
    batch.attempts = vec![attempt_row(false)];
    backend.commit(&batch).await.unwrap();
    let sql=format!("CREATE UNIQUE INDEX CONCURRENTLY {SPOT_INDEX} ON submit_attempts (venue_id,account_scope,client_order_id) WHERE venue_id='binance:spot'");
    assert!(sqlx::query(&sql).execute(&pool).await.is_err());
    let valid: bool =
        sqlx::query_scalar("SELECT indisvalid FROM pg_index WHERE indexrelid=to_regclass($1)")
            .bind(SPOT_INDEX)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!valid);
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_one_attempt_journal_is_not_ready_without_the_index_or_a_table(pool: PgPool) {
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    sqlx::query(&format!("DROP INDEX {SPOT_INDEX}")).execute(&pool).await.unwrap();
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
    backend.check_balance_schema(SPOT, false).await.unwrap();
    recreate_index(&pool, &correct_index()).await;
    sqlx::query("DROP TABLE venue_accounts").execute(&pool).await.unwrap();
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
    assert!(backend.check_balance_schema(SPOT, false).await.is_err());
    assert!(backend
        .check_balance_schema("test:unsupported", true)
        .await
        .unwrap_err()
        .contains("no durable one-attempt index"));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_usdm_journal_ignores_a_missing_invalid_or_mismatched_spot_index(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let tenant = Uuid::now_v7();
    let journal = journal_on(&proxied, tenant, false, false, VenueId::BINANCE_USDM);
    for bad in [None,Some(format!("CREATE INDEX {SPOT_INDEX} ON submit_attempts(attempt_id)")),Some(format!("CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts(venue_id,account_scope,client_order_id) WHERE venue_id='wrong'"))] {
        sqlx::query(&format!("DROP INDEX IF EXISTS {SPOT_INDEX}")).execute(&pool).await.unwrap();
        if let Some(sql)=bad {sqlx::query(&sql).execute(&pool).await.unwrap();}
        proxy.enable_query_trace(16);let before=proxy.stats().statements;journal.check_ready().await.unwrap();
        assert_eq!(proxy.stats().statements-before,1);let trace=proxy.query_trace();assert_eq!(trace.len(),1);
        assert!(!trace[0].contains("set_config")&&!trace[0].contains("pg_index")&&!trace[0].contains("CREATE TEMP")&&!trace[0].contains("accounting_state"));
        assert!(journal_on(&proxied,tenant,true,true,SPOT).check_ready().await.is_err());
    }
    // A real failed concurrent build leaves an invalid index under the right name.
    sqlx::query(&format!("DROP INDEX {SPOT_INDEX}")).execute(&pool).await.unwrap();
    seed_account(&pool, tenant, SPOT).await;
    let backend = PgJournalBackend::new(&pool, tenant);
    let mut duplicate = balance_batch("invalid", IntentKind::Exit);
    backend.commit(&duplicate).await.unwrap();
    duplicate.attempts = vec![attempt_row(false)];
    backend.commit(&duplicate).await.unwrap();
    assert!(sqlx::query(&format!("CREATE UNIQUE INDEX CONCURRENTLY {SPOT_INDEX} ON submit_attempts(venue_id,account_scope,client_order_id) WHERE venue_id='binance:spot'")).execute(&pool).await.is_err());
    proxy.enable_query_trace(16);
    journal.check_ready().await.unwrap();
    assert_eq!(proxy.query_trace().len(), 1);
    assert!(journal_on(&proxied, tenant, true, true, SPOT).check_ready().await.is_err());
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn temp_privilege_is_required_only_by_the_flagged_expected_index_probe(pool: PgPool) {
    // Use an existing built-in role, never create a role or database here.
    sqlx::query("GRANT SELECT ON ALL TABLES IN SCHEMA public TO pg_signal_backend")
        .execute(&pool)
        .await
        .unwrap();
    let database: String =
        sqlx::query_scalar("SELECT current_database()").fetch_one(&pool).await.unwrap();
    let quoted = format!("\"{}\"", database.replace('"', "\"\""));
    sqlx::query(&format!("REVOKE TEMPORARY ON DATABASE {quoted} FROM PUBLIC"))
        .execute(&pool)
        .await
        .unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let options = (*proxied.connect_options()).clone().options([("role", "pg_signal_backend")]);
    let restricted = PgPoolOptions::new().max_connections(2).connect_with(options).await.unwrap();
    let can_temp: bool =
        sqlx::query_scalar("SELECT has_database_privilege(current_database(),'TEMP')")
            .fetch_one(&restricted)
            .await
            .unwrap();
    assert!(!can_temp);
    let tenant = Uuid::now_v7();
    proxy.enable_query_trace(16);
    journal_on(&restricted, tenant, false, false, VenueId::BINANCE_USDM)
        .check_ready()
        .await
        .unwrap();
    assert_eq!(proxy.query_trace().len(), 1);
    assert!(journal_on(&restricted, tenant, true, true, SPOT).check_ready().await.is_err());
    no_temp_objects(&pool).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn readiness_rechecks_the_actual_index_oid_after_creating_the_expected_index(pool: PgPool) {
    let before: i64 = sqlx::query_scalar("SELECT to_regclass($1)::oid::bigint")
        .bind(SPOT_INDEX)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(&format!(r#"CREATE FUNCTION replace_index_during_probe() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN
        IF EXISTS(SELECT 1 FROM pg_event_trigger_ddl_commands() d JOIN pg_class c ON c.oid=d.objid WHERE c.relname LIKE 'robson_expected_idx_%') THEN
            EXECUTE 'DROP INDEX {SPOT_INDEX}'; EXECUTE 'CREATE UNIQUE INDEX {SPOT_INDEX} ON submit_attempts(venue_id,account_scope,client_order_id) WHERE venue_id=''binance:spot''';
        END IF; END $$; CREATE EVENT TRIGGER replace_index_during_probe ON ddl_command_end WHEN TAG IN ('CREATE INDEX') EXECUTE FUNCTION replace_index_during_probe();"#)).execute(&pool).await.unwrap();
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
    let after: i64 = sqlx::query_scalar("SELECT to_regclass($1)::oid::bigint")
        .bind(SPOT_INDEX)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after, "the probe rolled back its injected DDL too");
    sqlx::query("DROP EVENT TRIGGER replace_index_during_probe")
        .execute(&pool)
        .await
        .unwrap();
    backend.check_balance_schema(SPOT, true).await.unwrap();
    no_temp_objects(&pool).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn readiness_lost_rollback_and_injected_error_retire_connections(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, Uuid::now_v7());
    backend.check_ready().await.unwrap();
    let connections = proxy.stats().connections;
    proxy.set_mode(ProxyMode::DropRollbackAckOf("robson_expected_"));
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
    assert_eq!(proxy.rollback_acks_dropped(), 1);
    backend.check_balance_schema(SPOT, true).await.unwrap();
    assert!(proxy.stats().connections > connections);
    no_temp_objects(&pool).await;
    sqlx::raw_sql(r#"CREATE FUNCTION fail_expected_index() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN
        IF EXISTS(SELECT 1 FROM pg_event_trigger_ddl_commands() d JOIN pg_class c ON c.oid=d.objid WHERE c.relname LIKE 'robson_expected_idx_%') THEN RAISE EXCEPTION 'injected expected-index error'; END IF;
        END $$; CREATE EVENT TRIGGER fail_expected_index ON ddl_command_end WHEN TAG IN ('CREATE INDEX') EXECUTE FUNCTION fail_expected_index();"#).execute(&pool).await.unwrap();
    assert!(backend.check_balance_schema(SPOT, true).await.is_err());
    no_temp_objects(&pool).await;
    sqlx::query("DROP EVENT TRIGGER fail_expected_index")
        .execute(&pool)
        .await
        .unwrap();
    backend.check_balance_schema(SPOT, true).await.unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancelling_readiness_retires_its_connection_before_reuse(pool: PgPool) {
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let backend = Arc::new(PgJournalBackend::new(&pool, Uuid::now_v7()));
    let worker = backend.clone();
    let task = tokio::spawn(async move { worker.check_balance_schema(SPOT, true).await });
    let pid = wait_for_lock(&pool, "WITH needed").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    wait_until_retired(&pool, pid).await;
    held.rollback().await.unwrap();
    backend.check_balance_schema(SPOT, true).await.unwrap();
    no_temp_objects(&pool).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn server_idle_timeout_cleans_a_partition_after_temporary_ddl(pool: PgPool) {
    // Delay the DDL reply so the test can partition an already-created temp
    // table's transaction. The client remains alive until the server expires it.
    sqlx::raw_sql(r#"CREATE FUNCTION delay_expected_table() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN
        IF EXISTS(SELECT 1 FROM pg_event_trigger_ddl_commands() d JOIN pg_class c ON c.oid=d.objid WHERE c.relname LIKE 'robson_expected_%') THEN PERFORM pg_sleep(0.2); END IF;
        END $$; CREATE EVENT TRIGGER delay_expected_table ON ddl_command_end WHEN TAG IN ('CREATE TABLE') EXECUTE FUNCTION delay_expected_table();"#).execute(&pool).await.unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = Arc::new(PgJournalBackend::new(&proxied, Uuid::now_v7()));
    backend.check_ready().await.unwrap();
    for cancel in [false, true] {
        proxy.enable_query_trace(16);
        let worker = backend.clone();
        let task = tokio::spawn(async move { worker.check_balance_schema(SPOT, true).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !proxy.query_trace().iter().any(|sql| sql.starts_with("CREATE TEMP TABLE")) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        proxy.set_mode(ProxyMode::Blackhole);
        let pid:i32=tokio::time::timeout(Duration::from_secs(1),async {loop {
            let pid=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND state='idle in transaction' AND query LIKE 'CREATE TEMP TABLE%robson_expected_%' LIMIT 1").fetch_optional(&pool).await.unwrap();
            if let Some(pid)=pid {break pid;}tokio::time::sleep(Duration::from_millis(5)).await;
        }}).await.unwrap();
        let started = std::time::Instant::now();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            wait_until_retired(&pool, pid).await;
            assert!(
                started.elapsed() < Duration::from_millis(1500),
                "server idle timeout must fire before the client's 2s deadline"
            );
            assert!(task.await.unwrap().is_err());
        }
        wait_until_retired(&pool, pid).await;
        proxy.set_mode(ProxyMode::Pass);
        backend.check_balance_schema(SPOT, true).await.unwrap();
        no_temp_objects(&pool).await;
    }
}
