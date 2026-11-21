use super::*;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_spot_commit_takes_the_accounting_row_first(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2 FOR UPDATE",
    )
    .bind(tenant)
    .bind(SPOT)
    .fetch_one(&mut *held)
    .await
    .unwrap();
    let backend = Arc::new(PgJournalBackend::new(&pool, tenant));
    let writer = backend.clone();
    let task =
        tokio::spawn(
            async move { writer.commit(&balance_batch("blocked", IntentKind::Exit)).await },
        );
    let pid = wait_for_lock(&pool, "UPDATE accounting_state").await;
    let locked:Vec<String>=sqlx::query_scalar("SELECT c.relname FROM pg_locks l JOIN pg_class c ON c.oid=l.relation WHERE l.pid=$1 AND c.relname=ANY($2)")
        .bind(pid).bind(vec!["order_intents","submit_attempts","spot_intent_clock","balance_legs","balance_leg_admissions"]).fetch_all(&pool).await.unwrap();
    assert!(locked.is_empty(), "journal/ledger locks before accounting: {locked:?}");
    held.rollback().await.unwrap();
    task.await.unwrap().unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_exhausted_commit_budget_abandons_before_configuration_or_writes(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, tenant);
    backend.check_ready().await.unwrap();
    proxy.enable_query_trace(32);
    let mut batch = balance_batch("exhausted", IntentKind::Exit);
    batch.balance.as_mut().unwrap().commit_bound = Duration::from_millis(110);
    assert!(matches!(backend.commit(&batch).await, Err(CommitFailure::Known(_))));
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    let trace = proxy.query_trace();
    assert!(trace.iter().any(|q| q == "BEGIN"));
    assert!(
        !trace.iter().any(|q| q.contains("set_config")
            || q.contains("accounting_state")
            || q.contains("order_intents")),
        "{trace:?}"
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn delayed_acquisition_reduces_the_commit_budget_and_stays_known(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let backend = Arc::new(PgJournalBackend::new(&pool, tenant));
    backend.check_ready().await.unwrap();
    let a = JournalBatch {
        balance: None,
        intent: intent_row("hold-a"),
        attempts: vec![],
        outcome: None,
    };
    let b = JournalBatch {
        balance: None,
        intent: intent_row("hold-b"),
        attempts: vec![],
        outcome: None,
    };
    backend.commit(&a).await.unwrap();
    backend.commit(&b).await.unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT intent_id FROM order_intents WHERE client_order_id IN ('hold-a','hold-b') FOR UPDATE").fetch_all(&mut *held).await.unwrap();
    let mut account = pool.begin().await.unwrap();
    sqlx::query("SELECT version FROM accounting_state WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .fetch_one(&mut *account)
        .await
        .unwrap();
    let ba = backend.clone();
    let first = tokio::spawn(async move { ba.commit(&a).await });
    let bb = backend.clone();
    let second = tokio::spawn(async move { bb.commit(&b).await });
    tokio::time::timeout(Duration::from_secs(3),async {loop {
        let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%INSERT INTO order_intents%'").fetch_one(&pool).await.unwrap();
        if n==2 {break;} tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.unwrap();
    let journal = Arc::new(
        OrderJournal::new(backend, &VenueId::new(SPOT))
            .with_balance_account(true)
            .with_one_attempt_per_client_id(true),
    );
    let op = journal.begin_operation();
    let intent = op.scope(async { journal.open_intent(spec("delayed", IntentKind::Exit)) }).await;
    let release = async {
        tokio::time::sleep(Duration::from_millis(240)).await;
        held.rollback().await.unwrap();
    };
    let (result, ()) =
        tokio::join!(op.scope(journal.before_send(&intent, WriteClass::Strict)), release);
    assert!(matches!(result, Err(SendRefusal::NotSent(_))), "{result:?}");
    assert!(!journal.latch_set());
    assert!(journal.gate().open_incidents().is_empty());
    account.rollback().await.unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn two_lock_waits_still_reach_the_outer_uncertain_deadline(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let batch = balance_batch("two-waits", IntentKind::Exit);
    let backend = Arc::new(PgJournalBackend::new(&pool, tenant));
    // The intent already exists. The balance attempt waits on accounting,
    // then on the intent row held by a release-like transaction.
    let mut seeded = batch.clone();
    seeded.attempts.clear();
    backend.commit(&seeded).await.unwrap();
    let mut account = pool.begin().await.unwrap();
    sqlx::query("SELECT version FROM accounting_state WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .fetch_one(&mut *account)
        .await
        .unwrap();
    let mut intent_lock = pool.begin().await.unwrap();
    sqlx::query("SELECT intent_id FROM order_intents WHERE intent_id=$1 FOR UPDATE")
        .bind(batch.intent.intent_id)
        .fetch_one(&mut *intent_lock)
        .await
        .unwrap();
    let future = tokio::time::timeout(BOUND, backend.commit(&batch));
    let release_account = async {
        tokio::time::sleep(Duration::from_millis(220)).await;
        account.rollback().await.unwrap();
    };
    let (result, ()) = tokio::join!(future, release_account);
    assert!(
        result.is_err(),
        "the outer bound, not a promise of Known, ends this sequence: {result:?}"
    );
    intent_lock.rollback().await.unwrap();
    // Both database waits together exceed J's deadline; its public classifier
    // is covered by the same two waits below through the real journal.
    let journal = journal_on(&pool, tenant, true, true, SPOT);
    let op = journal.begin_operation();
    let intent = op
        .scope(async { journal.open_intent(spec("journal-two-waits", IntentKind::Exit)) })
        .await;
    sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,$3,$4,$5,'exit','{}',NOW())")
        .bind(intent.intent_id()).bind(tenant).bind(SPOT).bind(DEFAULT_ACCOUNT_SCOPE).bind(intent.client_order_id()).execute(&pool).await.unwrap();
    let mut account = pool.begin().await.unwrap();
    sqlx::query("SELECT version FROM accounting_state WHERE tenant_id=$1 FOR UPDATE")
        .bind(tenant)
        .fetch_one(&mut *account)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT intent_id FROM order_intents WHERE intent_id=$1 FOR UPDATE")
        .bind(intent.intent_id())
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let release = async {
        tokio::time::sleep(Duration::from_millis(220)).await;
        account.rollback().await.unwrap();
    };
    let (result, ()) =
        tokio::join!(op.scope(journal.before_send(&intent, WriteClass::Strict)), release);
    assert!(matches!(result, Err(SendRefusal::UncertainCommit(_))), "{result:?}");
    held.rollback().await.unwrap();
    op.scope(journal.finish_failed(&intent)).await;
    assert!(journal.gate().fenced());
    assert!(journal
        .gate()
        .open_incidents()
        .iter()
        .any(|incident| incident.reason == robson_exec::gate::IncidentReason::UncertainOutcome));
}
