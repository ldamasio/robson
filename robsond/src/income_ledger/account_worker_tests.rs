use robson_exec::venue::AccountKind;

use super::*;
use crate::{
    capability::worker_venue::{
        disconnected_pool, immediately, manager, NoAccessStore, WorkerVenue,
    },
    error::DaemonError,
};

#[tokio::test]
async fn balance_income_direct_run_and_poll_refuse_before_timer_store_or_venue() {
    let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
    let store = Arc::new(NoAccessStore);
    let manager = manager(&exchange, store.clone());
    let pool = disconnected_pool();
    pool.close().await;
    let worker = IncomeLedgerWorker::new(
        exchange.clone(),
        manager.clone(),
        store,
        pool,
        Arc::new(EventBus::new(8)),
        Duration::ZERO,
        CancellationToken::new(),
    );
    let held = manager.write().await;
    let before = held.configured_capital();
    assert!(matches!(immediately(worker.poll_and_match()).await,
        Err(DaemonError::Config(message)) if message == "income ledger requires a position account"));
    // A zero interval would panic if run constructed its timer before denial.
    assert!(matches!(immediately(worker.run()).await,
        Err(DaemonError::Config(message)) if message == "income ledger requires a position account"));
    assert_eq!(held.configured_capital(), before);
    assert_eq!(exchange.requests(), 0);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_income_direct_refusal_sends_zero_physical_sql(pool: PgPool) {
    let (proxy, proxied) = crate::capability::worker_venue::counted_pool(&pool).await;
    let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
    let store = Arc::new(NoAccessStore);
    let manager = manager(&exchange, store.clone());
    let worker = IncomeLedgerWorker::new(
        exchange.clone(),
        manager.clone(),
        store,
        proxied.clone(),
        Arc::new(EventBus::new(8)),
        Duration::ZERO,
        CancellationToken::new(),
    );
    let held = manager.write().await;
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    assert!(matches!(immediately(worker.poll_and_match()).await,
        Err(DaemonError::Config(message)) if message == "income ledger requires a position account"));
    assert!(matches!(immediately(worker.run()).await,
        Err(DaemonError::Config(message)) if message == "income ledger requires a position account"));
    let ready = proxied.acquire().await.unwrap();
    assert_eq!(proxy.stats().statements - before, 0);
    assert!(proxy.query_trace().is_empty());
    assert_eq!(exchange.requests(), 0);
    drop(ready);
    drop(held);
    proxied.close().await;
}
