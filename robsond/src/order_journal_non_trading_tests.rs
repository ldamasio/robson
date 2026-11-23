//! PS-1: a non-trading Spot process's journal refuses every batch before any
//! SQL. Its pool points at a closed loopback port: a batch that reached SQL
//! would fail there with another error (or hang to its bound), so the exact
//! refusal is the proof that no statement was attempted.

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use robson_exec::{
    order_journal::{
        AdmissionRow, AttemptRow, BalanceBatch, IntentKind, IntentRow, JournalBackend,
        JournalBatch, OutcomeRow, DEFAULT_ACCOUNT_SCOPE,
    },
    venue::VenueId,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use super::{CommitFailure, PgJournalBackend};

fn closed_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(300))
        .connect_lazy("postgres://robson@127.0.0.1:9/no_database")
        .unwrap()
}

fn batch(kind: IntentKind) -> JournalBatch {
    let intent_id = Uuid::now_v7();
    JournalBatch {
        intent: IntentRow {
            intent_id,
            venue_id: VenueId::BINANCE_SPOT.to_owned(),
            account_scope: DEFAULT_ACCOUNT_SCOPE.to_owned(),
            client_order_id: format!("ps1-{}", intent_id.simple()),
            position_id: Some(Uuid::now_v7()),
            kind,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at: Utc::now(),
        },
        attempts: Vec::new(),
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(300),
        }),
    }
}

fn admission(position_id: Uuid) -> AdmissionRow {
    let one = Decimal::ONE;
    AdmissionRow {
        position_id,
        symbol: "BTCUSDT".into(),
        decided_month: chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(),
        taker_bps: one,
        gap_bps: one,
        notional_margin_bps: one,
        min_notional: one,
        lot_step: one,
        fee_asset_policy: "base".into(),
        envelope_source: "ps1".into(),
        envelope_as_of: Utc::now(),
        envelope_hash: "ps1".into(),
        planned_qty: one,
        requested_qty: one,
        reference_price: one,
        remainder_reserve: one,
        admission_charge: one,
        initial_trigger: one,
    }
}

/// A strict attempt (the intent with its attempt, the commit a send's
/// authorization makes), an outcome, a disposition replay (an outcome with no
/// attempt) and an admission each come back `Known` with the refusal, so a
/// strict attempt ends `NotSent` and nothing reaches the database.
#[tokio::test]
async fn a_non_trading_commit_sends_no_statement() {
    let backend = PgJournalBackend::new(&closed_pool(), Uuid::now_v7())
        .with_balance_account(true)
        .non_trading();
    let mut attempt = batch(IntentKind::Entry);
    attempt.attempts.push(AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: Utc::now(),
        rejected_at: None,
    });
    let mut outcome = attempt.clone();
    outcome.outcome = Some(OutcomeRow {
        venue_outcome: None,
        cancel_class: None,
        not_sent: true,
        venue_evidence: None,
        uncertain_at: None,
        uncertain_generation: None,
    });
    let mut replay = batch(IntentKind::Exit);
    replay.outcome = outcome.outcome.clone();
    let mut admitted = batch(IntentKind::Entry);
    let position = admitted.intent.position_id.unwrap();
    admitted.balance.as_mut().unwrap().admission = Some(Arc::new(admission(position)));
    for (name, batch) in [
        ("strict attempt", attempt),
        ("outcome", outcome),
        ("disposition replay", replay),
        ("admission", admitted),
    ] {
        match backend.commit(&batch).await {
            Err(CommitFailure::Known(detail)) => {
                assert_eq!(detail, "non-trading Spot process: journal mutations refused", "{name}")
            },
            other => panic!("{name}: {other:?}"),
        }
    }
}

/// Through the journal, as the daemon wires it (one attempt, balance, its
/// gate refused before it is shared): neither a strict nor a bounded send is
/// authorized (a bounded first attempt would otherwise go out without its
/// row), nothing is written, and no incident opens or latch is set.
#[tokio::test]
async fn a_non_trading_journal_authorizes_no_send() {
    use robson_exec::{
        gate::ReadOnlyReason,
        order_journal::{IntentSpec, OrderJournal, SendRefusal, WriteClass},
    };
    let backend = PgJournalBackend::new(&closed_pool(), Uuid::now_v7())
        .with_balance_account(true)
        .non_trading();
    let journal = Arc::new(
        OrderJournal::new(Arc::new(backend), &VenueId::new(VenueId::BINANCE_SPOT))
            .with_one_attempt_per_client_id(true)
            .with_balance_account(true),
    );
    journal.gate().refuse_mutations(ReadOnlyReason::NonTrading).unwrap();
    for (kind, class) in [
        (IntentKind::Entry, WriteClass::Strict),
        (IntentKind::Protection, WriteClass::Bounded),
        (IntentKind::Exit, WriteClass::Bounded),
    ] {
        let operation = journal.begin_operation();
        let authorized = operation
            .scope(async {
                let intent = journal.open_intent(IntentSpec {
                    position_id: Some(Uuid::now_v7()),
                    kind,
                    client_order_id: format!("ps1-{}", Uuid::now_v7().simple()),
                    cancels_order_id: None,
                    params: serde_json::json!({}),
                });
                journal.authorize_send(&intent, class).await.map(drop)
            })
            .await;
        assert_eq!(
            authorized,
            Err(SendRefusal::NotSent(
                "venue mutation refused: read-only process (non_trading)".into()
            )),
            "{kind:?} {class:?}"
        );
    }
    let gate = journal.gate().snapshot();
    assert_eq!((gate.open_incidents, gate.fenced, gate.unpermitted), (0, false, 0));
    assert!(!journal.latch_set());
    assert_eq!(journal.statements_total(), 0, "nothing written");
}
