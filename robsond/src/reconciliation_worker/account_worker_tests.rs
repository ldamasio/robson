use robson_exec::venue::AccountKind;

use super::*;
use crate::capability::worker_venue::{immediately, manager, NoAccessStore, WorkerVenue};

#[tokio::test]
async fn balance_drift_skips_reads_and_recalibration_without_certifying_reconciliation() {
    let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
    let store = Arc::new(NoAccessStore);
    let manager = manager(&exchange, store.clone());
    let worker = ReconciliationWorker::new(
        exchange.clone(),
        manager.clone(),
        store,
        Arc::new(EventBus::new(8)),
        Duration::from_secs(60),
        CancellationToken::new(),
    );
    let held = manager.write().await;
    let before = held.configured_capital();
    immediately(worker.recalibrate_capital_base_after_pure_financial_drift())
        .await
        .unwrap();
    immediately(worker.recalibrate_capital_base_after_manual_drift(1))
        .await
        .unwrap();
    immediately(worker.recalibrate_capital_base("fixture-direct".into(), None))
        .await
        .unwrap();
    assert_eq!(exchange.requests(), 0);
    assert_eq!(held.configured_capital(), before);
    drop(held);

    // Real balance venues have no position port. Disabling only the two
    // legacy drift branches must not fabricate successful reconciliation.
    let exchange =
        Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true).without_position_account());
    let store = Arc::new(NoAccessStore);
    let manager = self::manager(&exchange, store.clone());
    let worker = ReconciliationWorker::new(
        exchange,
        manager,
        store,
        Arc::new(EventBus::new(8)),
        Duration::from_secs(60),
        CancellationToken::new(),
    );
    assert!(
        matches!(worker.scan_and_reconcile_inner().await, Err(DaemonError::Exec(ExecError::Config(message))) if message.contains("has no position account"))
    );
}
