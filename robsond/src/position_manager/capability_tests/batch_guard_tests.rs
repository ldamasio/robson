//! The legacy balance boundary is atomic for the original batch and preserves
//! caller lifecycle audits. All stop prices below come from the chart fixture.
use robson_domain::{ExitReason, OrderSide, StopPolicy};
use robson_exec::{
    order_journal::{OrderJournal, StubJournalBackend},
    StubExchange,
};

use super::*;

fn forbidden(p: &Position, signal: &DetectorSignal) -> Vec<EngineAction> {
    let qty = Quantity::new(dec!(1)).unwrap();
    vec![
        EngineAction::PlaceEntryOrder {
            position_id: p.id,
            cycle_id: Some(Uuid::now_v7()),
            symbol: p.symbol.clone(),
            side: OrderSide::Buy,
            quantity: qty,
            order_id: Uuid::now_v7(),
            client_order_id: signal.signal_id.to_string(),
            expected_price: signal.entry_price,
            signal_id: signal.signal_id,
        },
        EngineAction::PlaceExitOrder {
            position_id: p.id,
            cycle_id: Some(Uuid::now_v7()),
            symbol: p.symbol.clone(),
            side: OrderSide::Sell,
            quantity: qty,
            reason: ExitReason::UserPanic,
        },
        EngineAction::PlaceInsuranceStop {
            position_id: p.id,
            symbol: p.symbol.clone(),
            side: OrderSide::Sell,
            quantity: qty,
            stop_price: signal.stop_loss,
        },
        EngineAction::ReplaceInsuranceStop {
            position_id: p.id,
            symbol: p.symbol.clone(),
            side: OrderSide::Sell,
            quantity: qty,
            previous_order_id: "owned-protection".into(),
            new_stop_price: signal.stop_loss,
        },
        EngineAction::CancelInsuranceStop {
            position_id: p.id,
            symbol: p.symbol.clone(),
            order_id: "owned-protection".into(),
        },
    ]
}

fn entry_event(p: &Position, signal: &DetectorSignal) -> EngineAction {
    EngineAction::EmitEvent(Event::EntrySignalReceived {
        position_id: p.id,
        signal_id: signal.signal_id,
        entry_price: signal.entry_price,
        stop_loss: signal.stop_loss,
        quantity: Quantity::new(dec!(1)).unwrap(),
        initial_executable_stop: None,
        executable_span: None,
        cap_basis_distance: None,
        tick_size: None,
        rules_snapshot: None,
        timestamp: chrono::Utc::now(),
    })
}

fn counted_fixture() -> (Fixture, Arc<OrderJournal>) {
    let journal = Arc::new(OrderJournal::new(
        Arc::new(StubJournalBackend::default()),
        &VenueId::new("fixture:batch"),
    ));
    (
        fixture_with_journal(Arc::new(Recorder::default()), Some(journal.clone())),
        journal,
    )
}

async fn atomic_case(which: usize, prefix: usize, suffix: usize, ambient: bool, manager: bool) {
    let (f, journal) = counted_fixture();
    let p = position(Side::Long);
    let signal = signal(&p, p.side).await;
    f.manager.store.positions().save(&p).await.unwrap();
    let mut actions = vec![entry_event(&p, &signal); prefix];
    actions.push(forbidden(&p, &signal).remove(which));
    actions.extend(vec![entry_event(&p, &signal); suffix]);
    let run = async {
        if manager {
            f.manager.execute_and_persist(actions).await
        } else {
            f.manager.executor.execute(actions).await.map_err(DaemonError::from)
        }
    };
    let result = if ambient {
        let operation = journal.begin_operation();
        operation.scope(run).await
    } else {
        run.await
    };
    assert!(
        matches!(result, Err(DaemonError::Exec(ExecError::BalanceOwnerRequired))),
        "{result:?}"
    );
    unchanged(&f, &p).await;
    assert!(f.manager.store.events().find_by_position(p.id).await.unwrap().is_empty());
    assert_eq!(journal.statements_total(), 0);
    assert_eq!(journal.operations_in_flight(), 0);
    if !ambient {
        assert_eq!(
            f.manager.exchange().capability_scopes(),
            0,
            "guard must precede opening an operation"
        );
    }
    assert!(f.recorder.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn every_legacy_balance_trading_action_refuses_original_batch() {
    for action in 0..5 {
        for ambient in [false, true] {
            for manager in [false, true] {
                atomic_case(action, 1, 1, ambient, manager).await;
            }
        }
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(32))]
    #[test]
    fn generated_balance_batches_have_no_partial_effects(action in 0usize..5, prefix in 0usize..4, suffix in 0usize..4, ambient in proptest::bool::ANY, manager in proptest::bool::ANY) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(atomic_case(action,prefix,suffix,ambient,manager));
    }
}

#[tokio::test]
async fn event_and_state_only_balance_batches_remain_allowed() {
    let f = fixture();
    let p = position(Side::Long);
    let s = signal(&p, p.side).await;
    let state_only = [
        EngineAction::UpdateTrailingStop {
            position_id: p.id,
            previous_stop: s.stop_loss,
            new_stop: s.stop_loss,
            trigger_price: s.entry_price,
        },
        EngineAction::TriggerExit {
            position_id: p.id,
            reason: ExitReason::UserPanic,
            trigger_price: s.entry_price,
            stop_price: s.stop_loss,
        },
    ];
    assert!(f.manager.executor.validate_legacy_batch(&state_only).is_ok());
    let arm = Event::PositionArmed {
        position_id: p.id,
        account_id: p.account_id,
        symbol: p.symbol.clone(),
        side: p.side,
        timestamp: chrono::Utc::now(),
        tech_stop_distance: None,
        stop_policy: StopPolicy::LegacyUncapped,
        stop_buffer_bps_at_arm: None,
    };
    f.manager.execute_and_persist(vec![EngineAction::EmitEvent(arm)]).await.unwrap();
    assert!(f.manager.get_position(p.id).await.unwrap().is_some());
    assert_eq!(f.manager.store.events().find_by_position(p.id).await.unwrap().len(), 1);
    assert_eq!(f.manager.exchange().requests(), 0);
}

async fn governed(
    query: &mut ExecutionQuery,
    actions: Vec<EngineAction>,
    p: &Position,
    signal: &DetectorSignal,
) -> (GovernedAction, ProposedTrade) {
    let quantity = actions
        .iter()
        .find_map(|action| match action {
            EngineAction::PlaceEntryOrder { quantity, .. } => Some(quantity.as_decimal()),
            _ => None,
        })
        .unwrap();
    let proposed = ProposedTrade {
        symbol: p.symbol.as_pair(),
        side: "long".into(),
        quantity,
        entry_price: signal.entry_price.as_decimal(),
        notional_value: signal.entry_price.as_decimal() * quantity,
        initial_margin: signal.entry_price.as_decimal() * quantity,
        planned_risk: (signal.entry_price.as_decimal() - signal.stop_loss.as_decimal()).abs()
            * quantity,
    };
    query.transition(QueryState::Processing).unwrap();
    let engine = QueryEngine::new(Arc::new(TracingQueryRecorder), RiskGate::new());
    let action = engine
        .check_risk(query, &proposed, &RiskContext::new(dec!(1000000)), actions)
        .await
        .unwrap();
    (action, proposed)
}

#[tokio::test]
async fn governed_balance_batch_refuses_before_acting_and_admission() {
    let (f, journal) = counted_fixture();
    let p = position(Side::Long);
    let s = signal(&p, p.side).await;
    f.manager.store.positions().save(&p).await.unwrap();
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessSignal {
            signal_id: s.signal_id,
            symbol: p.symbol.clone(),
            side: p.side,
            entry_price: s.entry_price,
            stop_loss: s.stop_loss,
        },
        ActorKind::Detector,
    );
    query.position_id = Some(p.id);
    let actions = vec![entry_event(&p, &s), forbidden(&p, &s).remove(0)];
    let (action, _) = governed(&mut query, actions, &p, &s).await;
    let prior = query.state.clone();
    let mut reach = AdmissionReach::DetectorSpent(p.id);
    assert!(matches!(
        f.manager.execute_signal_query(&mut query, action, &mut reach).await,
        Err(DaemonError::Exec(ExecError::BalanceOwnerRequired))
    ));
    assert_eq!(query.state, prior);
    assert!(matches!(reach,AdmissionReach::DetectorSpent(id) if id==p.id));
    assert!(f.recorder.calls.lock().unwrap().is_empty());
    assert_eq!(journal.statements_total(), 0);
    unchanged(&f, &p).await;
}

fn configure_rules(f: &Fixture, p: &Position) {
    f.manager.exchange().set_trading_rules(
        robson_domain::SymbolTradingRules::new(
            p.symbol.clone(),
            dec!(0.01),
            Decimal::ZERO,
            dec!(0.001),
            dec!(0.001),
            dec!(1000000),
            Decimal::ZERO,
            2,
            3,
        )
        .unwrap(),
    );
}

async fn pending_long(f: &Fixture, p: &Position) -> Uuid {
    let s = signal(p, p.side).await;
    let mut query = ExecutionQuery::new(
        QueryKind::ProcessSignal {
            signal_id: s.signal_id,
            symbol: p.symbol.clone(),
            side: p.side,
            entry_price: s.entry_price,
            stop_loss: s.stop_loss,
        },
        ActorKind::Detector,
    );
    query.position_id = Some(p.id);
    let mut actions = forbidden(p, &s);
    let mut action = actions.remove(0);
    if let EngineAction::PlaceEntryOrder { quantity, .. } = &mut action {
        *quantity = Quantity::new(dec!(0.001)).unwrap();
    }
    let (governed, proposed) = governed(&mut query, vec![action], p, &s).await;
    query.await_approval("batch guard fixture".into(), 60).unwrap();
    let id = query.id;
    f.manager.pending_approvals.write().await.insert(id, PendingApprovalRecord {
        query,
        position: p.clone(),
        proposed,
        governed,
    });
    id
}

async fn preserved_after_preparation(f: &Fixture, p: &Position, requests: usize) {
    assert_eq!(
        serde_json::to_value(f.manager.get_position(p.id).await.unwrap().unwrap()).unwrap(),
        serde_json::to_value(p).unwrap()
    );
    assert!(f.manager.detectors.read().await.is_empty());
    assert!(f.manager.governed_rearm_counts.read().await.is_empty());
    assert!(!f.manager.is_entry_exhausted(p.id).await);
    assert_eq!(f.candles.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.manager.exchange().requests(),
        requests,
        "record earlier preparation separately; no post-refusal read/send"
    );
    assert!(f.manager.store.events().find_by_position(p.id).await.unwrap().is_empty());
    assert!(!f.recorder.calls.lock().unwrap().iter().any(|c| c == "acting" || c == "failed"));
    let expected: &[&str] = if requests == 1 {
        &["authorized"]
    } else {
        &["accepted", "processing", "risk_checked"]
    };
    assert_eq!(
        *f.recorder.calls.lock().unwrap(),
        expected,
        "preparation snapshots remain explicit"
    );
}

#[tokio::test]
async fn balance_long_signal_and_approval_refuse_without_rearm() {
    for approval in [false, true] {
        let (f, journal) = counted_fixture();
        let p = position(Side::Long);
        f.manager.store.positions().save(&p).await.unwrap();
        configure_rules(&f, &p);
        let result = if approval {
            let id = pending_long(&f, &p).await;
            f.manager.approve_query(id).await.map(|_| ())
        } else {
            f.manager.handle_signal(signal(&p, p.side).await).await
        };
        assert!(
            matches!(result, Err(DaemonError::Exec(ExecError::BalanceOwnerRequired))),
            "approval={approval}: {result:?}"
        );
        preserved_after_preparation(&f, &p, if approval { 1 } else { 2 }).await;
        assert!(f.manager.pending_approvals.read().await.is_empty());
        assert_eq!(journal.statements_total(), 0);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(24))]
    #[test]
    fn concurrent_balance_approvals_consume_once_without_rearm(first_yields in 0usize..5,second_yields in 0usize..5) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let f=fixture(); let p=position(Side::Long); f.manager.store.positions().save(&p).await.unwrap(); configure_rules(&f,&p);
            let id=pending_long(&f,&p).await;
            let one=f.manager.clone(); let two=f.manager.clone();
            let first=tokio::spawn(async move {for _ in 0..first_yields {tokio::task::yield_now().await;} one.approve_query(id).await});
            let second=tokio::spawn(async move {for _ in 0..second_yields {tokio::task::yield_now().await;} two.approve_query(id).await});
            let (first,second)=tokio::time::timeout(std::time::Duration::from_secs(2),async {tokio::join!(first,second)}).await.unwrap();
            let results=[first.unwrap(),second.unwrap()];
            assert_eq!(results.iter().filter(|r| matches!(r,Err(DaemonError::Exec(ExecError::BalanceOwnerRequired)))).count(),1);
            assert_eq!(results.iter().filter(|r| matches!(r,Err(DaemonError::QueryNotFound(_)))).count(),1);
            preserved_after_preparation(&f,&p,1).await;
        });
    }
}

async fn active(f: &Fixture) -> (Position, Price) {
    let mut p = position(Side::Long);
    let s = signal(&p, p.side).await;
    let now = chrono::Utc::now();
    p.entry_price = Some(s.entry_price);
    p.entry_filled_at = Some(now);
    p.quantity = Quantity::new(dec!(0.001)).unwrap();
    p.tech_stop_distance = Some(robson_domain::TechnicalStopDistance::from_entry_and_stop(
        s.entry_price,
        s.stop_loss,
    ));
    p.fees_paid = dec!(0.01);
    p.state = PositionState::Active {
        current_price: s.entry_price,
        trailing_stop: s.stop_loss,
        favorable_extreme: s.entry_price,
        extreme_at: now,
        insurance_stop_id: Some("owned-insurance".into()),
        invalidation_guard_level: None,
        last_emitted_stop: Some(s.stop_loss),
    };
    f.manager.store.positions().save(&p).await.unwrap();
    (p, s.stop_loss)
}

async fn call_exit(f: &Fixture, p: &Position, stop: Price, kind: &str) -> DaemonResult<()> {
    match kind {
        "close" => f.manager.close_position(p.id).await,
        "tick" => {
            f.manager
                .process_market_data(MarketData {
                    venue: f.manager.venue.clone(),
                    symbol: p.symbol.clone(),
                    price: stop,
                    timestamp: chrono::Utc::now(),
                    source: crate::event_bus::MarketDataSource::Ws,
                })
                .await
        },
        "panic" => {
            assert!(
                f.manager.panic_close_all().await?.is_empty(),
                "refused leg must not be reported closed"
            );
            Ok(())
        },
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn balance_close_tick_and_panic_keep_one_failed_snapshot() {
    for kind in ["close", "tick", "panic"] {
        let f = fixture();
        let (p, stop) = active(&f).await;
        let result = call_exit(&f, &p, stop, kind).await;
        if kind == "panic" {
            assert!(result.is_ok());
        } else {
            assert!(
                matches!(result, Err(DaemonError::Exec(ExecError::BalanceOwnerRequired))),
                "{kind}: {result:?}"
            );
        }
        unchanged(&f, &p).await;
        assert_eq!(
            *f.recorder.calls.lock().unwrap(),
            ["accepted", "processing", "acting", "failed"],
            "{kind}"
        );
        assert!(f.manager.exit_retry_after.read().await.is_empty());
        assert!(f.manager.exit_overtaken_suspects.read().await.is_empty());
        assert!(f.manager.store.events().find_by_position(p.id).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn position_account_entry_and_exit_still_require_futures_settings() {
    for entry in [false, true] {
        for hedge in [false, true] {
            let p = position(Side::Long);
            let s = signal(&p, p.side).await;
            let exchange = Arc::new(StubExchange::new(s.entry_price.as_decimal()));
            exchange.set_futures_settings(
                if hedge { "Hedge" } else { "OneWay" },
                if hedge { 1 } else { 2 },
            );
            let store = Arc::new(MemoryStore::new());
            store.positions().save(&p).await.unwrap();
            let executor = Executor::new(exchange.clone(), Arc::new(IntentJournal::new()), store);
            let action = forbidden(&p, &s).remove(if entry { 0 } else { 1 });
            assert!(executor.validate_legacy_batch(std::slice::from_ref(&action)).is_ok());
            let result = executor.execute(vec![action]).await;
            if entry {
                assert!(
                    matches!(result,Ok(ref r) if matches!(r.as_slice(),[ActionResult::EntryExecutionRejected{error,..}] if error.contains("FUTURES SAFETY VIOLATION"))),
                    "{result:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(ExecError::FuturesSafetyViolation { .. })),
                    "{result:?}"
                );
            }
            assert_eq!(exchange.market_order_call_count(), 0);
        }
    }
}

#[cfg(feature = "postgres")]
mod pg;

struct FailedAudit {
    fail: bool,
    entered: tokio::sync::Notify,
}

#[async_trait]
impl QueryRecorder for FailedAudit {
    async fn record_transition(
        &self,
        _: &ExecutionQuery,
        cause: &str,
    ) -> Result<(), QueryRecorderError> {
        if cause == "failed" {
            self.entered.notify_one();
            if self.fail {
                return Err(serde_json::from_str::<serde_json::Value>("invalid")
                    .unwrap_err()
                    .into());
            }
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn balance_refusal_audit_error_or_cancellation_preserves_exit_owner() {
    for kind in ["close", "tick", "panic"] {
        for fail in [false, true] {
            let audit = Arc::new(FailedAudit {
                fail,
                entered: tokio::sync::Notify::new(),
            });
            let f = Arc::new(fixture_with_recorder(Arc::new(Recorder {
                delegate: Some(audit.clone()),
                ..Recorder::default()
            })));
            let (p, stop) = active(&f).await;
            let task_f = f.clone();
            let task_p = p.clone();
            let task = tokio::spawn(async move { call_exit(&task_f, &task_p, stop, kind).await });
            tokio::time::timeout(std::time::Duration::from_secs(2), audit.entered.notified())
                .await
                .unwrap();
            if fail {
                let result = task.await.unwrap();
                if kind == "panic" {
                    assert!(result.is_ok());
                } else {
                    assert!(matches!(result, Err(DaemonError::QueryAudit(_))), "{result:?}");
                }
            } else {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            }
            unchanged(&f, &p).await;
            assert_eq!(*f.recorder.calls.lock().unwrap(), [
                "accepted",
                "processing",
                "acting",
                "failed"
            ]);
        }
    }
}


proptest::proptest! {
    #[test]
    fn batch_policy_depends_only_on_account_kind(balance in proptest::bool::ANY,spot in proptest::bool::ANY,supports_short in proptest::bool::ANY) {
        let mut capabilities=robson_testkit::balance_stub::spot_capabilities();
        capabilities.account_kind=if balance {robson_exec::venue::AccountKind::BalanceAccount}else{robson_exec::venue::AccountKind::PositionAccount};
        capabilities.market_kind=if spot {robson_exec::venue::MarketKind::Spot}else{robson_exec::venue::MarketKind::PerpUsdM};
        capabilities.supports_short=supports_short;
        let exchange=Arc::new(RestrictedExchange::new().with_capabilities(capabilities));
        let executor=Executor::new(exchange.clone(),Arc::new(IntentJournal::new()),Arc::new(MemoryStore::new()));
        let action=EngineAction::CancelInsuranceStop {position_id:Uuid::nil(),symbol:Symbol::from_pair("SOLUSDT").unwrap(),order_id:"owned-protection".into()};
        proptest::prop_assert_eq!(executor.validate_legacy_batch(&[action]).is_err(),balance);
        proptest::prop_assert_eq!(exchange.requests(),0);
    }
}
