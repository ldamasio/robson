use robson_exec::venue::AccountKind;

use super::*;
use crate::capability::worker_venue::{disconnected_pool, immediately, WorkerVenue};

#[tokio::test]
async fn balance_funding_service_direct_boundaries_refuse_all_configured_modes() {
    let pool = disconnected_pool();
    pool.close().await;
    for route in [
        FundingRouteMode::Selected,
        FundingRouteMode::Recovery,
        FundingRouteMode::Blocked,
    ] {
        let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
        let harness = test_service(pool.clone(), exchange.clone(), FundingConfig::default());
        let service = harness.service.with_route_mode(route);
        let held = harness.position_manager.write().await;
        let before = held.configured_capital();
        assert!(matches!(immediately(service.quote()).await,
            Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
        assert!(matches!(immediately(service.execute(Uuid::now_v7(), "fixture")).await,
            Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
        assert!(matches!(immediately(service.refresh_capital()).await,
            Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
        assert_eq!(held.configured_capital(), before);
        assert_eq!(exchange.requests(), 0);
        assert_eq!(exchange.funding_accesses(), 0);
    }
}

async fn funding_snapshot(pool: &PgPool, tenant: Uuid) -> Vec<serde_json::Value> {
    let mut snapshot = Vec::new();
    for table in [
        "funding_sagas",
        "event_log",
        "event_idempotency",
        "stream_state",
    ] {
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(r) FROM {table} r WHERE tenant_id=$1 ORDER BY to_jsonb(r)::text"
        ))
        .bind(tenant)
        .fetch_all(pool)
        .await
        .unwrap();
        snapshot.push(serde_json::json!({"table": table, "rows": rows}));
    }
    snapshot
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_funding_preserves_liabilities_with_bounded_physical_reads(pool: PgPool) {
    use robson_testkit::db_proxy::DbProxy;
    use sqlx::postgres::{PgPoolOptions, PgSslMode};

    use crate::funding::worker::FundingWorker;
    let _gauge = BLOCKED_GAUGE_TEST.lock().await;
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = DbProxy::start(upstream).await.unwrap();
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
    for count in [0, 1, 20] {
        let original = Arc::new(PostConvertedCrashExchange::new(configured_exchange()));
        let seeded = test_service(pool.clone(), original.clone(), FundingConfig::default());
        let expiring = FundingService::new(
            Arc::new(pool.clone()),
            seeded.tenant_id,
            original.clone(),
            seeded.position_manager.clone(),
            FundingConfig {
                quote_ttl_secs: 0,
                ..FundingConfig::default()
            },
        );
        let mut ids = Vec::new();
        for index in 0..count {
            if index % 2 == 1 {
                ids.push(expiring.quote().await.unwrap().quote_id);
            } else {
                let quote = seeded.service.quote().await.unwrap();
                original.fail_next_transfers(1);
                assert!(seeded
                    .service
                    .execute(quote.quote_id, "seed-held-liability")
                    .await
                    .is_err());
                assert_eq!(
                    seeded.service.get(quote.quote_id).await.unwrap().state,
                    FundingState::Transferring.as_str()
                );
                ids.push(quote.quote_id);
            }
        }
        let before_facts = funding_snapshot(&pool, seeded.tenant_id).await;
        for route in [
            FundingRouteMode::Selected,
            FundingRouteMode::Recovery,
            FundingRouteMode::Blocked,
        ] {
            let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
            let manager = test_manager(&exchange);
            let service = || {
                FundingService::new(
                    Arc::new(proxied.clone()),
                    seeded.tenant_id,
                    exchange.clone(),
                    manager.clone(),
                    FundingConfig::default(),
                )
                .with_route_mode(route)
            };
            let direct = service();
            let journal = manager.read().await.order_journal();
            let held = manager.write().await;
            let before_capital = held.configured_capital();
            proxy.enable_query_trace(128);
            let before = proxy.stats().statements;
            assert!(matches!(immediately(direct.quote()).await,
                Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
            for id in ids.iter().copied().chain(std::iter::once(Uuid::now_v7())) {
                assert!(matches!(immediately(direct.execute(id, "denied")).await,
                    Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
            }
            assert!(matches!(immediately(direct.refresh_capital()).await,
                Err(crate::error::DaemonError::Config(message)) if message == "funding requires a position account"));
            assert_eq!(proxy.stats().statements - before, 0);

            let before = proxy.stats().statements;
            assert_eq!(direct.resume_non_terminal().await.unwrap(), 0);
            assert_eq!(proxy.stats().statements - before, 1);
            let worker = FundingWorker::new(service()).with_order_journal(journal);
            let before = proxy.stats().statements;
            worker.poll().await;
            let ready = proxied.acquire().await.unwrap();
            assert_eq!(
                proxy.stats().statements - before,
                2,
                "{count}/{route:?}: {:?}",
                proxy.query_trace()
            );
            drop(ready);
            assert_eq!(crate::metrics::FUNDING_SAGAS_BLOCKED.get(), (count + 1) / 2);

            let before = proxy.stats().statements;
            let listed = direct.list().await.unwrap();
            assert_eq!(proxy.stats().statements - before, 1);
            assert_eq!(listed.len(), count as usize);
            for summary in &listed {
                assert_eq!(
                    summary.blocked_reason.as_deref(),
                    (summary.state != FundingState::Quoted.as_str())
                        .then_some(FUNDING_ROUTE_UNAVAILABLE)
                );
            }
            for id in ids.first().into_iter().chain(ids.last()).copied() {
                let before = proxy.stats().statements;
                let view = direct.get(id).await.unwrap();
                assert_eq!(proxy.stats().statements - before, 2);
                assert_eq!(
                    view.blocked_reason.as_deref(),
                    (view.state != FundingState::Quoted.as_str())
                        .then_some(FUNDING_ROUTE_UNAVAILABLE)
                );
                assert!(!view.events.is_empty());
            }

            if count == 1 && route == FundingRouteMode::Selected {
                // Exercise the real runtime timer/dispatch path too. Cancel
                // after its first request is observed; run finishes both reads
                // before selecting shutdown. The next real tick is five seconds away.
                let shutdown = tokio_util::sync::CancellationToken::new();
                let token = shutdown.clone();
                let before = proxy.stats().statements;
                let running = tokio::spawn(worker.run(token));
                tokio::time::timeout(std::time::Duration::from_secs(8), async {
                    loop {
                        if proxy.stats().statements - before >= 1 {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                })
                .await
                .unwrap();
                shutdown.cancel();
                running.await.unwrap().unwrap();
                let ready = proxied.acquire().await.unwrap();
                assert_eq!(proxy.stats().statements - before, 2);
                drop(ready);
            }
            assert_eq!(proxy.query_trace_dropped(), 0);
            assert_eq!(exchange.requests(), 0);
            assert_eq!(exchange.funding_accesses(), 0);
            assert_eq!(held.configured_capital(), before_capital);
            drop(held);
            assert_eq!(funding_snapshot(&pool, seeded.tenant_id).await, before_facts);
        }
    }
    proxied.close().await;
    // A fresh connection after renaming the table cannot reuse prepared plans
    // for its old name. Both resume and gauge must still be attempted on error.
    sqlx::query("ALTER TABLE funding_sagas RENAME TO funding_sagas_fixture_hidden")
        .execute(&pool)
        .await
        .unwrap();
    let (fault_proxy, fault_pool) = crate::capability::worker_venue::counted_pool(&pool).await;
    let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
    let service = FundingService::new(
        Arc::new(fault_pool.clone()),
        Uuid::now_v7(),
        exchange.clone(),
        test_manager(&exchange),
        FundingConfig::default(),
    );
    let worker = FundingWorker::new(service);
    fault_proxy.enable_query_trace(16);
    let before = fault_proxy.stats().statements;
    let gauge_before = crate::metrics::FUNDING_SAGAS_BLOCKED.get();
    worker.poll().await;
    let ready = fault_pool.acquire().await.unwrap();
    assert_eq!(fault_proxy.stats().statements - before, 2, "{:?}", fault_proxy.query_trace());
    assert_eq!(crate::metrics::FUNDING_SAGAS_BLOCKED.get(), gauge_before);
    assert_eq!(exchange.requests(), 0);
    drop(ready);
    sqlx::query("ALTER TABLE funding_sagas_fixture_hidden RENAME TO funding_sagas")
        .execute(&pool)
        .await
        .unwrap();
    fault_pool.close().await;
}


#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn position_funding_direct_modes_retain_legacy_execution(pool: PgPool) {
    for route in [
        FundingRouteMode::Selected,
        FundingRouteMode::Recovery,
        FundingRouteMode::Blocked,
    ] {
        let exchange = Arc::new(WorkerVenue::new(AccountKind::PositionAccount, true));
        let harness = test_service(pool.clone(), exchange.clone(), FundingConfig::default());
        let service = harness.service.with_route_mode(route);
        let before = harness.position_manager.read().await.configured_capital();
        let quote = service.quote().await.unwrap();
        assert!(!quote.items.is_empty());
        let result = service.execute(quote.quote_id, "position-preservation").await.unwrap();
        assert_eq!(result.state, FundingState::Refreshed.as_str());
        assert!(exchange.requests() > 0);
        assert_eq!(exchange.delegate.transfer_call_count(), 1);
        let capital = service.refresh_capital().await.unwrap();
        assert!(capital > Decimal::ZERO);
        assert_eq!(
            harness.position_manager.read().await.configured_capital(),
            if route == FundingRouteMode::Selected {
                capital
            } else {
                before
            }
        );
    }
}
