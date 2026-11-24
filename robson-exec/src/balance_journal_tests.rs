//! Balance journal wiring, without a database or venue (ADR-0056 step 8e-2).
use std::sync::atomic::AtomicUsize;

use proptest::prelude::*;
use rust_decimal_macros::dec;

use super::*;
use crate::venue::SubmitAttemptLedger;

const BOUND: Duration = Duration::from_millis(100);

fn journal(balance: bool) -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
    let backend = Arc::new(StubJournalBackend::default());
    let journal = Arc::new(
        OrderJournal::new(backend.clone(), &VenueId::new("test:balance"))
            .with_commit_bound(BOUND)
            .with_one_attempt_per_client_id(balance)
            .with_balance_account(balance),
    );
    (backend, journal)
}

fn spec(id: &str, kind: IntentKind) -> IntentSpec {
    IntentSpec {
        position_id: None,
        kind,
        client_order_id: id.to_string(),
        cancels_order_id: None,
        params: Value::Null,
    }
}

fn admission() -> AdmissionRow {
    AdmissionRow {
        position_id: Uuid::now_v7(),
        symbol: "ETHUSDT".to_string(),
        decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        taker_bps: dec!(10),
        gap_bps: dec!(5),
        notional_margin_bps: dec!(2),
        min_notional: dec!(10),
        lot_step: dec!(0.001),
        fee_asset_policy: "base".to_string(),
        envelope_source: "test metadata".to_string(),
        envelope_as_of: Utc::now(),
        envelope_hash: "test-envelope".to_string(),
        planned_qty: dec!(1),
        requested_qty: dec!(1),
        reference_price: dec!(3000),
        remainder_reserve: dec!(0.01),
        admission_charge: dec!(30),
        initial_trigger: dec!(2980),
    }
}

fn entry_spec(id: &str, admission: &AdmissionRow) -> IntentSpec {
    IntentSpec {
        position_id: Some(admission.position_id),
        ..spec(id, IntentKind::Entry)
    }
}

#[tokio::test]
async fn a_journal_off_the_balance_flag_carries_no_balance_rows() {
    let (backend, journal) = journal(false);
    assert!(!journal.balance_account());
    let op = journal.begin_operation();
    op.scope(async {
        let intent = journal.open_intent(spec("exit", IntentKind::Exit));
        journal.before_send(&intent, WriteClass::Strict).await.unwrap();
        journal.finish_success(&intent, None).await;
    })
    .await;
    assert_eq!(backend.commits().len(), 2);
    assert!(backend.commits().iter().all(|batch| batch.balance.is_none()));
    assert_eq!(journal.statements_total(), 11);
}

#[tokio::test]
async fn a_balance_journal_carries_its_rows_on_every_batch() {
    let (backend, journal) = journal(true);
    let op = journal.begin_operation();
    op.scope(async {
        for (index, kind) in [
            IntentKind::Entry,
            IntentKind::Exit,
            IntentKind::Protection,
            IntentKind::SweepClose,
            IntentKind::Cancel,
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("intent-{index}");
            let terms = admission();
            let intent = if kind == IntentKind::Entry {
                journal.open_balance_entry(entry_spec(&id, &terms), terms.clone()).unwrap()
            } else {
                journal.open_intent(spec(&id, kind))
            };
            journal.before_send(&intent, WriteClass::Strict).await.unwrap();
            backend.push_fault(JournalFault::Known);
            journal.finish_success(&intent, None).await;
            journal.flush_pending().await;
            let batches: Vec<_> = backend
                .commits()
                .into_iter()
                .filter(|batch| batch.intent.client_order_id == id)
                .collect();
            assert_eq!(batches.len(), 3, "pre-send, failed outcome, replay");
            for batch in batches {
                let balance = batch.balance.unwrap();
                assert_eq!(balance.commit_bound, BOUND);
                assert_eq!(
                    balance.admission.as_deref(),
                    (kind == IntentKind::Entry).then_some(&terms)
                );
                assert!(!balance.handed_off, "no ticket was issued");
            }
        }
    })
    .await;
}

proptest! {
    #[test]
    fn the_statement_formula_covers_every_batch_shape(attempts in 0..64usize, outcome: bool, balance: bool, admit: bool) {
        let (_, journal) = journal(balance);
        let terms = admission();
        let intent = if balance && admit {
            journal.open_balance_entry(entry_spec("formula", &terms), terms).unwrap()
        } else { journal.open_intent(spec("formula", IntentKind::Exit)) };
        let mut batch = journal.lock().intents[&intent.client_order_id].batch();
        batch.attempts = (0..attempts).map(|_| AttemptRow { attempt_id: Uuid::now_v7(), created_at: Utc::now(), rejected_at: None }).collect();
        batch.outcome = outcome.then(OutcomeRow::default);
        // BEGIN, the certificate's key, intent INSERT, COMMIT; optional attempt
        // INSERT and outcome UPDATE.
        let mut expected = 4;
        if attempts > 0 { expected += 1; }
        if outcome { expected += 1; }
        if balance {
            expected += 2; // timeouts and accounting UPDATE
            if attempts > 0 { expected += 1; } // clock INSERT
            if admit { expected += 1; } // admission function
        }
        prop_assert_eq!(batch.statements(), expected);
    }
}

#[tokio::test]
async fn handed_off_needs_a_ticket_and_an_outcome_that_is_not_not_sent() {
    let (backend, journal) = journal(true);
    let op = journal.begin_operation();
    let ledger = SubmitAttemptLedger::one_attempt(16);
    ledger.attach_journal(journal.clone());
    op.scope(async {
        let terms = admission();
        let lost = journal.open_balance_entry(entry_spec("lost", &terms), terms).unwrap();
        backend.push_fault(JournalFault::AckLost);
        assert!(matches!(
            journal.authorize_send(&lost, WriteClass::Strict).await,
            Err(SendRefusal::UncertainCommit(_))
        ));
        assert!(!journal.lock().intents["lost"].ticket_issued);
        journal.finish_failed(&lost).await;
        let lost_row = backend.row("lost").unwrap();
        assert!(lost_row.outcome.unwrap().uncertain_at.is_some());
        assert!(!lost_row.balance.unwrap().handed_off);

        for (id, not_sent, success) in [
            ("not-sent", true, false),
            ("success", false, true),
            ("uncertain", false, false),
        ] {
            let intent = journal.open_intent(spec(id, IntentKind::Exit));
            let guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            assert!(!journal.lock().intents[id].batch().balance.unwrap().handed_off);
            ledger
                .submit(id, |_: &ExecError| false, async { Ok::<(), ExecError>(()) })
                .await
                .unwrap();
            assert!(journal.lock().intents[id].ticket_issued);
            assert!(
                !journal.lock().intents[id].batch().balance.unwrap().handed_off,
                "a ticket without an outcome is insufficient"
            );
            if success {
                journal.finish_success(&intent, None).await;
            } else if not_sent {
                journal
                    .write_outcome(&intent, OutcomeRow { not_sent: true, ..OutcomeRow::default() })
                    .await;
            } else {
                journal.finish_failed(&intent).await;
            }
            assert_eq!(backend.row(id).unwrap().balance.unwrap().handed_off, !not_sent);
            drop(guard);
        }
    })
    .await;
}

#[tokio::test]
async fn an_admission_rides_only_its_entry() {
    let (_, off) = journal(false);
    let terms = admission();
    assert!(off.open_balance_entry(entry_spec("off", &terms), terms.clone()).is_err());
    assert!(off.lock().intents.is_empty());
    let (backend, journal) = journal(true);
    let op = journal.begin_operation();
    op.scope(async {
        for kind in [
            IntentKind::Exit,
            IntentKind::Protection,
            IntentKind::Cancel,
            IntentKind::SweepClose,
        ] {
            let mut wrong = entry_spec("wrong", &terms);
            wrong.kind = kind;
            assert!(journal.open_balance_entry(wrong, terms.clone()).is_err());
        }
        assert!(journal
            .open_balance_entry(spec("missing-position", IntentKind::Entry), terms.clone())
            .is_err());
        let mut wrong_position = entry_spec("wrong-position", &terms);
        wrong_position.position_id = Some(Uuid::now_v7());
        assert!(journal.open_balance_entry(wrong_position, terms.clone()).is_err());
        assert!(journal.lock().intents.is_empty());
        let entry = journal.open_balance_entry(entry_spec("entry", &terms), terms.clone()).unwrap();
        let mut changed = terms.clone();
        changed.admission_charge += Decimal::ONE;
        let duplicate = journal.open_balance_entry(entry_spec("entry", &changed), changed).unwrap();
        assert!(journal.before_send(&duplicate, WriteClass::Strict).await.is_err());
        assert!(backend.commits().is_empty());
        journal.before_send(&entry, WriteClass::Strict).await.unwrap();
        assert_eq!(
            backend.row("entry").unwrap().balance.unwrap().admission.as_deref(),
            Some(&terms)
        );
        let unrelated = journal.open_intent(spec("unrelated", IntentKind::Exit));
        journal.before_send(&unrelated, WriteClass::Strict).await.unwrap();
        assert!(backend.row("unrelated").unwrap().balance.unwrap().admission.is_none());
    })
    .await;
}

#[tokio::test]
async fn a_balance_entry_without_its_admission_is_not_sent() {
    let (backend, journal) = journal(true);
    let op = journal.begin_operation();
    op.scope(async {
        let intent = journal.open_intent(spec("unadmitted", IntentKind::Entry));
        for class in [WriteClass::Strict, WriteClass::Bounded] {
            assert!(matches!(
                journal.authorize_send(&intent, class).await,
                Err(SendRefusal::NotSent(_))
            ));
        }
        assert!(journal.lock().intents["unadmitted"].attempts.is_empty());
        assert!(backend.commits().is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_spent_id_never_sends_a_cancel_on_a_balance_journal() {
    for balance in [false, true] {
        for orphan in [false, true] {
            let (backend, journal) = journal(balance);
            let op = journal.begin_operation();
            let polled = AtomicUsize::new(0);
            backend.push_fault(JournalFault::IdSpent);
            op.scope(async {
                let cancel = async {
                    polled.fetch_add(1, Ordering::SeqCst);
                    Ok(StopCancelOutcome::Cancelled {
                        filled_before_cancel: Decimal::ZERO,
                        fills: None,
                    })
                };
                if orphan {
                    let result = journal.journaled_orphan_cancel("ins-1", cancel).await;
                    assert_eq!(matches!(result, OrphanCancel::Withheld), balance);
                } else {
                    let result = journal.journaled_cancel(None, "ins-1", cancel).await;
                    assert_eq!(
                        matches!(result, CancelClass::Errored(ExecError::SendRefused(_))),
                        balance
                    );
                }
                assert_eq!(polled.load(Ordering::SeqCst), usize::from(!balance));
                assert_eq!(backend.commits().len(), if balance { 1 } else { 2 });
                if balance {
                    assert!(backend.rows().is_empty());
                    assert!(backend.incidents().is_empty());
                    assert!(journal.lock().intents.values().all(|memory| memory.outcome.is_none()));
                }
            })
            .await;
        }
    }
}

#[tokio::test]
async fn other_refusals_preserve_bounded_cancel_behaviour() {
    for fault in [JournalFault::Known, JournalFault::NotLandedUncertain] {
        for orphan in [false, true] {
            let (backend, journal) = journal(true);
            let op = journal.begin_operation();
            backend.push_fault(fault);
            let polled = AtomicUsize::new(0);
            op.scope(async {
                let cancel = async {
                    polled.fetch_add(1, Ordering::SeqCst);
                    Ok(StopCancelOutcome::Cancelled {
                        filled_before_cancel: Decimal::ZERO,
                        fills: None,
                    })
                };
                if orphan {
                    assert!(matches!(
                        journal.journaled_orphan_cancel("ins-1", cancel).await,
                        OrphanCancel::Sent(_)
                    ));
                } else {
                    assert!(matches!(
                        journal.journaled_cancel(None, "ins-1", cancel).await,
                        CancelClass::Cancelled { .. }
                    ));
                }
            })
            .await;
            assert_eq!(polled.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn readiness_checks_the_balance_schema_only_under_the_flags() {
    for balance in [false, true] {
        for one_attempt in [false, true] {
            let backend = Arc::new(StubJournalBackend::default());
            let journal = OrderJournal::new(backend.clone(), &VenueId::new("test:venue"))
                .with_balance_account(balance)
                .with_one_attempt_per_client_id(one_attempt);
            journal.check_ready().await.unwrap();
            assert_eq!(
                backend.balance_schema_checks(),
                if balance || one_attempt {
                    vec![("test:venue".to_string(), one_attempt)]
                } else {
                    vec![]
                }
            );
            backend.set_balance_schema_error(Some("missing migration".to_string()));
            assert_eq!(journal.check_ready().await.is_err(), balance || one_attempt);
        }
    }
}

#[tokio::test]
async fn a_spent_cancel_stays_withheld_when_its_history_disappears_during_commit() {
    for orphan in [false, true] {
        let (backend, journal) = journal(true);
        let hold = backend.hold_commit(0);
        backend.push_fault(JournalFault::IdSpent);
        let polled = AtomicUsize::new(0);
        let op = journal.begin_operation();
        let work = op.scope(async {
            let cancel = async {
                polled.fetch_add(1, Ordering::SeqCst);
                Ok(StopCancelOutcome::Indeterminate)
            };
            if orphan {
                assert!(matches!(
                    journal.journaled_orphan_cancel("ins-1", cancel).await,
                    OrphanCancel::Withheld
                ));
            } else {
                assert!(matches!(
                    journal.journaled_cancel(None, "ins-1", cancel).await,
                    CancelClass::Errored(ExecError::SendRefused(_))
                ));
            }
        });
        let removal = async {
            while backend.commits().is_empty() {
                tokio::task::yield_now().await;
            }
            journal.lock().intents.clear();
            hold.notify_one();
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(work, removal);
        })
        .await
        .unwrap();
        assert_eq!(polled.load(Ordering::SeqCst), 0);
        assert!(backend.rows().is_empty());
        assert_eq!(backend.commits().len(), 1);
    }
}

#[tokio::test]
async fn admission_replays_are_immutable_and_handoff_is_monotone() {
    let (backend, journal) = journal(true);
    let terms = admission();
    let intent = journal.open_balance_entry(entry_spec("entry", &terms), terms).unwrap();
    let original = journal.lock().intents[&intent.client_order_id].batch();
    backend.commit(&original).await.unwrap();
    let mut handed_off = original.clone();
    handed_off.balance.as_mut().unwrap().handed_off = true;
    backend.commit(&handed_off).await.unwrap();
    backend.commit(&original).await.unwrap();
    assert!(backend.row("entry").unwrap().balance.unwrap().handed_off);
    let before = backend.row("entry").unwrap();
    let mut changed = original;
    Arc::make_mut(changed.balance.as_mut().unwrap().admission.as_mut().unwrap()).requested_qty +=
        Decimal::ONE;
    assert!(matches!(backend.commit(&changed).await, Err(CommitFailure::Known(_))));
    assert_eq!(backend.row("entry").unwrap(), before);
}

proptest! {
    #[test]
    fn concurrent_admissions_never_replace_each_others_terms(left_yields in 0..16usize, right_yields in 0..16usize) {
        let (_, journal) = journal(true);
        let left = admission();
        let mut right = left.clone();
        right.admission_charge += Decimal::ONE;
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                for _ in 0..left_yields { std::thread::yield_now(); }
                journal.open_balance_entry(entry_spec("same-id", &left), left.clone()).unwrap()
            });
            let b = scope.spawn(|| {
                barrier.wait();
                for _ in 0..right_yields { std::thread::yield_now(); }
                journal.open_balance_entry(entry_spec("same-id", &right), right.clone()).unwrap()
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        let state = journal.lock();
        prop_assert_eq!(state.intents.len(), 1);
        let memory = &state.intents["same-id"];
        let expected = if memory.row.intent_id == a.intent_id { &left } else {
            prop_assert_eq!(memory.row.intent_id, b.intent_id);
            &right
        };
        prop_assert_eq!(memory.balance.as_ref().unwrap().admission.as_deref(), Some(expected));
    }
}
