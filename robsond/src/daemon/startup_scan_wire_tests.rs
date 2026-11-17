use tracing::instrument::WithSubscriber as _;
// Private measurement fixture: reuse the existing proxy, never a new relay.
struct ObservedScanPool {
    proxy: robson_testkit::db_proxy::DbProxy,
    pool: sqlx::PgPool,
}
impl ObservedScanPool {
    async fn new(upstream: &sqlx::PgPool) -> Self {
        let options = upstream.connect_options();
        let addr = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await
            .unwrap()
            .next()
            .unwrap();
        let proxy = robson_testkit::db_proxy::DbProxy::start(addr).await.unwrap();
        // One observed connection makes return-to-pool rollback accounting
        // deterministic; these controls run one scan, not concurrent producers.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .connect_with(
                options
                    .as_ref()
                    .clone()
                    .host("127.0.0.1")
                    .port(proxy.addr().port())
                    .ssl_mode(sqlx::postgres::PgSslMode::Disable),
            )
            .await
            .unwrap();
        Self { proxy, pool }
    }
}

fn assert_wire_delta(
    label: &str,
    before: robson_testkit::db_proxy::ProxyStats,
    after: robson_testkit::db_proxy::ProxyStats,
    expected_statements: u64,
    expected_commits: u64,
) {
    let actual = (
        after.statements - before.statements,
        after.commits - before.commits,
        after.connections - before.connections,
        after.commit_acks_dropped - before.commit_acks_dropped,
    );
    // Emit ACTUAL before assertions. Source predictions are not new limits;
    // any extra statement is preserved as the first failure, never retried away.
    eprintln!("SCAN_WIRE label={label:?} actual_statements={} actual_commits={} new_connections={} dropped_acks={} expected_source_statements={expected_statements} expected_source_commits={expected_commits}", actual.0, actual.1, actual.2, actual.3);
    assert_eq!(
        (actual.0, actual.1),
        (expected_statements, expected_commits),
        "wire source inventory changed: {label}"
    );
    assert_eq!(actual.3, 0);
}

// Reads the exact NOW/window passed to the existing month loop, not a clock
// override.
fn history_months(window: Option<(DateTime<Utc>, DateTime<Utc>, usize)>) -> u64 {
    let (now, since, actual) = window.expect("scan must reach its actual closed-history lookup");
    let expected_since = now - chrono::Duration::hours(24);
    assert_eq!(since, expected_since);
    let expected = 1 + usize::from(
        (now.year(), now.month()) != (expected_since.year(), expected_since.month()),
    );
    assert_eq!(actual, expected, "actual 24h month-window cardinality");
    eprintln!(
        "SCAN_HISTORY now={now} since={since} months={actual} storage=postgres local_repository=0"
    );
    expected as u64
}
fn assert_production_io(label: &str, actual: [u64; 9], statements: u64, failed_prepares: u64) {
    // Public query attempts include failed preparation; BEGIN/COMMIT and queued
    // rollback are separate reached units. Wire executes include no failed Parse.
    eprintln!("SCAN_PRODUCTION label={label:?} db={} http={} local={} score={} incomplete={} query={} begin={} commit={} rollback_queued={} wire={statements} failed_prepares={failed_prepares}", actual[0],actual[1],actual[2],actual[3],actual[4],actual[5],actual[6],actual[7],actual[8]);
    assert_eq!(actual[0], actual[5..9].iter().sum::<u64>());
    assert_eq!(
        actual[0],
        statements + failed_prepares,
        "independent wire reconciliation: {label}"
    );
    assert_eq!(actual[1], 0, "Stub ports are not HTTP submissions");
    assert_eq!(actual[3], actual[0] + 2 * actual[1]);
}
fn assert_parse_delta(label: &str, before: u64, after: u64, expected: u64) {
    eprintln!("SCAN_PARSE label={label:?} actual={} expected={expected} unit=frontend_Parse_not_global_attempts", after-before);
    assert_eq!(after - before, expected, "qualified Parse/cache inventory: {label}");
}

type FixtureActivity = ([u64; 8], [u64; 5], [u64; 3]);
fn fixture_activity(daemon: &TestDaemon) -> FixtureActivity {
    (
        daemon.exchange.fixture_read_counts(),
        daemon.store.fixture_repository_call_counts(),
        [
            daemon.exchange.stop_cancel_call_count(),
            daemon.exchange.position_close_call_count(),
            daemon.exchange.protection_call_count(),
        ],
    )
}
fn assert_activity(
    label: &str,
    before: FixtureActivity,
    after: FixtureActivity,
    expected: FixtureActivity,
) {
    let actual: FixtureActivity = (
        std::array::from_fn(|i| after.0[i] - before.0[i]),
        std::array::from_fn(|i| after.1[i] - before.1[i]),
        std::array::from_fn(|i| after.2[i] - before.2[i]),
    );
    let exchange = actual.0.iter().sum::<u64>() + actual.2.iter().sum::<u64>();
    let repository = actual.1[..3].iter().sum::<u64>();
    eprintln!("SCAN_PORTS label={label:?} read_ports={:?} repository={:?} mutation_counters={:?} port_total={exchange} weighted_port_proxy={} repository_total={repository}", actual.0, actual.1, actual.2, 2*exchange);
    assert_eq!(actual, expected, "actual adapter/repository call inventory: {label}");
}

async fn place_journaled_fixture_orphan(daemon: &Arc<TestDaemon>) -> String {
    use robson_exec::order_journal::{IntentKind, IntentSpec, WriteClass};
    daemon
        .order_journal
        .with_permit(robson_exec::gate::OperationKind::MainLoop, async {
            let client = format!("ins-wire-orphan-{}", Uuid::now_v7());
            let intent = daemon.order_journal.open_intent(IntentSpec {
                position_id: None,
                kind: IntentKind::Protection,
                client_order_id: client.clone(),
                cancels_order_id: None,
                params: serde_json::Value::Null,
            });
            daemon.order_journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
            let position = daemon
                .store
                .positions()
                .find_active()
                .await
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            let PositionState::Active { trailing_stop, .. } = position.state else {
                panic!("actual Active fixture required");
            };
            // Reuse the real fixture's persisted technical stop and quantity;
            // this setup does not compute a stop from an entry percentage.
            let order = daemon
                .exchange
                .place_stop_market_order(
                    &position.symbol,
                    position.side.exit_action(),
                    position.quantity,
                    trailing_stop,
                    &client,
                )
                .await
                .unwrap();
            daemon.order_journal.finish_success(&intent, None).await;
            order.exchange_order_id
        })
        .await
        .unwrap()
}

async fn reached_or_joined(
    pause: &crate::reconciliation_worker::TestPause,
    task: &mut tokio::task::JoinHandle<DaemonResult<usize>>,
) -> Result<(), String> {
    if tokio::time::timeout(Duration::from_secs(3), pause.reached.notified())
        .await
        .is_err()
    {
        task.abort();
        let stopped = task.await;
        return Err(format!("wire boundary timeout; actual joined scan result: {stopped:?}"));
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn real_close_protection_journal_and_tail_have_separate_wire_counts(pool: sqlx::PgPool) {
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
    daemon.exchange.set_futures_balance(dec!(9000));
    let close_pause = worker.test_pause_at(ScanTestPhase::AfterClose);
    let before = wire.proxy.stats();
    let parse_before = wire.proxy.prepared_statement_parses();
    let ports_before = fixture_activity(&daemon);
    let production = crate::operation_observation::probe::Probe::new();
    let dispatch = production.dispatch();
    let mut scan = tokio::spawn({
        let worker = worker.clone();
        async move { worker.scan_and_reconcile().await }.with_subscriber(dispatch)
    });
    if let Err(primary) = reached_or_joined(&close_pause, &mut scan).await {
        wire.pool.close().await;
        panic!("close boundary: {primary}");
    }
    let after_close = wire.proxy.stats();
    let production_close = production.snapshot();
    let parse_close = wire.proxy.prepared_statement_parses();
    let ports_close = fixture_activity(&daemon);
    let guard_pause = worker.test_pause_at(ScanTestPhase::BeforeRiskSnapshot);
    close_pause.resume.notify_one();
    if let Err(primary) = reached_or_joined(&guard_pause, &mut scan).await {
        wire.pool.close().await;
        panic!("before-tail boundary: {primary}");
    }
    let after_protection = wire.proxy.stats();
    let production_protection = production.snapshot();
    let history_window = worker.test_recent_closed_window();
    let ports_protection = fixture_activity(&daemon);
    let parse_protection = wire.proxy.prepared_statement_parses();
    guard_pause.resume.notify_one();
    let result = joined(scan).await;
    let after_tail = wire.proxy.stats();
    let production_tail = production.snapshot();
    let parse_tail = wire.proxy.prepared_statement_parses();
    let ports_tail = fixture_activity(&daemon);
    let cancel_count = daemon.exchange.stop_cancel_call_count();
    let close_sends = daemon.exchange.position_close_call_count();
    let latch = daemon.order_journal.latch_set();
    let closed = daemon.store.positions().find_by_id(id).await;
    let closes: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type='position_closed'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await;
    let cancel: Result<(i64, i64), sqlx::Error> = sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE cancel_class='cancelled') FROM order_intents WHERE tenant_id=$1 AND kind='cancel' AND cancels_order_id=$2")
        .bind(tenant).bind(&orphan).fetch_one(&pool).await;
    let basis: Result<Decimal, sqlx::Error> =
        sqlx::query_scalar("SELECT capital_base FROM monthly_state WHERE year=$1 AND month=$2")
            .bind(Utc::now().year() as i16)
            .bind(Utc::now().month() as i16)
            .fetch_one(&pool)
            .await;
    let h = history_window
        .map(|(at, _, _)| {
            let since = at - chrono::Duration::hours(24);
            1 + u64::from((at.year(), at.month()) != (since.year(), since.month()))
        })
        .unwrap_or(0);
    let source_counts_match =
        (after_close.statements - before.statements, after_close.commits - before.commits)
            == (11, 1)
            && (
                after_protection.statements - after_close.statements,
                after_protection.commits - after_close.commits,
            ) == (9 + h, 2)
            && (
                after_tail.statements - after_protection.statements,
                after_tail.commits - after_protection.commits,
            ) == (2, 0);
    let before_next = wire.proxy.stats();
    let parse_before_next = wire.proxy.prepared_statement_parses();
    let ports_before_next = fixture_activity(&daemon);
    let next = if matches!(&result, Ok(Ok(1))) && source_counts_match {
        Some(worker.scan_and_reconcile().with_subscriber(production.dispatch()).await)
    } else {
        None
    };
    let after_next = wire.proxy.stats();
    let production_next = production.snapshot();
    let parse_after_next = wire.proxy.prepared_statement_parses();
    let next_history_window = worker.test_recent_closed_window();
    let ports_after_next = fixture_activity(&daemon);
    let after: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE tenant_id=$1 AND event_type='position_closed'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await;
    let cancel_after = daemon.exchange.stop_cancel_call_count();
    wire.pool.close().await;
    // Worker was released/joined and observed pool closed before assertions.
    // Existing proxy Drop signals Sever; no relay-join proof is inferred.
    assert_production_io(
        "close prefix",
        production_close,
        after_close.statements - before.statements,
        0,
    );
    assert_production_io(
        "close plus protection",
        production_protection,
        after_protection.statements - before.statements,
        0,
    );
    assert_production_io(
        "whole positive",
        production_tail,
        after_tail.statements - before.statements,
        0,
    );
    assert_production_io(
        "next whole positive",
        production_next,
        after_next.statements - before_next.statements,
        0,
    );
    assert_eq!(production_close[7], after_close.commits - before.commits);
    assert_eq!(production_tail[7], after_tail.commits - before.commits);
    assert_eq!(production_tail[8], 0, "healthy transactions queue no rollback");
    assert_eq!(production_tail[4], 0, "complete positive attribution");
    assert_eq!(result.unwrap().unwrap(), 1);
    let h = history_months(history_window);
    eprintln!("SCAN_PHASE_PARSES close={} protection={} tail={} whole={}; history shares the already-prepared net SELECT", parse_close-parse_before, parse_protection-parse_close, parse_tail-parse_protection, parse_tail-parse_before);
    assert_parse_delta("cached idle tail", parse_protection, parse_tail, 0);
    assert_wire_delta(
        "close append+projection9 plus MonthlyHalt read2",
        before,
        after_close,
        11,
        1,
    );
    assert_wire_delta(
        "orphan history plus cancel journal",
        after_close,
        after_protection,
        9 + h,
        2,
    );
    assert_wire_delta("idle tail", after_protection, after_tail, 2, 0);
    assert_wire_delta("whole positive close+protection+tail", before, after_tail, 22 + h, 3);
    assert_activity(
        "close prefix+append+MonthlyHalt",
        ports_before,
        ports_close,
        ([1, 0, 0, 1, 0, 0, 0, 0], [3, 0, 1, 1, 1], [0, 0, 0]),
    );
    assert_activity(
        "orphan sweep+durable cancel",
        ports_close,
        ports_protection,
        ([0, 0, 0, 0, 1, 0, 0, 0], [2, 0, 0, 0, 2], [1, 0, 0]),
    );
    assert_activity(
        "idle checksum tail",
        ports_protection,
        ports_tail,
        ([1, 1, 0, 0, 0, 0, 0, 0], [2, 0, 0, 1, 1], [0, 0, 0]),
    );
    assert_activity(
        "whole close+protection",
        ports_before,
        ports_tail,
        ([2, 1, 0, 1, 1, 0, 0, 0], [7, 0, 1, 2, 4], [1, 0, 0]),
    );
    assert_eq!(cancel_count, 1);
    assert_eq!(close_sends, 0, "observed fill is never a new close send");
    assert!(!latch);
    assert!(matches!(closed.unwrap().unwrap().state, PositionState::Closed { .. }));
    let closes = closes.unwrap();
    assert_eq!(closes, 1);
    assert_eq!(cancel.unwrap(), (1, 1));
    assert_eq!(basis.unwrap(), dec!(10000));
    assert_eq!(next.expect("healthy primary reaches next scan").unwrap(), 0);
    let next_h = history_months(next_history_window);
    assert_wire_delta(
        "next positive scan: history plus tail, no close/journal",
        before_next,
        after_next,
        2 + next_h,
        0,
    );
    assert_parse_delta(
        "next whole cached history and tail",
        parse_before_next,
        parse_after_next,
        0,
    );
    assert_activity(
        "next scan closed-history sweep",
        ports_before_next,
        ports_after_next,
        ([2, 1, 0, 0, 1, 0, 0, 0], [4, 0, 0, 1, 3], [0, 0, 0]),
    );
    assert_eq!(after.unwrap(), closes);
    assert_eq!(cancel_after, 1);
}

// Actual multi-leg/cardinality control shares these existing proxy/oracle
// helpers.
include!("startup_scan_cardinality_tests.rs");
