type TestDaemon = Daemon<StubExchange, MemoryStore>;
type TestWorker = ReconciliationWorker<StubExchange, MemoryStore>;

async fn prepared_scan_with_fills(
    pool: &sqlx::PgPool,
    model: &str,
    reset_landed: bool,
    close_count: u8,
    exit_price: Decimal,
    fee: Decimal,
) -> (Arc<TestDaemon>, Arc<TestWorker>, Uuid, Uuid) {
    prepared_scan_with_fills_journal(pool, model, reset_landed, close_count, exit_price, fee, false)
        .await
}

async fn prepared_scan_with_fills_journal(
    pool: &sqlx::PgPool,
    model: &str,
    reset_landed: bool,
    close_count: u8,
    exit_price: Decimal,
    fee: Decimal,
    durable_journal: bool,
) -> (Arc<TestDaemon>, Arc<TestWorker>, Uuid, Uuid) {
    let now = Utc::now();
    // Only the SQLx macro-owned database is reset between drained cases.
    sqlx::query("DELETE FROM income_ledger").execute(pool).await.unwrap();
    sqlx::query("DELETE FROM positions_current").execute(pool).await.unwrap();
    sqlx::query("INSERT INTO monthly_state(year,month,capital_base,monthly_budget_model,boundary_reset_at,created_at) VALUES($1,$2,10000,$3,$4,$5) ON CONFLICT(year,month) DO UPDATE SET capital_base=10000,realized_loss=0,month_peak_net=0,monthly_budget_model=EXCLUDED.monthly_budget_model,boundary_reset_at=EXCLUDED.boundary_reset_at")
            .bind(now.year() as i16).bind(now.month() as i16).bind(model)
            .bind(if reset_landed {Some(now)} else {None}).bind(now).execute(pool).await.unwrap();
    let tenant = Uuid::now_v7();
    let mut config = config_with_auto_reconcile();
    config.projection.tenant_id = Some(tenant);
    let daemon = Arc::new(if durable_journal {
        Daemon::new_stub_with_durable_journal(config, Arc::new(pool.clone()))
    } else {
        Daemon::new_stub_with_recovery(config, None, Some(Arc::new(pool.clone())))
    });
    daemon.exchange.set_futures_balance(dec!(10000));
    let mut ids = Vec::new();
    for (index, pair) in [
        "BTCUSDT", "ETHUSDT", "SOLUSDT", "ADAUSDT", "XRPUSDT", "DOGEUSDT", "AVAXUSDT", "LINKUSDT",
    ]
    .into_iter()
    .take(usize::from(close_count))
    .enumerate()
    {
        let mut position =
            active_position(robson_domain::Symbol::from_pair(pair).unwrap(), Side::Long);
        let id = position.id;
        ids.push(id);
        let order = format!("SCAN-REAL-FILL-{index}");
        attach_insurance_order(&daemon.store, &mut position, &order).await;
        daemon.store.positions().save(&position).await.unwrap();
        let PositionState::Active {
            current_price,
            trailing_stop,
            favorable_extreme,
            extreme_at,
            insurance_stop_id,
            invalidation_guard_level,
            ..
        } = &position.state
        else {
            panic!("prepared scan requires the actual Active fixture state");
        };
        sqlx::query("INSERT INTO positions_current(position_id,tenant_id,account_id,symbol,side,entry_price,entry_quantity,current_quantity,current_price,trailing_stop_price,favorable_extreme,extreme_at,insurance_stop_id,invalidation_guard_level,state,last_event_id,last_seq,created_at,updated_at) VALUES($1,$2,$3,$4,'long',$5,$6,$6,$7,$8,$9,$10,$11,$12,'active',$13,0,$14,$15)")
                .bind(id).bind(tenant).bind(position.account_id).bind(pair)
                .bind(position.entry_price.expect("actual Active fixture entry").as_decimal())
                .bind(position.quantity.as_decimal()).bind(current_price.as_decimal())
                .bind(trailing_stop.as_decimal()).bind(favorable_extreme.as_decimal())
                .bind(extreme_at).bind(insurance_stop_id.as_deref())
                .bind(invalidation_guard_level.as_ref().map(|price| price.as_decimal()))
                .bind(Uuid::now_v7()).bind(position.created_at).bind(position.updated_at)
                .execute(pool).await.unwrap();
        let mut fill = order_result(&order, exit_price, dec!(0.010), now);
        fill.fee = fee;
        fill.client_order_id = format!("ins-{id}");
        daemon.exchange.set_order_result(&order, fill);
    }
    let worker = Arc::new(
        ReconciliationWorker::new_with_missing_grace(
            daemon.exchange.clone(),
            daemon.position_manager.clone(),
            daemon.store.clone(),
            daemon.event_bus.clone(),
            Duration::from_secs(60),
            Duration::ZERO,
            CancellationToken::new(),
        )
        .with_order_journal(daemon.order_journal.clone()),
    );
    assert_eq!(worker.scan_and_reconcile().await.unwrap(), 0);
    (daemon, worker, ids[0], tenant)
}

async fn prepared_scan(
    pool: &sqlx::PgPool,
    model: &str,
    reset_landed: bool,
) -> (Arc<TestDaemon>, Arc<TestWorker>, Uuid, Uuid) {
    prepared_scan_with_fills(pool, model, reset_landed, 1, dec!(90), dec!(0.01)).await
}

#[derive(Debug)]
enum TaskFailure {
    Watchdog,
    Join,
}

// Every test-spawned task is joined, including watchdog/error branches.
async fn joined<T: Send + 'static>(mut task: tokio::task::JoinHandle<T>) -> Result<T, TaskFailure> {
    match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
        Ok(result) => result.map_err(|error| {
            eprintln!("SCAN_TASK_JOIN: {error}");
            TaskFailure::Join
        }),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(TaskFailure::Watchdog)
        },
    }
}

async fn projected_basis(pool: &sqlx::PgPool) -> Decimal {
    let now = Utc::now();
    sqlx::query_scalar("SELECT capital_base FROM monthly_state WHERE year=$1 AND month=$2")
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .fetch_one(pool)
        .await
        .unwrap()
}

// Typed zero-P&L durable history on the orphan's symbol. The original Active
// projection is untouched, and this setup is before injected
// failures/baselines.
async fn seed_closed_orphan_symbol(pool: &sqlx::PgPool, tenant: Uuid) {
    let mut leg = active_position(robson_domain::Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
    let at = Utc::now();
    let exit_price = leg.entry_price.expect("typed fixture entry");
    leg.closed_at = Some(at);
    leg.state = PositionState::Closed {
        exit_price,
        realized_pnl: Decimal::ZERO,
        exit_reason: robson_domain::ExitReason::InsuranceStop,
    };
    sqlx::query("INSERT INTO positions_current(position_id,tenant_id,account_id,symbol,side,entry_price,entry_quantity,current_quantity,current_price,state,realized_pnl,total_fees,closed_at,exit_reason,last_event_id,last_seq,created_at,updated_at) VALUES($1,$2,$3,$4,'long',$5,$6,0,$5,'closed',0,0,$7,'insurance_stop',$8,0,$9,$7)")
        .bind(leg.id).bind(tenant).bind(leg.account_id).bind(leg.symbol.as_pair())
        .bind(exit_price.as_decimal()).bind(leg.quantity.as_decimal()).bind(leg.closed_at)
        .bind(Uuid::now_v7()).bind(leg.created_at).execute(pool).await.unwrap();
}

async fn close_survives_failure(pool: sqlx::PgPool, append_fails: bool) {
    let wire = ObservedScanPool::new(&pool).await;
    let (daemon, worker, id, tenant) = prepared_scan_with_fills_journal(
        &wire.pool,
        "net_from_start_non_expanding_v1",
        true,
        1,
        dec!(90),
        dec!(0.01),
        true,
    )
    .await;
    let orphan = place_journaled_fixture_orphan(&daemon).await;
    seed_closed_orphan_symbol(&pool, tenant).await;
    if append_fails {
        sqlx::query(&format!("ALTER TABLE event_log ADD CONSTRAINT test_refuse_scan_close CHECK(event_type <> 'position_closed' OR stream_key <> 'position:{id}')")).execute(&pool).await.unwrap();
    } else {
        sqlx::query(&format!("ALTER TABLE positions_current ADD CONSTRAINT test_refuse_scan_projection CHECK(state <> 'closed' OR position_id <> '{id}'::uuid)")).execute(&pool).await.unwrap();
    }
    daemon.exchange.set_futures_balance(dec!(9000));
    let before = wire.proxy.stats();
    let parses_before = wire.proxy.prepared_statement_parses();
    let sends_before = daemon.exchange.mutations().len();
    let ports_before = fixture_activity(&daemon);
    let production = crate::operation_observation::probe::Probe::new();
    let first_scan = worker.scan_and_reconcile().with_subscriber(production.dispatch()).await;
    let production_first = production.snapshot();
    let after_scan = wire.proxy.stats();
    let parses_after_scan = wire.proxy.prepared_statement_parses();
    let history_window = worker.test_recent_closed_window();
    let withheld = daemon.order_journal.withheld_report();
    let sends_after_scan = daemon.exchange.mutations().len();
    let ports_after_scan = fixture_activity(&daemon);
    let h = history_window
        .map(|(at, _, _)| {
            let since = at - chrono::Duration::hours(24);
            1 + u64::from((at.year(), at.month()) != (since.year(), since.month()))
        })
        .unwrap_or(0);
    let expected_scan = if append_fails {
        (9 + h, 0)
    } else {
        (12 + h, 1)
    };
    let primary_ok = first_scan.as_ref().is_ok_and(|count| *count == 1)
        && (after_scan.statements - before.statements, after_scan.commits - before.commits)
            == expected_scan;
    let cancel_count = daemon.exchange.stop_cancel_call_count();
    let open_orders = daemon
        .exchange
        .get_open_orders(&robson_domain::Symbol::from_pair("BTCUSDT").unwrap())
        .await;
    let before_flush = wire.proxy.stats();
    let parses_before_flush = wire.proxy.prepared_statement_parses();
    let ports_before_flush = fixture_activity(&daemon);
    let flushed = if primary_ok {
        Some(
            daemon
                .order_journal
                .flush_pending()
                .with_subscriber(production.dispatch())
                .await,
        )
    } else {
        None
    };
    let after_flush = wire.proxy.stats();
    let production_after_flush = production.snapshot();
    let parses_after_flush = wire.proxy.prepared_statement_parses();
    let ports_after_flush = fixture_activity(&daemon);
    let pending = daemon.order_journal.retry_set_depth();
    let suspension: Result<(String, Option<i64>), sqlx::Error> = sqlx::query_as("SELECT error,failed_seq FROM stream_suspensions WHERE tenant_id=$1 AND stream_key=$2 AND cleared_at IS NULL")
        .bind(tenant).bind(format!("position:{id}")).fetch_one(&pool).await;
    let closed = daemon.store.positions().find_by_id(id).await;
    let latch = daemon.order_journal.latch_set();
    let persisted: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type='position_closed'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await;
    let state: Result<String, sqlx::Error> =
        sqlx::query_scalar("SELECT state FROM positions_current WHERE position_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await;
    let basis: Result<Decimal, sqlx::Error> =
        sqlx::query_scalar("SELECT capital_base FROM monthly_state WHERE year=$1 AND month=$2")
            .bind(Utc::now().year() as i16)
            .bind(Utc::now().month() as i16)
            .fetch_one(&pool)
            .await;
    let adjustments: Result<i64, sqlx::Error> = sqlx::query_scalar(
            "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type='capital_base_recalibrated'")
            .bind(tenant).fetch_one(&pool).await;
    let before_next = wire.proxy.stats();
    let parses_before_next = wire.proxy.prepared_statement_parses();
    let ports_before_next = fixture_activity(&daemon);
    let next = if primary_ok
        && flushed == Some(2)
        && pending == 0
        && (
            after_flush.statements - before_flush.statements,
            after_flush.commits - before_flush.commits,
        ) == (6, 2)
    {
        Some(worker.scan_and_reconcile().with_subscriber(production.dispatch()).await)
    } else {
        None
    };
    let after_next = wire.proxy.stats();
    let production_next = production.snapshot();
    let parses_after_next = wire.proxy.prepared_statement_parses();
    let next_history_window = worker.test_recent_closed_window();
    let withheld_next = daemon.order_journal.withheld_report();
    let sends_after_next = daemon.exchange.mutations().len();
    let ports_after_next = fixture_activity(&daemon);
    let after: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type='position_closed'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await;
    let cancel_after = daemon.exchange.stop_cancel_call_count();
    let latch_after = daemon.order_journal.latch_set();
    // Preserve all primary Results; close the observed pool BEFORE an oracle
    // can panic. Drop requests Sever, but is not a joined-relay receipt.
    wire.pool.close().await;
    assert_production_io(
        "failed-close reached scan",
        production_first,
        after_scan.statements - before.statements,
        0,
    );
    assert_production_io(
        "failed-close plus late correlated flush",
        production_after_flush,
        after_scan.statements - before.statements + after_flush.statements
            - before_flush.statements,
        0,
    );
    assert_eq!(
        production_after_flush[0] - production_first[0],
        after_flush.statements - before_flush.statements,
        "flush is attributed to its returned scan once"
    );
    assert_eq!(
        production_after_flush[7] - production_first[7],
        after_flush.commits - before_flush.commits
    );
    assert_eq!(
        production_first[8],
        u64::from(append_fails),
        "append error queues rollback; projection error is autocommit"
    );
    assert_eq!(production_after_flush[4], 0, "single-origin late work remains complete");
    assert_production_io(
        "next after failed close",
        production_next,
        after_next.statements - before_next.statements,
        0,
    );
    assert_eq!(first_scan.unwrap(), 1);
    history_months(history_window);
    eprintln!("SCAN_FAILURE_PARSE append_fails={append_fails} first={} unit=frontend_Parse actual_not_global_attempts", parses_after_scan-parses_before);
    assert_wire_delta(
        "failed-close scan incl tail",
        before,
        after_scan,
        expected_scan.0,
        expected_scan.1,
    );
    assert_activity(
        "failed close+vetoed orphan+tail",
        ports_before,
        ports_after_scan,
        ([2, 1, 0, 1, 1, 0, 0, 0], [7, 0, 1, 2, 4], [0, 0, 0]),
    );
    assert_activity(
        "journal incident flush no venue/repository calls",
        ports_before_flush,
        ports_after_flush,
        ([0; 8], [0; 5], [0; 3]),
    );
    assert_eq!(withheld.total, 1, "reachable orphan must hit the actual veto");
    assert_eq!(withheld.orders.len(), 1);
    assert_eq!(withheld.orders[0].exchange_order_id, orphan);
    assert_eq!(withheld.orders[0].symbol, "BTCUSDT");
    assert_eq!(sends_after_scan, sends_before, "veto reaches no fixture send boundary");
    assert_eq!(daemon.exchange.position_close_call_count(), 0);
    assert_eq!(cancel_count, 0, "suspension fence must veto orphan cancellation");
    assert!(open_orders.unwrap().iter().any(|order| order.exchange_order_id == orphan));
    eprintln!(
        "SCAN_FLUSH_PARSE actual={} unit=frontend_Parse first_flush_cache_unspecified",
        parses_after_flush - parses_before_flush
    );
    assert_wire_delta("incident+suspension flush separately", before_flush, after_flush, 6, 2);
    assert_eq!(flushed, Some(2));
    let (failure, failed_seq) = suspension.unwrap();
    let expected_constraint = if append_fails {
        "test_refuse_scan_close"
    } else {
        "test_refuse_scan_projection"
    };
    assert!(
        failure.contains(expected_constraint),
        "actual persistence error must name the target injection: {failure}"
    );
    assert_eq!(
        failed_seq.is_some(),
        !append_fails,
        "append failed before seq; projection failed after landed envelope"
    );
    assert_eq!(pending, 0);
    assert!(matches!(closed.unwrap().unwrap().state, PositionState::Closed { .. }));
    assert!(latch, "failed_close_persistence_must_preserve_admission_fence");
    let persisted = persisted.unwrap();
    assert_eq!(persisted, i64::from(!append_fails));
    assert_eq!(state.unwrap(), "active", "failed_projection_must_not_appear_durable");
    assert_eq!(basis.unwrap(), dec!(10000));
    assert_eq!(adjustments.unwrap(), 0);
    assert_eq!(next.expect("healthy primary reaches next scan").unwrap(), 0);
    let next_h = history_months(next_history_window);
    assert_wire_delta(
        "next failed-close scan: PG history plus tail",
        before_next,
        after_next,
        2 + next_h,
        0,
    );
    assert_parse_delta(
        "next failure scan cached history and tail",
        parses_before_next,
        parses_after_next,
        0,
    );
    assert_eq!(withheld_next.total, 1);
    assert_eq!(withheld_next.orders.len(), 1);
    assert_eq!(withheld_next.orders[0].exchange_order_id, orphan);
    assert_eq!(sends_after_next, sends_before);
    assert_activity(
        "next fenced scan keeps orphan",
        ports_before_next,
        ports_after_next,
        ([2, 1, 0, 0, 1, 0, 0, 0], [5, 0, 0, 1, 4], [0, 0, 0]),
    );
    assert_eq!(cancel_after, 0);
    assert!(latch_after);
    assert_eq!(after.unwrap(), persisted, "next_scan_must_not_append_the_financial_close_again");
}
