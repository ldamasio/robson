use super::*;

async fn isolated_pool(pool: &PgPool) -> (DbProxy, PgPool, String) {
    let (proxy, initial) = through_proxy(pool).await;
    let application = format!("ledger-retirement-{}", Uuid::now_v7());
    let options = (*initial.connect_options()).clone().application_name(&application);
    initial.close().await;
    let single = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .connect_with(options)
        .await
        .unwrap();
    (proxy, single, application)
}

async fn backend_pid(pool: &PgPool, application: &str) -> i32 {
    sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND backend_type='client backend'")
        .bind(application).fetch_one(pool).await.unwrap()
}

async fn retired(pool: &PgPool, pid: i32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let present: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if !present {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the retired backend must disappear before counting final wire statements");
}

async fn idle(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while pool.num_idle() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(pool.size(), 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_ledger_commit_counts_through_completed_retirement(pool: PgPool) {
    sqlx::raw_sql("CREATE FUNCTION reject_account_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'deferred ledger refusal' USING ERRCODE='23514'; END $$; CREATE CONSTRAINT TRIGGER reject_account_commit AFTER INSERT ON accounting_state DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_account_commit();").execute(&pool).await.unwrap();
    let (proxy, single, application) = isolated_pool(&pool).await;
    let tenant = Uuid::now_v7();
    let venue = VenueId::new(SPOT);
    let ledger = PgVenueLedger::new(&single, tenant);
    let pid = backend_pid(&pool, &application).await;
    proxy.enable_query_trace(32);
    let before = proxy.stats().statements;
    assert!(matches!(
        ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await,
        Err(LedgerError::Backend(_))
    ));
    retired(&pool, pid).await;
    assert_eq!(ledger.statements_total(), 5);
    assert_eq!(proxy.stats().statements - before, 5, "{:?}", proxy.query_trace());
    assert!(!proxy.query_trace().iter().any(|sql| sql == "ROLLBACK"));
    assert_eq!(single.size(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounting_state")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER reject_account_commit ON accounting_state")
        .execute(&pool)
        .await
        .unwrap();
    ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    idle(&single).await;
    let replacement = backend_pid(&pool, &application).await;
    assert_ne!(replacement, pid);
    ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    idle(&single).await;
    assert_eq!(backend_pid(&pool, &application).await, replacement);
    assert_eq!(ledger.statements_total(), 15);
    assert_eq!(proxy.stats().statements - before, 15);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_balance_journal_commit_counts_through_completed_retirement(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    sqlx::raw_sql("CREATE FUNCTION reject_journal_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'deferred journal refusal' USING ERRCODE='23514'; END $$; CREATE CONSTRAINT TRIGGER reject_journal_commit AFTER INSERT ON order_intents DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_journal_commit();").execute(&pool).await.unwrap();
    let (proxy, initial, _application) = isolated_pool(&pool).await;
    let backend = Arc::new(PgJournalBackend::new(&initial, tenant));
    initial.close().await;
    backend.check_ready().await.unwrap();
    proxy.enable_query_trace(32);
    let before = proxy.stats().statements;
    let batch = balance_batch("rejected-commit", IntentKind::Exit);
    assert_eq!(batch.statements(), 7);
    // J keeps its established conservative COMMIT classification.
    let (pid, result) = captured_commit(&pool, &backend, &batch).await;
    assert!(matches!(result, Err(CommitFailure::Uncertain(_))));
    retired(&pool, pid).await;
    assert_eq!(
        proxy.stats().statements - before,
        u64::from(batch.statements()),
        "{:?}",
        proxy.query_trace()
    );
    assert!(!proxy.query_trace().iter().any(|sql| sql == "ROLLBACK"));
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_intents")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

async fn recover(ledger: &PgVenueLedger, pool: &PgPool, venue: &VenueId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        ledger.ensure_accounting_row(venue, MonthlyBudgetModel::HwmV1).await.unwrap();
        idle(pool).await;
    })
    .await
    .expect("retirement must release the sole pool permit for recovery");
}

fn wire(proxy: &DbProxy, before: u64, expected: u64) {
    assert_eq!(proxy.stats().statements - before, expected, "{:?}", proxy.query_trace());
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn repeated_route_conflicts_release_the_only_pool_permit(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, single, application) = isolated_pool(&pool).await;
    let ledger = PgVenueLedger::new(&single, tenant);
    let venue = VenueId::new(SPOT);
    let position = Uuid::now_v7();
    ledger.insert_route(position, &venue, "operated", Utc::now()).await.unwrap();
    idle(&single).await;
    for _ in 0..4 {
        let pid = backend_pid(&pool, &application).await;
        proxy.enable_query_trace(16);
        let before = proxy.stats().statements;
        let counted = ledger.statements_total();
        assert_eq!(
            ledger.insert_route(position, &venue, "other", Utc::now()).await,
            Err(LedgerError::RouteConflict)
        );
        retired(&pool, pid).await;
        wire(&proxy, before, 4);
        assert_eq!(ledger.statements_total() - counted, 4);
        assert!(!proxy.query_trace().iter().any(|sql| sql == "ROLLBACK"));
        recover(&ledger, &single, &venue).await;
        assert_ne!(backend_pid(&pool, &application).await, pid);
        let scope: String =
            sqlx::query_scalar("SELECT account_scope FROM leg_routes WHERE position_id=$1")
                .bind(position)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(scope, "operated");
        assert_eq!(version(&pool, tenant, SPOT).await, 0);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn ledger_error_cancellation_and_lost_ack_have_no_hidden_cleanup_sql(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let (proxy, single, application) = isolated_pool(&pool).await;
    let ledger = Arc::new(PgVenueLedger::new(&single, tenant));
    let venue = VenueId::new(SPOT);
    sqlx::raw_sql("CREATE FUNCTION reject_before_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'insert refusal' USING ERRCODE='23514'; END $$; CREATE TRIGGER reject_before_insert BEFORE INSERT ON accounting_state FOR EACH ROW EXECUTE FUNCTION reject_before_insert();").execute(&pool).await.unwrap();
    let pid = backend_pid(&pool, &application).await;
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    assert!(matches!(
        ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await,
        Err(LedgerError::Invariant(_))
    ));
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    assert_eq!(ledger.statements_total(), 3);
    sqlx::query("DROP TRIGGER reject_before_insert ON accounting_state")
        .execute(&pool)
        .await
        .unwrap();
    recover(&ledger, &single, &venue).await;
    let pid = backend_pid(&pool, &application).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE accounting_state IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let counted = ledger.statements_total();
    let worker = ledger.clone();
    let task = tokio::spawn(async move {
        worker
            .ensure_accounting_row(&VenueId::new(SPOT), MonthlyBudgetModel::HwmV1)
            .await
    });
    assert_eq!(wait_for_lock(&pool, "INSERT INTO accounting_state").await, pid);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    assert_eq!(ledger.statements_total() - counted, 3);
    held.rollback().await.unwrap();
    recover(&ledger, &single, &venue).await;
    let pid = backend_pid(&pool, &application).await;
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let counted = ledger.statements_total();
    proxy.set_mode(ProxyMode::DropCommitAckOf("accounting_state"));
    // A fresh venue makes the lost-answer write visibly durable.
    let other = VenueId::new("test:lost-answer");
    assert!(matches!(
        ledger.ensure_accounting_row(&other, MonthlyBudgetModel::HwmV1).await,
        Err(LedgerError::Uncertain(_))
    ));
    retired(&pool, pid).await;
    wire(&proxy, before, 5);
    assert_eq!(ledger.statements_total() - counted, 5);
    assert_eq!(version(&pool, tenant, other.as_str()).await, 0);
    recover(&ledger, &single, &other).await;
}

async fn ready_backend(pool: &PgPool) -> (DbProxy, Arc<PgJournalBackend>, String) {
    let (proxy, initial, application) = isolated_pool(pool).await;
    let backend = Arc::new(PgJournalBackend::new(&initial, Uuid::now_v7()));
    initial.close().await;
    backend.check_ready().await.unwrap();
    (proxy, backend, application)
}

async fn no_temporary_tables(pool: &PgPool) {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relname LIKE 'robson_expected_%'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn flagged_readiness_errors_count_through_completed_retirement(pool: PgPool) {
    let (proxy, backend, _application) = ready_backend(&pool).await;
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    proxy.set_mode(ProxyMode::DropRollbackAckOf("robson_expected_"));
    let (pid, result) = captured_schema_check(&pool, &backend).await;
    assert!(result.is_err());
    retired(&pool, pid).await;
    wire(&proxy, before, 7);
    assert_eq!(proxy.query_trace().iter().filter(|sql| *sql == "ROLLBACK").count(), 1);
    no_temporary_tables(&pool).await;
    backend.check_balance_schema(SPOT, true).await.unwrap();
    sqlx::raw_sql(r#"CREATE FUNCTION refuse_expected_index() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN
        IF EXISTS(SELECT 1 FROM pg_event_trigger_ddl_commands() d JOIN pg_class c ON c.oid=d.objid WHERE c.relname LIKE 'robson_expected_idx_%') THEN RAISE EXCEPTION 'expected-index refusal'; END IF;
        END $$; CREATE EVENT TRIGGER refuse_expected_index ON ddl_command_end WHEN TAG IN ('CREATE INDEX') EXECUTE FUNCTION refuse_expected_index();"#).execute(&pool).await.unwrap();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let (pid, result) = captured_schema_check(&pool, &backend).await;
    assert!(result.is_err());
    retired(&pool, pid).await;
    wire(&proxy, before, 5);
    no_temporary_tables(&pool).await;
    sqlx::query("DROP EVENT TRIGGER refuse_expected_index")
        .execute(&pool)
        .await
        .unwrap();
    backend.check_balance_schema(SPOT, true).await.unwrap();
    // Keep the index present so catalog expression deparsing takes the
    // observed relation lock; make a required table unavailable instead.
    sqlx::query("ALTER TABLE venue_orders RENAME TO unavailable_venue_orders")
        .execute(&pool)
        .await
        .unwrap();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let (pid, result) = captured_schema_check(&pool, &backend).await;
    assert!(result.is_err());
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    no_temporary_tables(&pool).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn flagged_readiness_cancellation_counts_through_completed_retirement(pool: PgPool) {
    let (proxy, backend, _application) = ready_backend(&pool).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let worker = backend.clone();
    let task = tokio::spawn(async move { worker.check_balance_schema(SPOT, true).await });
    let pid = wait_for_lock(&pool, "WITH needed").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    no_temporary_tables(&pool).await;
    held.rollback().await.unwrap();
    backend.check_balance_schema(SPOT, true).await.unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_journal_known_error_and_cancellation_count_through_retirement(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let (proxy, initial, _application) = isolated_pool(&pool).await;
    let backend = Arc::new(PgJournalBackend::new(&initial, tenant));
    initial.close().await;
    backend.check_ready().await.unwrap();
    let batch = balance_batch("cancelled-journal", IntentKind::Exit);
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let (pid, result) = captured_commit(&pool, &backend, &batch).await;
    assert!(matches!(result, Err(CommitFailure::Known(_))));
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    seed_account(&pool, tenant, SPOT).await;
    backend.check_ready().await.unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT * FROM accounting_state WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    let worker = backend.clone();
    let pending = batch.clone();
    let task = tokio::spawn(async move { worker.commit(&pending).await });
    let pid = wait_for_lock(&pool, "UPDATE accounting_state").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_intents")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    held.rollback().await.unwrap();
    backend.commit(&batch).await.unwrap();
    assert_eq!(version(&pool, tenant, SPOT).await, 1);
    wire(&proxy, before, 10);
}

// The journal owns a separate max_connections=2 pool. Capture the actual
// transaction while it waits, never an arbitrary idle sibling by application.
async fn captured_commit(
    pool: &PgPool,
    backend: &Arc<PgJournalBackend>,
    batch: &JournalBatch,
) -> (i32, Result<(), CommitFailure>) {
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE accounting_state IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let worker = backend.clone();
    let pending = batch.clone();
    let task = tokio::spawn(async move { worker.commit(&pending).await });
    let pid = wait_for_lock(pool, "UPDATE accounting_state").await;
    held.rollback().await.unwrap();
    (pid, task.await.unwrap())
}

async fn captured_schema_check(
    pool: &PgPool,
    backend: &Arc<PgJournalBackend>,
) -> (i32, Result<(), String>) {
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let worker = backend.clone();
    let task = tokio::spawn(async move { worker.check_balance_schema(SPOT, true).await });
    let pid = wait_for_lock(pool, "WITH needed").await;
    held.rollback().await.unwrap();
    (pid, task.await.unwrap())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn journal_retirement_tracks_the_active_backend_with_an_idle_sibling(pool: PgPool) {
    let (proxy, backend, application) = ready_backend(&pool).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE submit_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let worker = backend.clone();
        tasks.push(tokio::spawn(async move { worker.check_balance_schema(SPOT, true).await }));
    }
    let pids: Vec<i32> = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let pids: Vec<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND wait_event_type='Lock' AND query LIKE '%WITH needed%'").bind(&application).fetch_all(&pool).await.unwrap();
            if pids.len() == 2 { break pids; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("both journal connections must be observed in flight");
    held.rollback().await.unwrap();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let before = proxy.stats().statements;
    proxy.enable_query_trace(16);
    let (pid, result) =
        captured_commit(&pool, &backend, &balance_batch("two-backends", IntentKind::Exit)).await;
    assert!(matches!(result, Err(CommitFailure::Known(_))));
    assert!(pids.contains(&pid));
    retired(&pool, pid).await;
    wire(&proxy, before, 3);
    let sibling = *pids.iter().find(|other| **other != pid).unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND application_name=$2)"
        )
        .bind(sibling)
        .bind(&application)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "retiring a failed operation must preserve the healthy sibling"
    );
}
