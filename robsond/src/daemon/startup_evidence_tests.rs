// Phase 1 must finish for the entire observed batch before any write.
#[tokio::test]
async fn test_startup_auto_reconcile_phase1_all_or_nothing() {
    use robson_domain::Symbol;
    for evidence_first in [true, false] {
        let daemon = Daemon::new_stub(config_with_auto_reconcile());
        for (symbol, order) in [("BTCUSDT", "EX-ORDER-A"), ("ETHUSDT", "EX-ORDER-B")] {
            let mut position = active_position(Symbol::from_pair(symbol).unwrap(), Side::Long);
            attach_insurance_order(&daemon.store, &mut position, order).await;
            daemon.store.positions().save(&position).await.unwrap();
        }
        // Use the repository's actual order, not assumed HashMap insertion order.
        let observed = daemon.store.positions().find_active().await.unwrap();
        let selected = &observed[usize::from(!evidence_first)];
        let order = selected.insurance_stop_id.as_ref().unwrap();
        daemon.exchange.set_order_result(
            order,
            order_result(order, dec!(90), dec!(0.010), chrono::Utc::now()),
        );
        let result = daemon.run_startup_auto_reconcile().await;
        assert!(
            matches!(result, Err(DaemonError::StartupStaleActiveDetected { count: 2, ref positions }) if positions.len() == 2),
            "startup_batch_missing_evidence_must_abort_before_phase2: {result:?}"
        );
        for position in observed {
            let stored = daemon.store.positions().find_by_id(position.id).await.unwrap().unwrap();
            assert!(
                matches!(stored.state, PositionState::Active { .. }),
                "startup_partial_close_before_phase1_completed"
            );
            let events = daemon.store.events().find_by_position(position.id).await.unwrap();
            assert!(!events
                .iter()
                .any(|e| matches!(e, robson_domain::Event::PositionClosed { .. })));
        }
    }
}

#[tokio::test]
async fn test_startup_auto_reconcile_ambiguous_trades_abort_without_close() {
    use robson_domain::Symbol;
    let daemon = Daemon::new_stub(config_with_auto_reconcile());
    let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    let position = active_position(symbol.clone(), Side::Long);
    let id = position.id;
    daemon.store.positions().save(&position).await.unwrap();
    let now = chrono::Utc::now();
    daemon.exchange.set_user_trades(&symbol.as_pair(), vec![
        user_trade("TRADE-A", "ORDER-A", dec!(90), dec!(0.010), now),
        user_trade("TRADE-B", "ORDER-B", dec!(91), dec!(0.010), now),
    ]);
    let result = daemon.run_startup_auto_reconcile().await;
    assert!(
        matches!(result, Err(DaemonError::StartupStaleActiveDetected { count: 1, .. })),
        "ambiguous_real_evidence_must_abort: {result:?}"
    );
    assert!(matches!(
        daemon.store.positions().find_by_id(id).await.unwrap().unwrap().state,
        PositionState::Active { .. }
    ));
}

// Private actual producer/PG controls share the original scan_pg namespace.
#[cfg(feature = "postgres")]
mod scan_pg {
    use std::{sync::Arc, time::Duration};

    use robson_exec::StubExchange;
    use robson_store::MemoryStore;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::*;
    use crate::reconciliation_worker::{ReconciliationWorker, ScanTestPhase};
    include!("startup_scan_wire_tests.rs");
    include!("startup_scan_pg_fixture.rs");
    include!("startup_scan_pg_tests.rs");
    include!("startup_income_outcome_tests.rs");
}
