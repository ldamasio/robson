// Private regression controls included in the existing worker test module.
async fn governed_close_worker(
    wallet: Decimal,
) -> (
    Arc<ReconciliationWorker<StubExchange, MemoryStore>>,
    Arc<StubExchange>,
    Arc<MemoryStore>,
    PositionId,
) {
    let exchange = Arc::new(StubExchange::new(dec!(100)));
    exchange.set_futures_balance(wallet);
    let store = Arc::new(MemoryStore::new());
    let bus = Arc::new(EventBus::new(32));
    let mut position = tracked_active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
    let id = position.id;
    attach_insurance_order(&store, &mut position, "GOVERNED-STOP").await;
    let mut fill = order_result("GOVERNED-STOP", dec!(90), dec!(0.010), Utc::now());
    fill.client_order_id = format!("ins-{id}");
    exchange.set_order_result("GOVERNED-STOP", fill);
    let worker = Arc::new(create_worker(exchange.clone(), store.clone(), bus, Duration::ZERO));
    assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
    (worker, exchange, store, id)
}

#[tokio::test]
async fn a_governed_close_does_not_author_an_unexplained_basis_adjustment() {
    for wallet in [dec!(9000), dec!(11000)] {
        let (worker, _, store, id) = governed_close_worker(wallet).await;
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);
        let closed = store.positions().find_by_id(id).await.unwrap().unwrap();
        assert!(matches!(closed.state, PositionState::Closed {
            exit_reason: ExitReason::InsuranceStop,
            ..
        }));
        assert_eq!(closed.realized_pnl, dec!(-0.1));
        assert_eq!(closed.fees_paid, dec!(0.01));
        assert_eq!(close_events_for(&store, id).await, 1);
        let adjustments = capital_base_recalibration_events(&store).await;
        assert!(adjustments.is_empty(), "scan_authored_unexplained_adjustment: {adjustments:?}");
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
        assert_eq!(
            close_events_for(&store, id).await,
            1,
            "a checksum must never resubmit the financial close"
        );
    }
}

#[tokio::test]
async fn a_checksum_error_after_close_preserves_the_close_and_next_scan() {
    let (worker, exchange, store, id) = governed_close_worker(dec!(9000)).await;
    let pause = worker.test_pause_at(ScanTestPhase::AfterGuardSnapshot);
    let scan = tokio::spawn({
        let worker = worker.clone();
        async move { worker.scan_and_reconcile().await }
    });
    if tokio::time::timeout(Duration::from_secs(2), pause.reached.notified())
        .await
        .is_err()
    {
        scan.abort();
        let stopped = scan.await;
        panic!("scan did not reach the actual checksum boundary: {stopped:?}");
    }
    let observed_closes_before_checksum = close_events_for(&store, id).await;
    exchange.set_fail_next(true); // The next real exchange call is the wallet read.
    pause.resume.notify_one();
    let result = scan.await.unwrap();
    assert_eq!(observed_closes_before_checksum, 1, "the actual close precedes the checksum");
    assert!(
        matches!(result, Err(DaemonError::Exec(ExecError::Exchange(ref message))) if message == "Simulated futures balance fetch failure"),
        "post_close_balance_error_must_propagate: {result:?}"
    );
    assert!(matches!(
        store.positions().find_by_id(id).await.unwrap().unwrap().state,
        PositionState::Closed { .. }
    ));
    assert!(capital_base_recalibration_events(&store).await.is_empty());
    assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
    assert_eq!(close_events_for(&store, id).await, 1);
    assert_eq!(
        exchange.position_close_call_count(),
        0,
        "recording an observed fill sends no close request"
    );
}

#[tokio::test]
async fn a_reconciled_close_does_not_bypass_open_armed_or_inflight_guards() {
    for variant in 0..3 {
        let (worker, exchange, store, id) = governed_close_worker(dec!(9000)).await;
        let mut other = tracked_active_position(Symbol::from_pair("ETHUSDT").unwrap(), Side::Long);
        match variant {
            0 => exchange.set_open_position(
                other.symbol.clone(),
                other.side,
                other.quantity,
                other.entry_price.unwrap(),
            ),
            1 => other.state = PositionState::Armed,
            _ => {
                other.state = PositionState::Exiting {
                    exit_order_id: Uuid::now_v7(),
                    exit_reason: ExitReason::TrailingStop,
                }
            },
        }
        let other_id = other.id;
        store.positions().save(&other).await.unwrap();
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);
        assert_eq!(close_events_for(&store, id).await, 1);
        assert_eq!(
            serde_json::to_value(
                store.positions().find_by_id(other_id).await.unwrap().unwrap().state
            )
            .unwrap(),
            serde_json::to_value(other.state).unwrap()
        );
        assert!(
            capital_base_recalibration_events(&store).await.is_empty(),
            "guarded_scan_must_not_adjust_basis"
        );
    }
}
