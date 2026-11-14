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
    let (proxy, initial, application) = isolated_pool(&pool).await;
    let backend = PgJournalBackend::new(&initial, tenant);
    initial.close().await;
    backend.check_ready().await.unwrap();
    let pid = backend_pid(&pool, &application).await;
    proxy.enable_query_trace(32);
    let before = proxy.stats().statements;
    let batch = balance_batch("rejected-commit", IntentKind::Exit);
    assert_eq!(batch.statements(), 7);
    // J keeps its established conservative COMMIT classification.
    assert!(matches!(backend.commit(&batch).await, Err(CommitFailure::Uncertain(_))));
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
