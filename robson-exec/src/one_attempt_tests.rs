//! One wire attempt per client id (ADR-0056 step 8, PR 8c-1), counted at the
//! request: a one-attempt ledger, its journal's grants, and a request future
//! that counts what would have reached the venue. Plan §7 and §11.

use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use serde_json::json;

use super::*;
use crate::venue::SubmitAttemptLedger;

const BOUND: Duration = Duration::from_millis(30);

fn flagged_on(backend: Arc<dyn JournalBackend>) -> Arc<OrderJournal> {
    let journal = OrderJournal::new(backend, &VenueId::new("test:spot"))
        .with_commit_bound(BOUND)
        .with_one_attempt_per_client_id(true);
    Arc::new(journal)
}

fn flagged() -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
    let backend = Arc::new(StubJournalBackend::default());
    (backend.clone(), flagged_on(backend))
}

fn unflagged() -> (Arc<StubJournalBackend>, Arc<OrderJournal>) {
    let backend = Arc::new(StubJournalBackend::default());
    let journal = OrderJournal::new(backend.clone(), &VenueId::new("test:venue"));
    (backend, Arc::new(journal.with_commit_bound(BOUND)))
}

fn ledger_for(journal: &Arc<OrderJournal>) -> Arc<SubmitAttemptLedger> {
    let ledger = SubmitAttemptLedger::one_attempt(64);
    ledger.attach_journal(Arc::clone(journal));
    Arc::new(ledger)
}

fn spec(id: &str, kind: IntentKind) -> IntentSpec {
    IntentSpec {
        position_id: None,
        kind,
        client_order_id: id.to_string(),
        cancels_order_id: None,
        params: json!({}),
    }
}

/// One submission through the ledger: the request counts itself when polled.
async fn send(ledger: &SubmitAttemptLedger, id: &str, sent: &AtomicUsize) -> Result<(), ExecError> {
    ledger
        .submit(id, |_: &ExecError| false, async {
            sent.fetch_add(1, Ordering::SeqCst);
            Ok::<(), ExecError>(())
        })
        .await
}

/// A submission whose request never answers, once it has been sent.
async fn send_hanging(ledger: &SubmitAttemptLedger, id: &str, sent: &AtomicUsize) {
    let _ = ledger
        .submit(id, |_: &ExecError| false, async {
            sent.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<Result<(), ExecError>>().await
        })
        .await;
}

/// An operation kept open for the whole test, shareable across tasks.
fn operation(journal: &Arc<OrderJournal>) -> Arc<Operation> {
    Arc::new(journal.begin_operation())
}

async fn open(op: &Operation, journal: &OrderJournal, id: &str) -> JournaledIntent {
    op.scope(async { journal.open_intent(spec(id, IntentKind::Exit)) }).await
}

async fn in_scope<F: Future>(op: &Operation, work: F) -> F::Output {
    op.scope(work).await
}

#[tokio::test]
async fn a_granted_send_goes_out_once_and_only_once() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        assert!(guard.is_granted());
        send(&ledger, "x", &sent).await.unwrap();
        // FILLED, and a resubmission under the same id before completion.
        assert!(send(&ledger, "x", &sent).await.is_err());
        drop(guard);
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nothing_goes_out_without_a_grant() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let direct = open(&op, &journal, "direct").await;
    let legacy = open(&op, &journal, "legacy").await;
    in_scope(&op, async {
        // A direct submission, with no authorization at all.
        assert!(send(&ledger, "direct", &sent).await.is_err());
        // The legacy authorization commits the attempt and grants nothing.
        journal.before_send(&legacy, WriteClass::Strict).await.unwrap();
        assert!(send(&ledger, "legacy", &sent).await.is_err());
        // And it cannot be followed by a granting one: one attempt.
        assert!(journal.authorize_send(&legacy, WriteClass::Strict).await.is_err());
        assert!(send(&ledger, "legacy", &sent).await.is_err());
        let _ = direct;
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_cancel_takes_no_grant() {
    let (_, journal) = flagged();
    let op = operation(&journal);
    let intent =
        in_scope(&op, async { journal.open_intent(spec("cxl-1", IntentKind::Cancel)) }).await;
    let refused = in_scope(&op, journal.authorize_send(&intent, WriteClass::Bounded)).await;
    assert!(matches!(refused, Err(SendRefusal::NotSent(_))), "{refused:?}");
}

#[tokio::test]
async fn a_failed_or_uncertain_strict_commit_grants_nothing() {
    for fault in [
        JournalFault::Known,
        JournalFault::NotLandedUncertain,
        JournalFault::AckLost,
    ] {
        let (backend, journal) = flagged();
        let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
        let op = operation(&journal);
        let intent = open(&op, &journal, "x").await;
        backend.push_fault(fault);
        in_scope(&op, async {
            assert!(journal.authorize_send(&intent, WriteClass::Strict).await.is_err());
            assert!(send(&ledger, "x", &sent).await.is_err());
        })
        .await;
        assert_eq!(sent.load(Ordering::SeqCst), 0, "{fault:?}");
    }
}

/// J lets a first Bounded attempt go out with its row unlanded; on a
/// one-attempt journal that is still the one granted send, and nothing more.
#[tokio::test]
async fn a_first_bounded_commit_that_fails_still_sends_once() {
    for fault in [JournalFault::Known, JournalFault::NotLandedUncertain] {
        let (backend, journal) = flagged();
        let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
        let op = operation(&journal);
        let intent = open(&op, &journal, "x").await;
        backend.push_fault(fault);
        in_scope(&op, async {
            let guard = journal.authorize_send(&intent, WriteClass::Bounded).await.unwrap();
            assert!(guard.is_granted());
            send(&ledger, "x", &sent).await.unwrap();
            assert!(journal.authorize_send(&intent, WriteClass::Bounded).await.is_err());
            assert!(send(&ledger, "x", &sent).await.is_err());
        })
        .await;
        assert_eq!(sent.load(Ordering::SeqCst), 1, "{fault:?}");
        assert!(journal.latch_set(), "{fault:?}: the unlanded send set the latch");
    }
}

/// An overflow seals the memory against further attempts; it does not close
/// the first send (round 3, finding 1). Nothing goes out after the outcome or
/// the operation's end.
#[tokio::test]
async fn bounded_overflow_sends_the_first_and_nothing_after() {
    let (backend, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    backend.set_fail_all(Some(JournalFault::Known));
    in_scope(&op, async {
        for index in 0..RETRY_SET_CAPACITY {
            let filler = journal.open_intent(spec(&format!("filler-{index}"), IntentKind::Exit));
            journal.before_send(&filler, WriteClass::Bounded).await.unwrap();
        }
    })
    .await;
    assert_eq!(journal.lock().retry_set.len(), RETRY_SET_CAPACITY);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let guard = journal.authorize_send(&intent, WriteClass::Bounded).await.unwrap();
        assert!(journal.lock().discarded.contains("x"), "it overflowed into the discarded set");
        send(&ledger, "x", &sent).await.unwrap();
        assert!(journal.authorize_send(&intent, WriteClass::Bounded).await.is_err());
        assert!(send(&ledger, "x", &sent).await.is_err());
        journal.finish_failed(&intent).await;
        assert!(send(&ledger, "x", &sent).await.is_err());
        drop(guard);
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);

    // An overflowed first send whose operation ends before it is sent.
    let other = operation(&journal);
    let late = open(&other, &journal, "y").await;
    let guard = in_scope(&other, journal.authorize_send(&late, WriteClass::Bounded))
        .await
        .unwrap();
    drop(other);
    let after = operation(&journal);
    assert!(in_scope(&after, send(&ledger, "y", &sent)).await.is_err());
    drop(guard);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_pending_commit_grants_nothing() {
    // A commit bound far longer than the test: the commit stays pending.
    let backend = Arc::new(StubJournalBackend::default());
    let journal = Arc::new(
        OrderJournal::new(backend.clone(), &VenueId::new("test:spot"))
            .with_commit_bound(Duration::from_secs(30))
            .with_one_attempt_per_client_id(true),
    );
    let (ledger, sent) = (ledger_for(&journal), Arc::new(AtomicUsize::new(0)));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    let hold = backend.hold_commit(0);
    let authorizing = {
        let (journal, op, intent) = (Arc::clone(&journal), Arc::clone(&op), intent.clone());
        tokio::spawn(async move {
            op.scope(async { journal.authorize_send(&intent, WriteClass::Strict).await })
                .await
        })
    };
    while backend.commits().is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(in_scope(&op, send(&ledger, "x", &sent)).await.is_err(), "pending: no grant");
    hold.notify_one();
    let guard = authorizing.await.unwrap().unwrap();
    in_scope(&op, send(&ledger, "x", &sent)).await.unwrap();
    drop(guard);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_send_outside_the_intents_operation_is_refused() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let (op, other) = (operation(&journal), operation(&journal));
    let intent = open(&op, &journal, "x").await;
    let guard = in_scope(&op, journal.authorize_send(&intent, WriteClass::Strict))
        .await
        .unwrap();
    // No operation at all.
    assert!(send(&ledger, "x", &sent).await.is_err());
    // Another operation of the same journal.
    assert!(in_scope(&other, send(&ledger, "x", &sent)).await.is_err());
    // Another journal's operation.
    let (_, stranger) = flagged();
    let foreign = operation(&stranger);
    assert!(in_scope(&foreign, send(&ledger, "x", &sent)).await.is_err());
    // A refusal before the grant check leaves the grant for its owner.
    in_scope(&op, send(&ledger, "x", &sent)).await.unwrap();
    drop(guard);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_wrong_journal_grants_nothing() {
    let (_, journal) = flagged();
    let (_, other) = flagged();
    let (ledger, sent) = (ledger_for(&other), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_ledger_needs_a_journal_with_the_capability() {
    let sent = AtomicUsize::new(0);
    let detached = SubmitAttemptLedger::one_attempt(8);
    assert!(send(&detached, "x", &sent).await.is_err());

    let (_, unflagged) = unflagged();
    let ledger = SubmitAttemptLedger::one_attempt(8);
    ledger.attach_journal(Arc::clone(&unflagged));
    let op = operation(&unflagged);
    let intent = open(&op, &unflagged, "x").await;
    in_scope(&op, async {
        let guard = unflagged.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        assert!(!guard.is_granted(), "an unflagged journal grants nothing");
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_completed_intent_sends_nothing() {
    for success in [true, false] {
        let (_, journal) = flagged();
        let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
        let op = operation(&journal);
        let intent = open(&op, &journal, "x").await;
        in_scope(&op, async {
            let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            if success {
                journal.finish_success(&intent, None).await;
            } else {
                journal.finish_failed(&intent).await;
            }
            assert!(send(&ledger, "x", &sent).await.is_err());
        })
        .await;
        assert_eq!(sent.load(Ordering::SeqCst), 0, "success {success}");
    }
}

#[tokio::test]
async fn a_restarted_journal_cannot_send_a_landed_id() {
    let (backend, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        send(&ledger, "x", &sent).await.unwrap();
    })
    .await;
    let restarted = flagged_on(backend);
    let ledger = ledger_for(&restarted);
    let op = operation(&restarted);
    let again = open(&op, &restarted, "x").await;
    in_scope(&op, async {
        assert!(restarted.authorize_send(&again, WriteClass::Strict).await.is_err(), "id spent");
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

/// Round 2's sequence: the child that authorized is cancelled while the outer
/// operation lives on (`within_operation` reuses it), then a direct
/// submission under that operation.
#[tokio::test]
async fn a_cancelled_child_leaves_no_grant_to_its_outer_operation() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let child = journal.within_operation(async {
            let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
            send(&ledger, "x", &sent).await
        });
        assert!(tokio::time::timeout(Duration::from_millis(20), child).await.is_err());
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_submission_cancelled_during_its_commit_sends_nothing() {
    let (backend, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    let _hold = backend.hold_commit(0);
    in_scope(&op, async {
        let child = async {
            let _guard = journal.authorize_send(&intent, WriteClass::Strict).await;
            send(&ledger, "x", &sent).await
        };
        assert!(tokio::time::timeout(Duration::from_millis(5), child).await.is_err());
        // The appended attempt still blocks a second one.
        assert!(journal.authorize_send(&intent, WriteClass::Strict).await.is_err());
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_submission_cancelled_after_its_send_sends_once() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let child = async {
            let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            send_hanging(&ledger, "x", &sent).await;
        };
        assert!(tokio::time::timeout(Duration::from_millis(20), child).await.is_err());
        assert_eq!(sent.load(Ordering::SeqCst), 1, "it was sent before the cancel");
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_guard_dropped_before_its_request_sends_nothing() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        drop(guard);
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

/// A failed outcome write, its retry and its late landing never give a taken
/// grant back.
#[tokio::test]
async fn a_late_outcome_failure_restores_no_grant() {
    let (backend, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        send(&ledger, "x", &sent).await.unwrap();
        backend.set_fail_all(Some(JournalFault::Known));
        journal.finish_success(&intent, None).await;
        assert!(send(&ledger, "x", &sent).await.is_err());
        backend.set_fail_all(None);
        assert!(journal.flush_pending().await > 0);
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_panic_while_holding_the_guard_revokes_it() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    let panicked = {
        let (journal, op, intent) = (Arc::clone(&journal), Arc::clone(&op), intent.clone());
        tokio::spawn(async move {
            op.scope(async {
                let _guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
                panic!("unwinding with the guard held");
            })
            .await
        })
        .await
    };
    assert!(panicked.unwrap_err().is_panic());
    assert!(in_scope(&op, send(&ledger, "x", &sent)).await.is_err());
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_second_authorization_leaves_the_first_grant_alone() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let first = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
        let second = journal.authorize_send(&intent, WriteClass::Strict).await;
        assert!(matches!(second, Err(SendRefusal::NotSent(_))), "{second:?}");
        send(&ledger, "x", &sent).await.unwrap();
        drop(first);
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_guard_moved_to_another_task_sends_only_inside_its_operation() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), Arc::new(AtomicUsize::new(0)));
    let op = operation(&journal);

    // Moved to a task that runs inside the same live operation: one send.
    let intent = open(&op, &journal, "inside").await;
    let guard = in_scope(&op, journal.authorize_send(&intent, WriteClass::Strict))
        .await
        .unwrap();
    let inside = {
        let (op, ledger, sent) = (Arc::clone(&op), Arc::clone(&ledger), Arc::clone(&sent));
        tokio::spawn(async move {
            let result = op.scope(send(&ledger, "inside", &sent)).await;
            drop(guard);
            result
        })
    };
    inside.await.unwrap().unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), 1);

    // Moved to a task outside it: refused, and its drop revokes the grant.
    let intent = open(&op, &journal, "outside").await;
    let guard = in_scope(&op, journal.authorize_send(&intent, WriteClass::Strict))
        .await
        .unwrap();
    let outside = {
        let (ledger, sent) = (Arc::clone(&ledger), Arc::clone(&sent));
        tokio::spawn(async move {
            let result = send(&ledger, "outside", &sent).await;
            drop(guard);
            result
        })
    };
    assert!(outside.await.unwrap().is_err());
    assert!(in_scope(&op, send(&ledger, "outside", &sent)).await.is_err());
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_operation_ended_before_its_guard_sends_nothing() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = journal.begin_operation();
    let intent = op.scope(async { journal.open_intent(spec("x", IntentKind::Exit)) }).await;
    let guard = op.scope(journal.authorize_send(&intent, WriteClass::Strict)).await.unwrap();
    drop(op);
    let after = operation(&journal);
    assert!(in_scope(&after, send(&ledger, "x", &sent)).await.is_err());
    drop(guard);
    assert!(in_scope(&after, send(&ledger, "x", &sent)).await.is_err());
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

/// An earlier guard's drop never revokes a later grant under the same id
/// (possible only where no durable row stops the id's reuse).
#[tokio::test]
async fn a_stale_revocation_leaves_a_later_grant_alone() {
    let journal = flagged_on(Arc::new(UnpersistedBackend));
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    let op = operation(&journal);
    let first = open(&op, &journal, "x").await;
    let old = in_scope(&op, async {
        let guard = journal.authorize_send(&first, WriteClass::Strict).await.unwrap();
        send(&ledger, "x", &sent).await.unwrap();
        journal.finish_success(&first, None).await;
        guard
    })
    .await;
    assert!(journal.lock().intents.is_empty(), "the first intent's memory went");
    let second = open(&op, &journal, "x").await;
    in_scope(&op, async {
        let new = journal.authorize_send(&second, WriteClass::Strict).await.unwrap();
        drop(old);
        send(&ledger, "x", &sent).await.unwrap();
        drop(new);
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_exhausted_grant_counter_grants_nothing() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), AtomicUsize::new(0));
    journal.exhaust_grants();
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        assert!(journal.authorize_send(&intent, WriteClass::Strict).await.is_err());
        assert!(send(&ledger, "x", &sent).await.is_err());
    })
    .await;
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn grant_ids_are_never_reused() {
    let (_, journal) = flagged();
    let op = operation(&journal);
    let mut seen = HashSet::new();
    for index in 0..32 {
        let intent = open(&op, &journal, &format!("x{index}")).await;
        let guard = in_scope(&op, journal.authorize_send(&intent, WriteClass::Strict))
            .await
            .unwrap();
        assert!(seen.insert(guard.grant_id.unwrap()));
    }
}

#[tokio::test]
async fn concurrent_submissions_under_one_grant_send_once() {
    let (_, journal) = flagged();
    let (ledger, sent) = (ledger_for(&journal), Arc::new(AtomicUsize::new(0)));
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    let guard = in_scope(&op, journal.authorize_send(&intent, WriteClass::Strict))
        .await
        .unwrap();
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let (op, ledger, sent) = (Arc::clone(&op), Arc::clone(&ledger), Arc::clone(&sent));
            tokio::spawn(async move { op.scope(send(&ledger, "x", &sent)).await })
        })
        .collect();
    let mut ok = 0;
    for task in tasks {
        ok += usize::from(task.await.unwrap().is_ok());
    }
    drop(guard);
    assert_eq!((ok, sent.load(Ordering::SeqCst)), (1, 1));
}

/// Flag off, nothing changes: the same id is authorized and sent again, and a
/// ledger built with `new` sends with no grant (USD-M, the stub, funding).
#[tokio::test]
async fn without_the_capability_the_same_id_retry_is_still_authorized() {
    let (backend, journal) = unflagged();
    let ledger = SubmitAttemptLedger::new(8);
    ledger.attach_journal(Arc::clone(&journal));
    let sent = AtomicUsize::new(0);
    let op = operation(&journal);
    let intent = open(&op, &journal, "x").await;
    in_scope(&op, async {
        journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
        send(&ledger, "x", &sent).await.unwrap();
        journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
        send(&ledger, "x", &sent).await.unwrap();
    })
    .await;
    // A send with no intent at all (a funding order): counted, no ticket.
    in_scope(&op, send(&ledger, "funding-1", &sent)).await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), 3);
    assert_eq!(backend.row("x").unwrap().attempts.len(), 2);
    assert!(journal.lock().intents.values().all(|memory| memory.grant.is_none()));
}

/// Seeded random interleavings (no new dependency): tasks that authorize and
/// send, send directly, are cancelled at a random point, or drop their guard
/// first, over random commit delays and faults, Strict and Bounded. At most
/// one request per id; none when no authorization succeeded; none after every
/// guard is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_interleavings_send_at_most_once() {
    let mut seed: u64 = 0x8c1_5eed;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for case in 0..200 {
        let (backend, journal) = flagged();
        let ledger = ledger_for(&journal);
        let sent = Arc::new(AtomicUsize::new(0));
        let authorized = Arc::new(AtomicUsize::new(0));
        let op = operation(&journal);
        let class = if next() % 2 == 0 {
            WriteClass::Strict
        } else {
            WriteClass::Bounded
        };
        match next() % 4 {
            0 => backend.push_fault(JournalFault::Known),
            1 => backend.push_fault(JournalFault::NotLandedUncertain),
            _ => {},
        }
        backend.set_commit_delay(Duration::from_micros(next() % 1500));
        let intent = open(&op, &journal, "x").await;
        let tasks: Vec<_> = (0..2 + next() % 4)
            .map(|_| {
                let strategy = next() % 4;
                let cancel_after = Duration::from_micros(next() % 3000);
                let (journal, ledger, op, intent) =
                    (Arc::clone(&journal), Arc::clone(&ledger), Arc::clone(&op), intent.clone());
                let (sent, authorized) = (Arc::clone(&sent), Arc::clone(&authorized));
                tokio::spawn(async move {
                    op.scope(async move {
                        let submission = async {
                            if strategy == 1 {
                                let _ = send(&ledger, "x", &sent).await;
                                return;
                            }
                            let Ok(guard) = journal.authorize_send(&intent, class).await else {
                                return;
                            };
                            authorized.fetch_add(1, Ordering::SeqCst);
                            if strategy == 3 {
                                drop(guard);
                                let _ = send(&ledger, "x", &sent).await;
                                return;
                            }
                            tokio::task::yield_now().await;
                            let _ = send(&ledger, "x", &sent).await;
                            drop(guard);
                        };
                        if strategy == 2 {
                            let _ = tokio::time::timeout(cancel_after, submission).await;
                        } else {
                            submission.await;
                        }
                    })
                    .await
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let (sent_now, authorized) =
            (sent.load(Ordering::SeqCst), authorized.load(Ordering::SeqCst));
        assert!(sent_now <= 1, "case {case}: {sent_now} requests");
        assert!(authorized <= 1, "case {case}: {authorized} authorizations");
        if authorized == 0 {
            assert_eq!(sent_now, 0, "case {case}: sent with no authorization");
        }
        assert!(in_scope(&op, send(&ledger, "x", &sent)).await.is_err(), "case {case}");
        assert!(journal.lock().intents.values().all(|memory| memory.grant.is_none()));
    }
}
