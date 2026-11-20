use robson_exec::venue::AccountKind;

use super::*;
use crate::capability::worker_venue::{immediately, WorkerVenue};

fn daemon(venue: Arc<WorkerVenue>) -> Daemon<WorkerVenue, MemoryStore> {
    Daemon::with_venue(
        Config::test(),
        venue,
        Arc::new(StubTradeFeed::new()),
        Arc::new(robson_exec::StubOhlcv::default()),
    )
}

#[tokio::test]
async fn balance_funding_route_precedes_selected_and_recovery_surfaces() {
    for kind in [AccountKind::BalanceAccount, AccountKind::PositionAccount] {
        for funding in [false, true] {
            for recovery in [false, true] {
                let selected = Arc::new(WorkerVenue::new(kind, funding));
                let old = Arc::new(WorkerVenue::new(AccountKind::PositionAccount, true));
                let daemon = daemon(selected.clone())
                    .with_funding_recovery_route(recovery.then(|| old.clone()));
                let (route, mode) = daemon.funding_worker_route();
                let expected = if kind == AccountKind::BalanceAccount {
                    FundingRouteMode::Blocked
                } else if funding {
                    FundingRouteMode::Selected
                } else if recovery {
                    FundingRouteMode::Recovery
                } else {
                    FundingRouteMode::Blocked
                };
                assert_eq!(mode, expected);
                assert!(Arc::ptr_eq(
                    &route,
                    if expected == FundingRouteMode::Recovery {
                        &old
                    } else {
                        &selected
                    }
                ));
                if kind == AccountKind::BalanceAccount {
                    assert_eq!(selected.funding_accesses(), 0);
                    assert_eq!(old.funding_accesses(), 0);
                }
                assert_eq!(selected.requests() + old.requests(), 0);
            }
        }
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn balance_income_constructs_no_worker_even_with_a_configured_pool() {
    use crate::capability::worker_venue::disconnected_pool;
    let pool = Arc::new(disconnected_pool());
    pool.close().await;
    for kind in [AccountKind::BalanceAccount, AccountKind::PositionAccount] {
        let venue = Arc::new(WorkerVenue::new(kind, true));
        let mut daemon = daemon(venue.clone());
        daemon.pg_pool = Some(pool.clone());
        assert_eq!(
            daemon
                .income_ledger_worker(tokio_util::sync::CancellationToken::new())
                .is_some(),
            kind == AccountKind::PositionAccount
        );
        assert_eq!(venue.requests(), 0);
    }
}

#[tokio::test]
async fn daemon_capital_initialization_refuses_balance_before_read_or_manager_lock() {
    let venue = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, true));
    let daemon = daemon(venue.clone());
    let held = daemon.position_manager.write().await;
    let before = held.configured_capital();
    immediately(daemon.initialize_capital()).await;
    assert_eq!(held.configured_capital(), before);
    assert_eq!(venue.requests(), 0);
    drop(held);

    let venue = Arc::new(WorkerVenue::new(AccountKind::PositionAccount, true));
    let daemon = self::daemon(venue.clone());
    daemon.initialize_capital().await;
    assert_eq!(venue.requests(), 1);
    assert_eq!(daemon.position_manager.read().await.configured_capital(), dec!(12000));
    venue.delegate.set_futures_balance(Decimal::ZERO);
    daemon.initialize_capital().await;
    assert_eq!(venue.requests(), 2);
    assert_eq!(daemon.position_manager.read().await.configured_capital(), dec!(12000));
}
