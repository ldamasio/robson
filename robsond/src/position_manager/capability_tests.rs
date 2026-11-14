//! Capability denial exercises complete manager consumers, not only the
//! predicate.
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use robson_domain::Candle;
use robson_exec::{ports::CandleInterval, venue::AccountPort, IntentJournal};
use robson_store::MemoryStore;
use robson_testkit::{balance_stub::BalanceStub, spot_book::SpotBook};
use rust_decimal_macros::dec;

use super::*;
use crate::query_engine::{QueryRecorderError, TracingQueryRecorder};

type Manager = PositionManager<BalanceStub, MemoryStore>;

#[derive(Default)]
struct Recorder {
    delegate: Option<Arc<dyn QueryRecorder>>,
    calls: std::sync::Mutex<Vec<String>>,
    mode: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl QueryRecorder for Recorder {
    async fn record_transition(
        &self,
        query: &ExecutionQuery,
        cause: &str,
    ) -> Result<(), QueryRecorderError> {
        self.calls.lock().unwrap().push(cause.to_owned());
        self.entered.notify_one();
        match self.mode.load(Ordering::SeqCst) {
            1 => Err(serde_json::from_str::<serde_json::Value>("not json").unwrap_err().into()),
            2 => {
                self.release.notified().await;
                Ok(())
            },
            _ => match &self.delegate {
                Some(delegate) => delegate.record_transition(query, cause).await,
                None => Ok(()),
            },
        }
    }
}

#[derive(Default)]
struct CountOhlcv(AtomicUsize);

#[async_trait]
impl OhlcvPort for CountOhlcv {
    async fn fetch_candles(
        &self,
        _: &Symbol,
        _: CandleInterval,
        _: u16,
    ) -> Result<Vec<Candle>, ExecError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ExecError::Config("unexpected capability-denial OHLCV request".into()))
    }
}

struct Fixture {
    manager: Arc<Manager>,
    recorder: Arc<Recorder>,
    candles: Arc<CountOhlcv>,
}

fn fixture() -> Fixture {
    fixture_with_recorder(Arc::new(Recorder::default()))
}

fn fixture_with_recorder(recorder: Arc<Recorder>) -> Fixture {
    fixture_with_journal(recorder, None)
}

fn fixture_with_journal(
    recorder: Arc<Recorder>,
    journal: Option<Arc<robson_exec::order_journal::OrderJournal>>,
) -> Fixture {
    let exchange = Arc::new(BalanceStub::new(Arc::new(SpotBook::new(vec![], Decimal::ZERO))));
    let store = Arc::new(MemoryStore::new());
    let candles = Arc::new(CountOhlcv::default());
    let executor = Executor::new(exchange, Arc::new(IntentJournal::new()), store.clone());
    let executor = Arc::new(match journal {
        Some(journal) => executor.with_order_journal(journal),
        None => executor,
    });
    let manager = Arc::new(
        PositionManager::new(
            Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
            executor,
            store,
            Arc::new(EventBus::new(32)),
            recorder.clone(),
            TradingPolicy::default(),
        )
        .with_ohlcv_port(candles.clone()),
    );
    Fixture { manager, recorder, candles }
}

fn position(side: Side) -> Position {
    Position::new(Uuid::now_v7(), Symbol::from_pair("ETHUSDT").unwrap(), side)
}

async fn signal(position: &Position, side: Side) -> DetectorSignal {
    // Derive the stop from the canonical chart fixture, never a price percentage.
    let candles = StubOhlcv::default()
        .fetch_candles(&position.symbol, CandleInterval::FifteenMinutes, 100)
        .await
        .unwrap();
    let price = Price::new(candles.last().unwrap().close).unwrap();
    let analysis = robson_engine::technical_stop_analyzer::TechnicalStopAnalyzer::analyze(
        &candles,
        price,
        side,
        &TechnicalStopConfig::default(),
    )
    .unwrap();
    DetectorSignal::new(position.id, position.symbol.clone(), side, price, analysis.stop_price)
}

async fn unchanged(f: &Fixture, original: &Position) {
    assert_eq!(
        serde_json::to_value(f.manager.get_position(original.id).await.unwrap().unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    assert!(f.manager.detectors.read().await.is_empty());
    assert!(f.manager.governed_rearm_counts.read().await.is_empty());
    assert!(!f.manager.is_entry_exhausted(original.id).await);
    assert_eq!(f.candles.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.manager.exchange().sends(), 0);
}

async fn pending(f: &Fixture, current: &Position, cached: Side, proposed_side: Side) -> Uuid {
    let mut cached_position = current.clone();
    cached_position.side = cached;
    let signal = signal(current, proposed_side).await;
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessSignal {
            signal_id: signal.signal_id,
            symbol: current.symbol.clone(),
            side: proposed_side,
            entry_price: signal.entry_price,
            stop_loss: signal.stop_loss,
        },
        ActorKind::Detector,
    );
    query.position_id = Some(current.id);
    query.transition(QueryState::Processing).unwrap();
    let proposed = ProposedTrade {
        symbol: current.symbol.as_pair(),
        side: format!("{proposed_side:?}").to_lowercase(),
        quantity: dec!(1),
        entry_price: dec!(100),
        notional_value: dec!(100),
        initial_margin: dec!(100),
        planned_risk: dec!(1),
    };
    let engine = QueryEngine::new(Arc::new(TracingQueryRecorder), RiskGate::new());
    let governed = engine
        .check_risk(&mut query, &proposed, &RiskContext::new(dec!(10000)), vec![])
        .await
        .unwrap();
    query.await_approval("capability fixture".into(), 60).unwrap();
    let id = query.id;
    f.manager.pending_approvals.write().await.insert(id, PendingApprovalRecord {
        query,
        position: cached_position,
        proposed,
        governed,
    });
    id
}

#[tokio::test]
async fn unsupported_arm_preview_restore_and_rearm_are_pure() {
    let f = fixture();
    let p = position(Side::Short);
    let initial_capital = f.manager.risk_config_snapshot().capital();
    assert!(matches!(
        f.manager
            .arm_position_with_policy(
                p.symbol.clone(),
                p.side,
                RiskConfig::new(dec!(7654)).unwrap(),
                None,
                p.account_id,
                EntryPolicyConfig::default()
            )
            .await,
        Err(DaemonError::VenueUnsupported { side: Side::Short })
    ));
    assert_eq!(f.manager.risk_config_snapshot().capital(), initial_capital);
    assert!(f.manager.store.positions().find_active().await.unwrap().is_empty());
    assert!(matches!(
        f.manager.prepare_entry_preview(p.symbol.clone(), p.side).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    f.manager.store.positions().save(&p).await.unwrap();
    assert!(matches!(
        f.manager.restore_armed_position(&p, EntryPolicyConfig::default()).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    assert!(f.manager.entry_policies.read().await.is_empty());
    assert!(!f.manager.rearm_detector_after_governed_block(p.id, &p, "capability test").await);
    assert!(
        !Manager::rearm_detector(
            f.manager.exchange().capabilities(),
            f.manager.venue.clone(),
            p.id,
            p.clone(),
            EntryPolicyConfig::default(),
            TechnicalStopConfig::default(),
            f.manager.event_bus.clone(),
            f.candles.clone(),
            f.manager.detectors.clone(),
            &f.manager.circuit_breaker,
            f.manager.shutdown_token.clone(),
            "capability test",
            false,
            0,
            None
        )
        .await
    );
    unchanged(&f, &p).await;
    assert!(f.recorder.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unsupported_signals_preserve_authoritative_facts_and_never_rearm() {
    for (current, incoming) in [
        (Side::Short, Side::Long),
        (Side::Long, Side::Short),
        (Side::Short, Side::Short),
    ] {
        let f = fixture();
        let mut p = position(current);
        // Evidence is retained even though the legacy state still says Armed.
        p.entry_order_id = Some(Uuid::now_v7());
        f.manager.store.positions().save(&p).await.unwrap();
        for _ in 0..3 {
            assert!(matches!(
                f.manager.handle_signal(signal(&p, incoming).await).await,
                Err(DaemonError::VenueUnsupported { .. })
            ));
        }
        unchanged(&f, &p).await;
        assert!(f.recorder.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unsupported_approvals_consume_once_even_when_the_audit_fails() {
    for mode in [0, 1] {
        for (current, cached, proposed) in [
            (Side::Short, Side::Long, Side::Long),
            (Side::Long, Side::Short, Side::Long),
            (Side::Long, Side::Long, Side::Short),
        ] {
            let f = fixture();
            let p = position(current);
            f.manager.store.positions().save(&p).await.unwrap();
            let id = pending(&f, &p, cached, proposed).await;
            f.recorder.mode.store(mode, Ordering::SeqCst);
            let result = f.manager.approve_query(id).await;
            if mode == 0 {
                assert!(matches!(result, Err(DaemonError::VenueUnsupported { .. })));
            } else {
                assert!(matches!(result, Err(DaemonError::QueryAudit(_))));
            }
            assert!(matches!(
                f.manager.approve_query(id).await,
                Err(DaemonError::QueryNotFound(_))
            ));
            assert_eq!(*f.recorder.calls.lock().unwrap(), ["failed"]);
            unchanged(&f, &p).await;
        }
    }
}

#[tokio::test]
async fn cancelled_capability_audit_cannot_rearm_or_resurrect_pending_work() {
    let f = fixture();
    let p = position(Side::Long);
    f.manager.store.positions().save(&p).await.unwrap();
    let id = pending(&f, &p, Side::Short, Side::Long).await;
    f.recorder.mode.store(2, Ordering::SeqCst);
    let manager = f.manager.clone();
    let task = tokio::spawn(async move { manager.approve_query(id).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), f.recorder.entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(f.manager.pending_approvals.read().await.is_empty());
    unchanged(&f, &p).await;
    assert_eq!(*f.recorder.calls.lock().unwrap(), ["failed"]);
}

#[tokio::test]
async fn autonomous_expiry_denial_never_enters_expired_or_rearm_cleanup() {
    for mode in [0, 1] {
        let f = fixture();
        let p = position(Side::Long);
        f.manager.store.positions().save(&p).await.unwrap();
        let id = pending(&f, &p, Side::Short, Side::Long).await;
        f.recorder.mode.store(mode, Ordering::SeqCst);
        f.manager.spawn_approval_expiration_task(id, chrono::Utc::now());
        tokio::time::timeout(std::time::Duration::from_secs(2), f.recorder.entered.notified())
            .await
            .unwrap();
        let _finished = f.manager.entry_flow_lock.lock().await;
        assert!(f.manager.pending_approvals.read().await.is_empty());
        assert_eq!(*f.recorder.calls.lock().unwrap(), ["failed"]);
        unchanged(&f, &p).await;
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(32))]

    #[test]
    fn generated_expiry_approval_disarm_schedules_preserve_financial_facts(
        approval_yields in 0usize..6, disarm_yields in 0usize..6, failed_audit in proptest::bool::ANY,
    ) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let f = fixture();
            let p = position(Side::Long);
            f.manager.store.positions().save(&p).await.unwrap();
            let id = pending(&f, &p, Side::Short, Side::Long).await;
            f.recorder.mode.store(usize::from(failed_audit), Ordering::SeqCst);
            let guard = f.manager.entry_flow_lock.lock().await;
            f.manager.spawn_approval_expiration_task(id, chrono::Utc::now());
            let manager = f.manager.clone();
            let approve = tokio::spawn(async move {
                for _ in 0..approval_yields { tokio::task::yield_now().await; }
                manager.approve_query(id).await
            });
            let manager = f.manager.clone();
            let position_id = p.id;
            let disarm = tokio::spawn(async move {
                for _ in 0..disarm_yields { tokio::task::yield_now().await; }
                manager.disarm_position(position_id).await
            });
            tokio::task::yield_now().await;
            drop(guard);
            let (approved, disarmed) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(approve, disarm)
            }).await.unwrap();
            assert!(approved.unwrap().is_err());
            let _ = disarmed.unwrap(); // Failure injection may stop the disarm audit.
            let current = f.manager.get_position(p.id).await.unwrap().unwrap();
            assert_eq!((current.quantity, current.fees_paid, current.realized_pnl, current.entry_price, current.entry_order_id, current.residual_quantity),
                       (p.quantity, p.fees_paid, p.realized_pnl, p.entry_price, p.entry_order_id, p.residual_quantity));
            assert!(matches!(current.state, PositionState::Armed | PositionState::Cancelled));
            assert!(f.manager.pending_approvals.read().await.is_empty());
            assert!(f.manager.detectors.read().await.is_empty());
            assert!(f.manager.governed_rearm_counts.read().await.is_empty());
            assert!(!f.manager.is_entry_exhausted(p.id).await);
            assert_eq!(f.candles.0.load(Ordering::SeqCst), 0);
            assert_eq!(f.manager.exchange().sends(), 0);
            f.manager.shutdown_token.cancel();
        });
    }
}

#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn capability_denial_audit_wire_counts_include_failure_cleanup(pool: sqlx::PgPool) {
    use robson_testkit::db_proxy::{DbProxy, ProxyMode};
    use sqlx::postgres::{PgPoolOptions, PgSslMode};

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
    sqlx::raw_sql("CREATE FUNCTION capability_audit_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'capability fixture fault' USING ERRCODE = '23514'; END $$").execute(&pool).await.unwrap();
    let cases = [
        ("success", "", 6),
        ("sequence", "CREATE TRIGGER capability_fail BEFORE INSERT ON stream_state FOR EACH ROW EXECUTE FUNCTION capability_audit_fault()", 3),
        ("idempotency", "CREATE TRIGGER capability_fail BEFORE INSERT ON event_idempotency FOR EACH ROW EXECUTE FUNCTION capability_audit_fault()", 4),
        ("event", "CREATE TRIGGER capability_fail BEFORE INSERT ON event_log FOR EACH ROW EXECUTE FUNCTION capability_audit_fault()", 5),
        ("stream_update", "CREATE TRIGGER capability_fail BEFORE UPDATE OF last_event_id ON stream_state FOR EACH ROW EXECUTE FUNCTION capability_audit_fault()", 6),
        ("deferred_commit", "CREATE CONSTRAINT TRIGGER capability_fail AFTER INSERT ON event_log DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION capability_audit_fault()", 7),
        ("lost_commit", "", 6),
    ];
    for autonomous in [false, true] {
        for (stage, ddl, expected) in cases {
            if !ddl.is_empty() {
                sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
            }
            let tenant = Uuid::now_v7();
            let stream = format!("query:capability:{tenant}");
            let recorder = Arc::new(Recorder {
                delegate: Some(Arc::new(crate::query_engine::EventLogQueryRecorder::new(
                    proxied.clone(),
                    tenant,
                    stream.clone(),
                ))),
                ..Recorder::default()
            });
            let f = fixture_with_recorder(recorder);
            let p = position(Side::Long);
            f.manager.store.positions().save(&p).await.unwrap();
            let id = pending(&f, &p, Side::Short, Side::Long).await;
            proxy.enable_query_trace(32);
            let before = proxy.stats().statements;
            if stage == "lost_commit" {
                proxy.set_mode(ProxyMode::DropCommitAckOf("INSERT INTO event_log"));
            }
            if autonomous {
                f.manager.spawn_approval_expiration_task(id, chrono::Utc::now());
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    f.recorder.entered.notified(),
                )
                .await
                .unwrap();
                let finished = f.manager.entry_flow_lock.lock().await;
                drop(finished);
            } else {
                let result = f.manager.approve_query(id).await;
                if stage == "success" {
                    assert!(matches!(result, Err(DaemonError::VenueUnsupported { .. })));
                } else {
                    assert!(
                        matches!(result, Err(DaemonError::QueryAudit(_))),
                        "{stage}: {result:?}"
                    );
                }
            }
            // Pool size1 plus acquisition waits for SQLx's queued rollback to
            // finish. No verification statement is hidden in the measured path.
            let ready = tokio::time::timeout(std::time::Duration::from_secs(3), proxied.acquire())
                .await
                .unwrap()
                .unwrap();
            let trace = proxy.query_trace();
            assert_eq!(proxy.query_trace_dropped(), 0);
            assert_eq!(
                proxy.stats().statements - before,
                expected,
                "{stage}, autonomous={autonomous}: {trace:?}"
            );
            drop(ready);
            assert_eq!(*f.recorder.calls.lock().unwrap(), ["failed"]);
            assert!(f.manager.pending_approvals.read().await.is_empty());
            unchanged(&f, &p).await;
            let rows: i64 =
                sqlx::query_scalar("SELECT count(*) FROM event_log WHERE stream_key = $1")
                    .bind(&stream)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(rows, i64::from(stage == "success" || stage == "lost_commit"));
            assert!(matches!(
                f.manager.approve_query(id).await,
                Err(DaemonError::QueryNotFound(_))
            ));
            assert_eq!(proxy.stats().statements - before, expected, "repeated denial wrote again");
            f.manager.shutdown_token.cancel();
            for table in ["stream_state", "event_idempotency", "event_log"] {
                sqlx::raw_sql(&format!("DROP TRIGGER IF EXISTS capability_fail ON {table}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
    }
    proxied.close().await;
}

#[tokio::test]
async fn closed_legacy_approval_gates_precede_consumption_without_io() {
    use robson_exec::order_journal::StreamSuspensionRow;
    for halted in [false, true] {
        let f = fixture();
        let p = position(Side::Short);
        f.manager.store.positions().save(&p).await.unwrap();
        let id = pending(&f, &p, Side::Short, Side::Short).await;
        if halted {
            f.manager.circuit_breaker.trigger_halt("capability fixture".into()).await;
        } else {
            f.manager.order_journal().record_suspension(StreamSuspensionRow {
                suspension_id: Uuid::now_v7(),
                stream_key: "position:capability".into(),
                stream_kind: "position",
                failed_seq: None,
                month: None,
                error: "fixture".into(),
                epoch_id: None,
                suspended_at: chrono::Utc::now(),
            });
        }
        let before = f.manager.order_journal().statements_total();
        let result = f.manager.approve_query(id).await;
        if halted {
            assert!(matches!(result, Err(DaemonError::MonthlyHaltActive { .. })));
        } else {
            assert!(matches!(result, Err(DaemonError::AdmissionsClosed { .. })));
        }
        assert!(f.manager.pending_approvals.read().await.contains_key(&id));
        assert!(f.recorder.calls.lock().unwrap().is_empty());
        assert_eq!(f.manager.order_journal().statements_total(), before);
        unchanged(&f, &p).await;
    }
}

#[tokio::test]
async fn recovered_empty_armed_leg_does_not_discharge_unresolved_journal_evidence() {
    use robson_exec::{
        order_journal::{IntentKind, IntentSpec, OrderJournal, StubJournalBackend, WriteClass},
        venue::AttemptEvidence,
    };
    let backend = Arc::new(StubJournalBackend::default());
    let journal = Arc::new(OrderJournal::new(backend.clone(), &VenueId::new("fixture:recovered")));
    let p = position(Side::Short);
    let operation = journal.begin_operation();
    operation
        .scope(async {
            let intent = journal.open_intent(IntentSpec {
                position_id: Some(p.id),
                kind: IntentKind::Entry,
                client_order_id: "unresolved-history".into(),
                cancels_order_id: None,
                params: serde_json::json!({"symbol": p.symbol.as_pair()}),
            });
            journal.before_send(&intent, WriteClass::Strict).await.unwrap();
        })
        .await;
    drop(operation);
    assert!(matches!(
        journal.attempt_evidence("unresolved-history").await,
        AttemptEvidence::Unresolved { .. }
    ));
    let row = backend.row("unresolved-history").unwrap();
    assert_eq!(row.attempts.len(), 1);
    assert!(row.outcome.is_none());
    let before = journal.statements_total();
    let f = fixture_with_journal(Arc::new(Recorder::default()), Some(journal.clone()));
    f.manager.store.positions().save(&p).await.unwrap();
    assert!(!p.carries_admission_evidence());
    assert!(p.entry_order_id.is_none());
    assert!(matches!(
        f.manager.restore_armed_position(&p, EntryPolicyConfig::default()).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    assert!(matches!(
        f.manager.handle_signal(signal(&p, Side::Short).await).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    unchanged(&f, &p).await;
    assert_eq!(journal.statements_total(), before);
    let after = backend.row("unresolved-history").unwrap();
    assert_eq!(after.attempts, row.attempts);
    assert_eq!(after.intent, row.intent);
    assert!(after.outcome.is_none());
    assert!(matches!(
        journal.attempt_evidence("unresolved-history").await,
        AttemptEvidence::Unresolved { .. }
    ));
}

#[tokio::test]
async fn unsupported_non_armed_work_retains_its_owner_and_emits_visible_refusal() {
    let f = fixture();
    let mut p = position(Side::Short);
    let signal = signal(&p, Side::Short).await;
    p.state = PositionState::Entering {
        entry_order_id: Uuid::now_v7(),
        expected_entry: signal.entry_price,
        signal_id: signal.signal_id,
    };
    p.quantity = Quantity::new(dec!(2)).unwrap();
    p.fees_paid = dec!(0.03);
    f.manager.store.positions().save(&p).await.unwrap();
    let mut events = f.manager.event_bus.subscribe();
    assert!(matches!(
        f.manager.handle_signal(signal).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .unwrap();
    assert!(
        matches!(event, Some(Ok(DaemonEvent::EntryRejected { position_id, terminal: false, ref reason_code, .. })) if position_id == p.id && reason_code == "venue_unsupported")
    );
    unchanged(&f, &p).await;
    assert!(f.recorder.calls.lock().unwrap().is_empty());
    let id = pending(&f, &p, Side::Long, Side::Long).await;
    assert!(matches!(
        f.manager.approve_query(id).await,
        Err(DaemonError::VenueUnsupported { .. })
    ));
    unchanged(&f, &p).await;
}
