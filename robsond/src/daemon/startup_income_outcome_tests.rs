// The actual poll deliberately swallows recalibration errors. Its private
// outcome witness must distinguish that path from a successful policy refusal.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn income_poll_records_real_refusal_and_swallowed_month_error(pool: sqlx::PgPool) {
    use crate::income_ledger::{IncomeLedgerWorker, IncomeTestOutcome};
    for broken_month in [false, true] {
        let (daemon, worker, _, _) =
            prepared_scan(&pool, "net_from_start_non_expanding_v1", true).await;
        assert_eq!(worker.scan_and_reconcile().await.unwrap(), 1);
        daemon.exchange.set_futures_balance(dec!(9899.89)); // real close net=-0.11, independent residual=-100
        if broken_month {
            sqlx::query("ALTER TABLE monthly_state RENAME TO test_income_hidden_month")
                .execute(&pool)
                .await
                .unwrap();
        }
        // No transfer record: the healthy case must reach the actual unmatched
        // evidence refusal; the broken case must expose the swallowed error.
        let producer = IncomeLedgerWorker::new(
            daemon.exchange.clone(),
            daemon.position_manager.clone(),
            daemon.store.clone(),
            pool.clone(),
            daemon.event_bus.clone(),
            Duration::from_secs(60),
            CancellationToken::new(),
        );
        let outcome = producer.test_outcome();
        let poll = producer.poll_and_match().await;
        if broken_month {
            sqlx::query("ALTER TABLE test_income_hidden_month RENAME TO monthly_state")
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(poll.is_ok(), "this control exercises the real swallowed-error boundary");
        assert_eq!(
            *outcome.lock().unwrap(),
            Some(if broken_month {
                IncomeTestOutcome::Error
            } else {
                IncomeTestOutcome::Unexplained
            })
        );
    }
}
