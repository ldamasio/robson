use super::*;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn ensure_accounting_row_is_idempotent_and_keeps_the_stored_model(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let venue = VenueId::new(SPOT);
    let (proxy, proxied) = through_proxy(&pool).await;
    let ledger = PgVenueLedger::new(&proxied, tenant);
    let before = proxy.stats().statements;
    let first = ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    let replay = ledger
        .ensure_accounting_row(&venue, MonthlyBudgetModel::NetFromStartNonExpandingV1)
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(first.version, 0);
    assert_eq!(first.budget_model, MonthlyBudgetModel::HwmV1);
    assert_eq!(ledger.statements_total(), 10);
    assert_eq!(proxy.stats().statements - before, 10);
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_route_is_written_once(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let other = Uuid::now_v7();
    let position = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    seed_account(&pool, other, SPOT).await;
    seed_account(&pool, tenant, "test:other").await;
    let ledger = PgVenueLedger::new(&pool, tenant);
    let venue = VenueId::new(SPOT);
    let at = Utc::now();
    ledger.insert_route(position, &venue, "operated", at).await.unwrap();
    ledger
        .insert_route(position, &venue, "operated", at + chrono::Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        ledger.insert_route(position, &venue, "other", at).await,
        Err(LedgerError::RouteConflict)
    );
    assert_eq!(
        ledger.insert_route(position, &VenueId::new("test:other"), "operated", at).await,
        Err(LedgerError::RouteConflict)
    );
    assert_eq!(
        PgVenueLedger::new(&pool, other)
            .insert_route(position, &venue, "operated", at)
            .await,
        Err(LedgerError::RouteConflict)
    );
    let row = sqlx::query(
        "SELECT tenant_id,venue_id,account_scope,routed_at FROM leg_routes WHERE position_id=$1",
    )
    .bind(position)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<Uuid, _>("tenant_id"), tenant);
    assert_eq!(row.get::<String, _>("venue_id"), SPOT);
    assert_eq!(row.get::<String, _>("account_scope"), "operated");
    assert_eq!(
        row.get::<DateTime<Utc>, _>("routed_at").timestamp_micros(),
        at.timestamp_micros()
    );
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    assert_eq!(version(&pool, other, SPOT).await, 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_rejected_ledger_commit_is_known_and_a_lost_answer_is_uncertain(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let venue = VenueId::new(SPOT);
    sqlx::raw_sql("CREATE FUNCTION reject_ledger_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'deferred refusal' USING ERRCODE='23514'; END $$; CREATE CONSTRAINT TRIGGER reject_ledger_commit AFTER INSERT ON accounting_state DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_ledger_commit();").execute(&pool).await.unwrap();
    let ledger = PgVenueLedger::new(&pool, tenant);
    assert!(matches!(
        ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await,
        Err(LedgerError::Backend(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounting_state")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    for code in ["40003", "08007", "ZX999"] {
        sqlx::raw_sql(&format!("CREATE OR REPLACE FUNCTION reject_ledger_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'unknown commit resolution' USING ERRCODE='{code}'; END $$"))
            .execute(&pool).await.unwrap();
        let result = ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await;
        assert!(matches!(result, Err(LedgerError::Uncertain(_))), "{code}: {result:?}");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounting_state")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
    sqlx::query("DROP TRIGGER reject_ledger_commit ON accounting_state")
        .execute(&pool)
        .await
        .unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let ledger = PgVenueLedger::new(&proxied, tenant);
    proxy.set_mode(ProxyMode::DropCommitAckOf("accounting_state"));
    assert!(matches!(
        ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await,
        Err(LedgerError::Uncertain(_))
    ));
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounting_state")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_ledger_deadline_retires_an_unanswered_connection(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let tenant = Uuid::now_v7();
    let ledger = PgVenueLedger::new(&proxied, tenant);
    let venue = VenueId::new(SPOT);
    proxy.set_mode(ProxyMode::Blackhole);
    let started = std::time::Instant::now();
    let result = ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await;
    assert!(matches!(result, Err(LedgerError::Backend(_))));
    assert!(started.elapsed() < Duration::from_millis(2500));
    proxy.set_mode(ProxyMode::Pass);
    ledger.ensure_accounting_row(&venue, MonthlyBudgetModel::HwmV1).await.unwrap();
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn concurrent_preparation_keeps_one_model_and_one_route(pool: PgPool) {
    let mut runner = TestRunner::default();
    let schedules = proptest::collection::vec(0u64..12, 2..7);
    for _ in 0..16 {
        let delays = schedules.new_tree(&mut runner).unwrap().current();
        let tenant = Uuid::now_v7();
        let other = Uuid::now_v7();
        let venue = VenueId::new(SPOT);
        let ledger = Arc::new(PgVenueLedger::new(&pool, tenant));
        let mut tasks = Vec::new();
        for (index, delay) in delays.iter().copied().enumerate() {
            let ledger = ledger.clone();
            let venue = venue.clone();
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                ledger
                    .ensure_accounting_row(
                        &venue,
                        if index % 2 == 0 {
                            MonthlyBudgetModel::HwmV1
                        } else {
                            MonthlyBudgetModel::NetFromStartNonExpandingV1
                        },
                    )
                    .await
                    .unwrap()
            }));
        }
        let mut rows = Vec::new();
        for task in tasks {
            rows.push(task.await.unwrap());
        }
        assert!(rows.iter().all(|row| row == &rows[0]));
        assert_eq!(rows[0].version, 0);
        assert_eq!(ledger.statements_total(), 5 * delays.len() as u64);
        seed_account(&pool, other, SPOT).await;
        let position = Uuid::now_v7();
        let mut tasks = Vec::new();
        for (index, delay) in delays.into_iter().enumerate() {
            let who = if index % 2 == 0 { tenant } else { other };
            let ledger = PgVenueLedger::new(&pool, who);
            let venue = venue.clone();
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                (who, ledger.insert_route(position, &venue, "operated", Utc::now()).await)
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        let owner: Uuid =
            sqlx::query_scalar("SELECT tenant_id FROM leg_routes WHERE position_id=$1")
                .bind(position)
                .fetch_one(&pool)
                .await
                .unwrap();
        for (who, result) in results {
            assert_eq!(
                result,
                if who == owner {
                    Ok(())
                } else {
                    Err(LedgerError::RouteConflict)
                }
            );
        }
        assert_eq!(version(&pool, tenant, SPOT).await, 0);
        assert_eq!(version(&pool, other, SPOT).await, 0);
    }
}
