// Included inside the existing worker tests, never compiled into production.
mod financial_counts {
    use super::*;
    include!("financial_count_ports.rs");

    fn fixture() -> (
        Arc<ReconciliationWorker<CountedExchange, CountedStore>>,
        Arc<CountedExchange>,
        Arc<CountedStore>,
    ) {
        fixture_with(|manager| manager)
    }

    fn fixture_with(
        configure: impl FnOnce(
            PositionManager<CountedExchange, CountedStore>,
        ) -> PositionManager<CountedExchange, CountedStore>,
    ) -> (
        Arc<ReconciliationWorker<CountedExchange, CountedStore>>,
        Arc<CountedExchange>,
        Arc<CountedStore>,
    ) {
        let exchange = Arc::new(CountedExchange::new());
        let store = Arc::new(CountedStore::default());
        let bus = Arc::new(EventBus::new(32));
        let executor = Arc::new(Executor::new(
            exchange.clone(),
            Arc::new(IntentJournal::new()),
            store.clone(),
        ));
        let manager = Arc::new(RwLock::new(configure(PositionManager::new(
            Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
            executor,
            store.clone(),
            bus.clone(),
            Arc::new(TracingQueryRecorder),
            TradingPolicy::default(),
        ))));
        let worker = Arc::new(ReconciliationWorker::new_with_missing_grace(
            exchange.clone(),
            manager,
            store.clone(),
            bus,
            Duration::from_secs(60),
            Duration::ZERO,
            CancellationToken::new(),
        ));
        (worker, exchange, store)
    }

    #[tokio::test]
    async fn checksum_guard_skips_actual_wallet_and_both_local_reads_are_counted() {
        for variant in 0..3 {
            let (worker, exchange, store) = fixture();
            let mut position =
                tracked_active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
            match variant {
                0 => exchange.inner.set_open_position(
                    position.symbol.clone(),
                    position.side,
                    position.quantity,
                    position.entry_price.unwrap(),
                ),
                1 => position.state = PositionState::Armed,
                _ => {
                    position.state = PositionState::Exiting {
                        exit_order_id: Uuid::now_v7(),
                        exit_reason: ExitReason::TrailingStop,
                    }
                },
            }
            store.positions().save(&position).await.unwrap();
            exchange.fail_wallet.store(true, Ordering::SeqCst);
            worker.recalibrate_capital_base_after_pure_financial_drift().await.unwrap();
            assert_eq!(exchange.inventory.load(Ordering::SeqCst), 1);
            assert_eq!(
                exchange.wallet.load(Ordering::SeqCst),
                0,
                "guard must skip actual wallet port"
            );
            assert_eq!(store.risk.load(Ordering::SeqCst), 1);
            assert_eq!(store.active.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn actual_local_and_exchange_error_prefixes_remain_distinct() {
        for phase in 0..4 {
            let (worker, exchange, store) = fixture();
            match phase {
                0 => store.fail_risk.store(true, Ordering::SeqCst),
                1 => store.fail_active.store(true, Ordering::SeqCst),
                2 => exchange.fail_wallet.store(true, Ordering::SeqCst),
                _ => exchange.fail_inventory.store(true, Ordering::SeqCst),
            }
            let result = worker.recalibrate_capital_base_after_pure_financial_drift().await;
            assert_eq!(result.is_err(), phase != 3, "inventory retains real local fallback");
            assert_eq!(exchange.inventory.load(Ordering::SeqCst), usize::from(phase != 0));
            assert_eq!(exchange.wallet.load(Ordering::SeqCst), usize::from(phase >= 2));
            assert_eq!(store.risk.load(Ordering::SeqCst), 1);
            assert_eq!(store.active.load(Ordering::SeqCst), usize::from(phase != 0));
        }
    }

    #[tokio::test]
    async fn risk_local_read_really_precedes_the_inventory_await() {
        let (worker, exchange, store) = fixture();
        let pause = worker.position_manager.read().await.test_pause_before_inventory();
        let task = tokio::spawn({
            let worker = worker.clone();
            async move { worker.recalibrate_capital_base_after_pure_financial_drift().await }
        });
        if tokio::time::timeout(Duration::from_secs(2), pause.reached.notified())
            .await
            .is_err()
        {
            task.abort();
            let result = task.await;
            panic!("actual local/inventory boundary not reached: {result:?}");
        }
        let local_before = store.risk.load(Ordering::SeqCst);
        let inventory_before = exchange.inventory.load(Ordering::SeqCst);
        pause.resume.notify_one();
        let result = task.await.unwrap();
        assert_eq!(local_before, 1);
        assert_eq!(inventory_before, 0, "pause must be before the real inventory port await");
        result.unwrap();
        assert_eq!(exchange.inventory.load(Ordering::SeqCst), 1);
    }
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn real_checksum_wire_counts_cover_present_missing_guard_and_error_prefixes(
        pool: sqlx::PgPool,
    ) {
        use robson_testkit::db_proxy::DbProxy;
        use sqlx::postgres::{PgPoolOptions, PgSslMode};
        let now = Utc::now();
        // Parse deltas qualify preparation attempts ONLY in each case's fresh
        // connection/cache. Cached later phases are measured separately.
        // Each case gets its own observed connection/cache. Setup and oracle SQL
        // use only the macro-owned upstream pool, outside measured operation.
        for phase in 0..9 {
            sqlx::query("DELETE FROM monthly_state WHERE year=$1 AND month=$2")
                .bind(now.year() as i16)
                .bind(now.month() as i16)
                .execute(&pool)
                .await
                .unwrap();
            let missing = phase == 1 || phase == 8;
            if !missing {
                sqlx::query("INSERT INTO monthly_state(year,month,capital_base,boundary_reset_at) VALUES($1,$2,10000,NOW())")
                    .bind(now.year() as i16).bind(now.month() as i16).execute(&pool).await.unwrap();
            }
            if phase == 6 {
                sqlx::query("ALTER TABLE monthly_state RENAME TO test_hidden_monthly_state")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            if phase >= 7 {
                sqlx::query(
                    "ALTER TABLE positions_current RENAME TO test_hidden_positions_current",
                )
                .execute(&pool)
                .await
                .unwrap();
            }
            let options = pool.connect_options();
            let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
                .await
                .unwrap()
                .next()
                .unwrap();
            let proxy = DbProxy::start(upstream).await.unwrap();
            let observed = PgPoolOptions::new()
                .max_connections(1)
                .min_connections(1)
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
            let tenant = Uuid::now_v7();
            let (worker, exchange, store) =
                fixture_with(|manager| manager.with_event_log(observed.clone(), tenant));
            match phase {
                2 => {
                    let mut p =
                        tracked_active_position(Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
                    p.state = PositionState::Armed;
                    store.positions().save(&p).await.unwrap();
                },
                3 => store.fail_risk.store(true, Ordering::SeqCst),
                4 => store.fail_active.store(true, Ordering::SeqCst),
                5 => exchange.fail_wallet.store(true, Ordering::SeqCst),
                _ => (),
            }
            let before = proxy.stats();
            let parses_before = proxy.prepared_statement_parses();
            let result = worker.recalibrate_capital_base_after_pure_financial_drift().await;
            let after = proxy.stats();
            let parses_after = proxy.prepared_statement_parses();
            let tail_ports = (
                exchange.inventory.load(Ordering::SeqCst),
                exchange.wallet.load(Ordering::SeqCst),
                store.risk.load(Ordering::SeqCst),
                store.active.load(Ordering::SeqCst),
            );
            let whole = if phase == 0
                && result.is_ok()
                && after.statements - before.statements == 2
                && after.commits == before.commits
            {
                let scan = worker.scan_and_reconcile().await;
                Some((
                    scan,
                    proxy.stats(),
                    proxy.prepared_statement_parses(),
                    worker.test_recent_closed_window(),
                    (
                        exchange.inventory.load(Ordering::SeqCst),
                        exchange.wallet.load(Ordering::SeqCst),
                        store.risk.load(Ordering::SeqCst),
                        store.active.load(Ordering::SeqCst),
                    ),
                ))
            } else {
                None
            };
            // Existing real method accepts a timestamp: exercise both PG month
            // cardinalities directly, without replacing the scan's real clock.
            let mut history_cases = Vec::new();
            if phase == 0 && whole.is_some() {
                for (at, expected) in [("2027-03-02T12:00:00Z", 1u64), ("2027-03-01T01:00:00Z", 2)]
                {
                    let before_history = proxy.stats();
                    let parse_before_history = proxy.prepared_statement_parses();
                    let symbols = worker.recently_closed_symbols(at.parse().unwrap()).await;
                    history_cases.push((
                        expected,
                        symbols,
                        proxy.stats().statements - before_history.statements,
                        proxy.stats().commits - before_history.commits,
                        proxy.prepared_statement_parses() - parse_before_history,
                        worker.test_recent_closed_window(),
                    ));
                }
            }
            observed.close().await;
            let restored = if phase == 6 {
                sqlx::query("ALTER TABLE test_hidden_monthly_state RENAME TO monthly_state")
                    .execute(&pool)
                    .await
                    .map(|_| ())
            } else if phase >= 7 {
                sqlx::query("ALTER TABLE test_hidden_positions_current RENAME TO positions_current")
                    .execute(&pool)
                    .await
                    .map(|_| ())
            } else {
                Ok(())
            };
            eprintln!("SCAN_TAIL_CLEANUP phase={phase} restored={}", restored.is_ok());
            let (expected_executions, expected_parses) = match phase {
                0 => (2, 2),
                1 => (3, 3),
                6 => (0, 1), // First relation missing: Parse fails before Execute.
                7 => (1, 2), // Month executes; net preparation fails.
                8 => (2, 3), // Current/inherited month execute; net Parse fails.
                _ => (0, 0),
            };
            eprintln!("SCAN_FRESH_PROTOCOL phase={phase} query_execute={} parses={} expected_query_execute={expected_executions} expected_parses={expected_parses}",
                after.statements-before.statements, parses_after-parses_before);
            assert_eq!(
                after.statements - before.statements,
                expected_executions,
                "actual Query/Execute messages on this fresh connection phase={phase}"
            );
            assert_eq!(
                parses_after - parses_before,
                expected_parses,
                "actual Parse messages including failed preparation phase={phase}"
            );
            assert_eq!(
                after.commits - before.commits,
                0,
                "checksum never attempts an append transaction"
            );
            assert_eq!(tail_ports.0, usize::from(phase != 3));
            assert_eq!(tail_ports.1, usize::from(![2, 3, 4].contains(&phase)));
            assert_eq!(tail_ports.2, 1);
            assert_eq!(tail_ports.3, usize::from(phase != 3));
            assert_eq!(
                result.is_err(),
                phase >= 3,
                "actual read failure must not become a successful checksum"
            );
            if phase == 6 {
                assert!(
                    matches!(&result, Err(DaemonError::Database(sqlx::Error::Database(error)))
                    if error.code().as_deref() == Some("42P01")),
                    "month read preserves its actual PostgreSQL SQLSTATE: {result:?}"
                );
            } else if phase >= 7 {
                // The existing StoreError conversion retains the actual message
                // but not SQLSTATE. Do not invent a typed code that was lost.
                assert!(
                    matches!(&result, Err(DaemonError::Store(robson_store::StoreError::Database(message)))
                    if message.contains("relation \"positions_current\" does not exist")),
                    "net read preserves the actual mapped missing-relation error: {result:?}"
                );
            }
            let events: i64 =
                sqlx::query_scalar("SELECT count(*) FROM event_log WHERE tenant_id=$1")
                    .bind(tenant)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(events, 0, "no scan-authored PG append");
            assert!(store.events().get_all_events().await.unwrap().is_empty());
            if phase == 0 {
                let (scan, whole, whole_parses, window, ports) =
                    whole.expect("healthy first tail reaches the separate whole-scan control");
                assert_eq!(scan.unwrap(), 0);
                let (at, since, months) = window.expect("real whole scan history must be observed");
                assert_eq!(since, at - chrono::Duration::hours(24));
                let expected_months = 1 + usize::from(
                    (at.year(), at.month())
                        != (
                            (at - chrono::Duration::hours(24)).year(),
                            (at - chrono::Duration::hours(24)).month(),
                        ),
                );
                assert_eq!(months, expected_months);
                eprintln!(
                    "SCAN_HISTORY_WINDOW at={at} since={since} months={months} storage=postgres"
                );
                assert_eq!(
                    whole.statements - after.statements,
                    2 + expected_months as u64,
                    "whole empty scan: two tail SQL plus actual 24h PG history months"
                );
                assert_eq!(whole.commits - after.commits, 0);
                eprintln!(
                    "SCAN_WARM_PROTOCOL query_execute={} parses={}",
                    whole.statements - after.statements,
                    whole_parses - parses_after
                );
                assert_eq!(
                    whole_parses - parses_after,
                    0,
                    "history and tail share the same connection's two cached SELECTs"
                );
                assert_eq!(ports.0 - tail_ports.0, 2, "prefix inventory plus tail inventory");
                assert_eq!(ports.1 - tail_ports.1, 1);
                assert_eq!(ports.2 - tail_ports.2, 1);
                assert_eq!(ports.3 - tail_ports.3, 3, "missing-position pass, orphan sweep, tail");
            }
            for (expected, symbols, executions, commits, parses, window) in history_cases {
                let (at, since, months) = window.expect("actual parameterized PG history window");
                assert_eq!(since, at - chrono::Duration::hours(24));
                assert_eq!(months as u64, expected);
                assert_eq!(executions, expected, "one actual PG history execution per month");
                assert_eq!(commits, 0, "history lookup has no transaction append");
                assert_eq!(parses, 0, "same net/history SQL already cached on this connection");
                assert!(symbols.is_empty(), "empty tenant remains empty across month boundary");
                eprintln!("SCAN_PG_HISTORY at={at} months={months} executions={executions} parses={parses}");
            }
            restored.unwrap();
        }
    }
}
