//! The order journal against real PostgreSQL (ADR-0056 step 7, PR J).
//!
//! The database comes from the environment, as for every `DATABASE_URL`-gated
//! test: `sqlx::test` creates one per test. Run with
//! `DATABASE_URL=… bash scripts/test-pg.sh --test order_journal_pg`.

#![cfg(feature = "postgres")]

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use robson_exec::{
    order_journal::{
        AttemptRow, CommitFailure, IntentKind, IntentRow, IntentSpec, JournalBackend, JournalBatch,
        JournaledIntent, Operation, OrderJournal, OutcomeRow, SendRefusal, VenueOutcome,
        WriteClass, DEFAULT_ACCOUNT_SCOPE,
    },
    venue::{AttemptEvidence, OrderLookup, SubmitAttemptLedger, VenueFinalityRule, VenueId},
    ExecError, StopCancelOutcome,
};
use robson_testkit::{
    daemon_process::database_url_through,
    db_proxy::{DbProxy, ProxyMode},
};
use robsond::order_journal_store::PgJournalBackend;
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, Connection, PgPool};
use uuid::Uuid;

const VENUE: &str = "test:venue";

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn real_journal(pool: &PgPool) -> Arc<OrderJournal> {
    let backend = Arc::new(PgJournalBackend::new(pool, Uuid::now_v7()));
    Arc::new(OrderJournal::new(backend, &venue()))
}

fn spec(id: &str, kind: IntentKind) -> IntentSpec {
    IntentSpec {
        position_id: None,
        kind,
        client_order_id: id.to_string(),
        cancels_order_id: None,
        params: json!({ "symbol": "BTCUSDT" }),
    }
}

async fn opened(
    journal: &Arc<OrderJournal>,
    id: &str,
    kind: IntentKind,
) -> (Operation, JournaledIntent) {
    let operation = journal.begin_operation();
    let intent = operation.scope(async { journal.open_intent(spec(id, kind)) }).await;
    (operation, intent)
}

fn intent_row(id: &str) -> IntentRow {
    IntentRow {
        intent_id: Uuid::now_v7(),
        venue_id: VENUE.to_string(),
        account_scope: DEFAULT_ACCOUNT_SCOPE.to_string(),
        client_order_id: id.to_string(),
        position_id: None,
        kind: IntentKind::Exit,
        cancels_order_id: None,
        params: json!({}),
        created_at: Utc::now(),
    }
}

fn attempt_row(rejected: bool) -> AttemptRow {
    AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: Utc::now(),
        rejected_at: rejected.then(Utc::now),
    }
}

async fn attempt_states_of(pool: &PgPool, id: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT state FROM submit_attempts WHERE client_order_id = $1 ORDER BY attempt_id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn blockers(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT client_order_id FROM order_intents_blocking_rollback ORDER BY client_order_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A pool through a proxy in front of this test's database.
async fn through_proxy(pool: &PgPool) -> (DbProxy, PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = DbProxy::start(upstream).await.unwrap();
    let database = options.get_database().expect("sqlx::test names its database");
    let url = database_url_through(&std::env::var("DATABASE_URL").unwrap(), proxy.addr(), database)
        .unwrap();
    let proxied = PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    (proxy, proxied)
}

// Rows and rules, on real SQL
// ---------------------------------------------------

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_intent_and_its_attempt_land_in_one_transaction_and_replays_converge(pool: PgPool) {
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    let batch = JournalBatch {
        intent: intent_row("a"),
        attempts: vec![attempt_row(false)],
        outcome: None,
    };
    backend.commit(&batch).await.unwrap();
    backend.commit(&batch).await.unwrap();
    assert_eq!(attempt_states_of(&pool, "a").await, vec!["started"]);

    // The rejection lands, then the stale `started` arrives again.
    let mut rejected = batch.clone();
    rejected.attempts[0].rejected_at = Some(Utc::now());
    backend.commit(&rejected).await.unwrap();
    backend.commit(&batch).await.unwrap();
    assert_eq!(attempt_states_of(&pool, "a").await, vec!["rejected"]);
}

/// The whole batch or nothing: a batch that dies on its attempt leaves no
/// intent behind.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failing_attempt_rolls_its_intent_back(pool: PgPool) {
    sqlx::query(
        "CREATE FUNCTION refuse_attempts() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_attempts BEFORE INSERT ON submit_attempts
         FOR EACH ROW EXECUTE FUNCTION refuse_attempts()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    let batch = JournalBatch {
        intent: intent_row("a"),
        attempts: vec![attempt_row(false)],
        outcome: None,
    };

    let failure = backend.commit(&batch).await.unwrap_err();

    assert!(matches!(failure, CommitFailure::Known(_)), "{failure:?}");
    assert_eq!(blockers(&pool).await, Vec::<String>::new());
}

/// Rule 3 across processes: a later process cannot take a landed id.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_landed_client_id_is_spent_for_every_later_process(pool: PgPool) {
    let first = real_journal(&pool);
    let (operation, intent) = opened(&first, "a", IntentKind::Entry).await;
    operation.scope(first.before_send(&intent, WriteClass::Strict)).await.unwrap();

    let later = real_journal(&pool);
    for class in [WriteClass::Strict, WriteClass::Bounded] {
        let (operation, intent) = opened(&later, "a", IntentKind::Exit).await;
        let refusal = operation.scope(later.before_send(&intent, class)).await.unwrap_err();
        assert!(matches!(refusal, SendRefusal::NotSent(_)), "{class:?}: {refusal:?}");
        operation.scope(later.finish_failed(&intent)).await;
    }
    assert_eq!(attempt_states_of(&pool, "a").await, vec!["started"], "the row is untouched");
}

/// Rule 4 across processes: the evidence is the rows.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn absent_needs_every_durable_attempt_rejected_across_a_restart(pool: PgPool) {
    let rule = VenueFinalityRule::EveryAttemptRejected;
    let first = real_journal(&pool);
    let ledger = SubmitAttemptLedger::new(8);
    ledger.attach_journal(first.clone());

    // No row: unknown for ever.
    assert!(matches!(
        rule.classify_not_found(ledger.final_evidence("a").await),
        OrderLookup::Unknown
    ));

    let (operation, intent) = opened(&first, "a", IntentKind::Exit).await;
    operation.scope(first.before_send(&intent, WriteClass::Bounded)).await.unwrap();
    operation
        .scope(ledger.submit("a", |_: &()| true, async { Err::<(), ()>(()) }))
        .await
        .unwrap_err();
    // In memory, so unresolved, although it is rejected.
    assert!(matches!(
        rule.classify_not_found(ledger.final_evidence("a").await),
        OrderLookup::Unknown
    ));
    operation.scope(first.finish_failed(&intent)).await;
    assert!(matches!(
        rule.classify_not_found(ledger.final_evidence("a").await),
        OrderLookup::Absent { .. }
    ));

    // A fresh process finds the same, and only the rejected id is absent.
    let (operation, intent) = opened(&first, "b", IntentKind::Exit).await;
    operation.scope(first.before_send(&intent, WriteClass::Bounded)).await.unwrap();
    operation.scope(first.finish_failed(&intent)).await;

    let later = real_journal(&pool);
    let restarted = SubmitAttemptLedger::new(8);
    restarted.attach_journal(later.clone());
    assert!(matches!(
        rule.classify_not_found(restarted.final_evidence("a").await),
        OrderLookup::Absent { .. }
    ));
    assert!(matches!(
        restarted.final_evidence("b").await,
        AttemptEvidence::Unresolved { .. }
    ));
}

// How writes really reach PostgreSQL
// ---------------------------------------------

/// Every journal write is an explicit transaction ending in a simple-query
/// `COMMIT`, so the proxy counts each one and `DropCommitAck` can reach it.
/// Nothing commits implicitly or through the extended protocol.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_journal_write_is_a_simple_query_commit(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);
    journal.check_ready().await.expect("the pool is up");
    let commits_before = proxy.stats().commits;
    let statements_before = proxy.stats().statements;

    let (operation, intent) = opened(&journal, "a", IntentKind::Entry).await;
    operation.scope(journal.before_send(&intent, WriteClass::Strict)).await.unwrap();
    assert_eq!(proxy.stats().commits, commits_before + 1, "the intent and its attempt");
    operation.scope(journal.finish_failed(&intent)).await;
    assert_eq!(proxy.stats().commits, commits_before + 2, "the result");
    assert_eq!(
        proxy.stats().statements,
        statements_before + journal.statements_total(),
        "the journal counts what the database sees"
    );
}

/// The acknowledgement of a real strict commit is lost: the row is there, the
/// client cannot know, and no request may go out.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_commit_ack_is_an_uncertain_commit_and_the_row_landed(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);

    proxy.set_mode(ProxyMode::DropCommitAckOf("order_intents"));
    let (operation, intent) = opened(&journal, "a", IntentKind::Entry).await;
    let refusal = operation
        .scope(journal.before_send(&intent, WriteClass::Strict))
        .await
        .unwrap_err();

    assert!(matches!(refusal, SendRefusal::UncertainCommit(_)), "{refusal:?}");
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
    assert_eq!(attempt_states_of(&pool, "a").await, vec!["started"], "the commit landed");
}

/// A targeted fault leaves other transactions on the connection alone.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_targeted_fault_ignores_other_work(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    sqlx::query("CREATE TABLE harness_probe (id INT)").execute(&pool).await.unwrap();

    proxy.set_mode(ProxyMode::DropCommitAckOf("order_intents"));
    let mut tx = proxied.begin().await.unwrap();
    sqlx::query("INSERT INTO harness_probe VALUES (1)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.expect("not the targeted transaction");
    assert_eq!(proxy.mode(), ProxyMode::DropCommitAckOf("order_intents"), "still armed");
}

/// With every connection refused, the commit fails before any statement went
/// out: a proven no-send. The acquire bound keeps it inside the commit bound.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_database_that_is_gone_is_a_known_failure_for_a_strict_commit(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);
    proxy.set_mode(ProxyMode::Sever);

    let (operation, intent) = opened(&journal, "a", IntentKind::Entry).await;
    let started = std::time::Instant::now();
    let refusal = operation
        .scope(journal.before_send(&intent, WriteClass::Strict))
        .await
        .unwrap_err();

    assert!(matches!(refusal, SendRefusal::NotSent(_)), "{refusal:?}");
    assert!(started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
}

/// A statement that does not answer: a timeout is an uncertain commit, inside
/// the bound, and the bounded class sends anyway with the latch set.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stalled_journal_commit_times_out_at_its_bound(pool: PgPool) {
    sqlx::query(
        "CREATE FUNCTION stall_intents() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(3); RETURN NEW; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER stall_intents BEFORE INSERT ON order_intents
         FOR EACH ROW EXECUTE FUNCTION stall_intents()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let journal = real_journal(&pool);

    let (operation, entry) = opened(&journal, "e", IntentKind::Entry).await;
    let started = std::time::Instant::now();
    let refusal = operation
        .scope(journal.before_send(&entry, WriteClass::Strict))
        .await
        .unwrap_err();
    assert!(matches!(refusal, SendRefusal::UncertainCommit(_)), "{refusal:?}");
    assert!(started.elapsed() < Duration::from_millis(900), "{:?}", started.elapsed());
    assert!(!journal.latch_set());

    let (operation, exit) = opened(&journal, "x", IntentKind::Exit).await;
    operation.scope(journal.before_send(&exit, WriteClass::Bounded)).await.unwrap();
    assert!(journal.latch_set());
    assert_eq!(journal.retry_set_depth(), 1);
}

// Boundary B

/// Every outstanding intent blocks until it carries a disposition; a definite
/// outcome or a proven no-send never blocks. Read from the view itself.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn boundary_b_lists_exactly_the_outstanding_intents_with_no_disposition(pool: PgPool) {
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    let write = |id: &'static str, outcome: Option<OutcomeRow>| {
        let batch = JournalBatch {
            intent: intent_row(id),
            attempts: vec![attempt_row(false)],
            outcome,
        };
        let backend = &backend;
        async move { backend.commit(&batch).await.unwrap() }
    };
    let definite = |outcome| OutcomeRow {
        venue_outcome: Some(outcome),
        ..OutcomeRow::default()
    };
    write("success", Some(definite(VenueOutcome::Success))).await;
    write("rejected", Some(definite(VenueOutcome::Rejected))).await;
    write("not-sent", Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() })).await;
    write(
        "uncertain",
        Some(OutcomeRow {
            uncertain_at: Some(Utc::now()),
            ..OutcomeRow::default()
        }),
    )
    .await;
    // No stamp and no outcome: what a dead process leaves behind.
    write("unstamped", None).await;
    assert_eq!(blockers(&pool).await, vec!["uncertain", "unstamped"]);

    sqlx::query(
        "UPDATE order_intents SET disposition_kind = 'accepted_risk',
            disposition_treatment = 'unbooked_accepted', risk_accepted_at = NOW(),
            risk_accepted_by = 'x', risk_accepted_auth = 'verified'
         WHERE client_order_id = 'uncertain'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(blockers(&pool).await, vec!["unstamped"]);
}

/// A cancel's class on real SQL (ADR-0056 step 7, PR C): `gone_terminal` passes
/// the column's check, the fills land as evidence, and boundary B lists exactly
/// the uncertain classes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_cancel_lands_its_class_and_evidence_on_real_sql(pool: PgPool) {
    let journal = real_journal(&pool);
    let gone = |final_state| Ok(StopCancelOutcome::AlreadyGone { final_state });
    let cases: Vec<(&str, Result<StopCancelOutcome, ExecError>)> = vec![
        (
            "cancelled",
            Ok(StopCancelOutcome::Cancelled {
                filled_before_cancel: Decimal::ZERO,
                fills: None,
            }),
        ),
        (
            "gone_terminal",
            gone(OrderLookup::Terminal {
                filled_qty: Decimal::new(25, 2),
                cancelled_qty: Decimal::ZERO,
                fills: None,
            }),
        ),
        ("gone_unresolved", gone(OrderLookup::Unknown)),
        ("indeterminate", Ok(StopCancelOutcome::Indeterminate)),
        ("error", Err(ExecError::Timeout("no answer".to_string()))),
    ];
    for (order, result) in cases {
        let operation = journal.begin_operation();
        let class = operation.scope(journal.journaled_cancel(None, order, async { result })).await;
        assert_eq!(class.label(), order);
    }

    let row = |order: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Option<String>, Option<String>, Option<serde_json::Value>, bool)>(
                "SELECT venue_outcome, cancel_class, venue_evidence, uncertain_at IS NOT NULL
                 FROM order_intents WHERE cancels_order_id = $1 AND kind = 'cancel'",
            )
            .bind(order)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let (outcome, class, evidence, uncertain) = row("cancelled").await;
    assert_eq!(outcome.as_deref(), Some("success"));
    assert_eq!(class.as_deref(), Some("cancelled"));
    assert_eq!(evidence.unwrap()["fills"], serde_json::Value::Null);
    assert!(!uncertain);

    let (outcome, class, evidence, uncertain) = row("gone_terminal").await;
    assert_eq!(outcome.as_deref(), Some("gone_terminal"));
    assert_eq!(class.as_deref(), Some("gone_terminal"));
    assert_eq!(evidence.unwrap()["filled_qty"], json!(Decimal::new(25, 2)));
    assert!(!uncertain);

    for order in ["gone_unresolved", "indeterminate", "error"] {
        let (outcome, class, evidence, uncertain) = row(order).await;
        assert_eq!(outcome, None, "{order}");
        assert_eq!(class.as_deref(), Some(order));
        assert_eq!(evidence, None, "{order}");
        assert!(uncertain, "{order}");
    }

    let mut blocking: Vec<String> = sqlx::query_scalar(
        "SELECT cancels_order_id FROM order_intents
         WHERE intent_id IN (SELECT intent_id FROM order_intents_blocking_rollback)",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    blocking.sort();
    assert_eq!(blocking, vec!["error", "gone_unresolved", "indeterminate"]);
}

/// The constraints hold the predicate even for a write that skips the
/// endpoint: an empty-account note, a bare assertion, or no treatment cannot be
/// a disposition.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_schema_refuses_an_empty_account_observation_as_terminal_evidence(pool: PgPool) {
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    backend
        .commit(&JournalBatch {
            intent: intent_row("a"),
            attempts: vec![attempt_row(false)],
            outcome: None,
        })
        .await
        .unwrap();

    for assignment in [
        r#"disposition_kind = 'terminal_evidence', disposition_treatment = 'unbooked_accepted',
           disposition_evidence = '{"positions": []}'"#,
        r#"disposition_kind = 'terminal_evidence', disposition_treatment = 'unbooked_accepted'"#,
        r#"disposition_kind = 'terminal_evidence', disposition_treatment = 'unbooked_accepted',
           disposition_evidence = '{"order_ref": "a", "terminal_status": "filled"}'"#,
        r#"disposition_kind = 'terminal_evidence', disposition_treatment = 'unbooked_accepted',
           disposition_evidence = '{"order_ref": "a", "terminal_status": "filled",
                                     "venue_record": {}}'"#,
        r#"disposition_kind = 'accepted_risk'"#,
        r#"disposition_kind = 'accepted_risk', disposition_treatment = 'unbooked_accepted'"#,
    ] {
        let result = sqlx::query(&format!("UPDATE order_intents SET {assignment}"))
            .execute(&pool)
            .await;
        assert!(result.is_err(), "{assignment}");
    }
    assert_eq!(blockers(&pool).await, vec!["a"]);
}

// Readiness and connection recovery

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_readiness_check_refuses_a_schema_without_the_journal(pool: PgPool) {
    let journal = real_journal(&pool);
    journal.check_ready().await.expect("the schema is there");

    sqlx::query("DROP VIEW order_intents_blocking_rollback")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TABLE submit_attempts").execute(&pool).await.unwrap();
    let started = std::time::Instant::now();
    let refused = real_journal(&pool).check_ready().await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a missing table is not worth waiting on"
    );
}

/// Commit one intent, retrying until the pool has a connection for it.
async fn lands_within(
    journal: &Arc<OrderJournal>,
    id: &str,
    limit: Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + limit;
    let (operation, intent) = opened(journal, id, IntentKind::Exit).await;
    let result = operation.scope(journal.before_send(&intent, WriteClass::Bounded)).await;
    assert_eq!(result, Ok(()));
    drop(operation);
    while journal.retry_set_depth() > 0 || journal.is_live(id) {
        if std::time::Instant::now() >= deadline {
            return Err(format!("still {} pending", journal.retry_set_depth()));
        }
        journal.flush_pending().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// Both connections are cancelled mid-transaction and the replies they were
/// waiting for are lost. Returned to the pool they would wait for those replies
/// for ever, holding both slots; retired, the pool works as soon as the network
/// does.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn two_cancelled_writes_do_not_exhaust_the_journal_pool(pool: PgPool) {
    sqlx::query(
        "CREATE FUNCTION stall_intents() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(1); RETURN NEW; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER stall_intents BEFORE INSERT ON order_intents
         FOR EACH ROW EXECUTE FUNCTION stall_intents()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);
    journal.check_ready().await.expect("ready");

    let strict = |id: &'static str| {
        let journal = Arc::clone(&journal);
        async move {
            let (operation, intent) = opened(&journal, id, IntentKind::Entry).await;
            operation.scope(journal.before_send(&intent, WriteClass::Strict)).await
        }
    };
    // Both writes are inside their stalled statement when the replies stop.
    let cut_off = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        proxy.set_mode(ProxyMode::Blackhole);
    };
    let (first, second, ()) = tokio::join!(strict("a"), strict("b"), cut_off);
    assert!(matches!(first, Err(SendRefusal::UncertainCommit(_))), "{first:?}");
    assert!(matches!(second, Err(SendRefusal::UncertainCommit(_))), "{second:?}");

    // Long enough for the statements' replies to have been swallowed.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // A transaction left open would hold its lock on the table for ever.
    let drop_trigger = sqlx::query("DROP TRIGGER stall_intents ON order_intents").execute(&pool);
    tokio::time::timeout(Duration::from_secs(5), drop_trigger)
        .await
        .expect("the cancelled writes released their locks")
        .unwrap();
    proxy.set_mode(ProxyMode::Pass);

    lands_within(&journal, "c", Duration::from_secs(5))
        .await
        .expect("the pool recovered");
    assert_eq!(attempt_states_of(&pool, "c").await, vec!["started"]);
}

/// Hold a lock on `table` that stalls anything reading it.
async fn lock_table(pool: &PgPool, table: &str) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// The read side of the write test above: both connections are inside a
/// stalled evidence read when the replies stop, and the outer timeout drops
/// them. Retired, not returned, they leave the pool usable.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn two_cancelled_reads_do_not_exhaust_the_journal_pool(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);
    journal.check_ready().await.expect("ready");
    let lock = lock_table(&pool, "submit_attempts").await;

    let cut_off = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        proxy.set_mode(ProxyMode::Blackhole);
    };
    let (first, second, ()) =
        tokio::join!(journal.attempt_evidence("a"), journal.attempt_evidence("b"), cut_off);
    assert_eq!(first, AttemptEvidence::Unavailable);
    assert_eq!(second, AttemptEvidence::Unavailable);

    // The reads' answers are produced now, and swallowed.
    lock.rollback().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    proxy.set_mode(ProxyMode::Pass);

    lands_within(&journal, "c", Duration::from_secs(5))
        .await
        .expect("the pool recovered");
}

/// A startup probe that acquires its connection and then stalls gives up at
/// the deadline of the whole check.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_readiness_check_gives_up_at_its_deadline_on_a_stalled_probe(pool: PgPool) {
    let backend =
        PgJournalBackend::new(&pool, Uuid::now_v7()).with_ready_wait(Duration::from_secs(1));
    let lock = lock_table(&pool, "order_intents").await;

    let refused = tokio::time::timeout(Duration::from_secs(4), backend.check_ready())
        .await
        .expect("the check has a deadline of its own");
    assert!(refused.is_err(), "{refused:?}");

    lock.rollback().await.unwrap();
    backend.check_ready().await.expect("the database is back");
}

/// Probes stalled with their replies lost: both connections are retired at
/// the deadline, and the pool works afterwards.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn stalled_readiness_probes_leave_the_pool_usable(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend =
        PgJournalBackend::new(&proxied, Uuid::now_v7()).with_ready_wait(Duration::from_secs(1));
    backend.check_ready().await.expect("ready");
    let lock = lock_table(&pool, "order_intents").await;

    let cut_off = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        proxy.set_mode(ProxyMode::Blackhole);
    };
    let (first, second, ()) = tokio::join!(backend.check_ready(), backend.check_ready(), cut_off);
    assert!(first.is_err() && second.is_err(), "{first:?} {second:?}");

    lock.rollback().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    proxy.set_mode(ProxyMode::Pass);

    let journal = Arc::new(OrderJournal::new(Arc::new(backend), &venue()));
    lands_within(&journal, "c", Duration::from_secs(5))
        .await
        .expect("the pool recovered");
}

/// Both connections are lost outright and the pool comes back with them.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_journal_pool_recovers_after_losing_both_connections(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = real_journal(&proxied);
    journal.check_ready().await.expect("ready");

    proxy.set_mode(ProxyMode::Sever);
    for id in ["a", "b"] {
        let (operation, intent) = opened(&journal, id, IntentKind::Entry).await;
        let refusal = operation
            .scope(journal.before_send(&intent, WriteClass::Strict))
            .await
            .unwrap_err();
        assert!(matches!(refusal, SendRefusal::NotSent(_)), "{refusal:?}");
    }
    proxy.set_mode(ProxyMode::Pass);

    lands_within(&journal, "c", Duration::from_secs(5))
        .await
        .expect("the pool recovered");
}

// Fault targeting

/// The negative control: an autocommit read of the target table, then a
/// transaction that only touches another, on one connection. The commit is not
/// the targeted one.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_targeted_fault_ignores_an_autocommit_statement_before_the_transaction(pool: PgPool) {
    let (proxy, proxied) = through_proxy(&pool).await;
    sqlx::query("CREATE TABLE harness_probe (id INT)").execute(&pool).await.unwrap();
    let mut connection = proxied.acquire().await.unwrap();

    proxy.set_mode(ProxyMode::DropCommitAckOf("order_intents"));
    sqlx::query("SELECT 1 FROM order_intents LIMIT 0")
        .execute(&mut *connection)
        .await
        .unwrap();
    let mut tx = connection.begin().await.unwrap();
    sqlx::query("INSERT INTO harness_probe VALUES (1)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.expect("not the targeted transaction");

    assert_eq!(proxy.mode(), ProxyMode::DropCommitAckOf("order_intents"), "still armed");
    assert_eq!(proxy.stats().commit_acks_dropped, 0);
}

// Generated interleaving 1
// -------------------------------------------------------

/// The next commit's fate, over the real backend.
#[derive(Clone, Copy, Debug)]
enum Fault {
    Pass,
    Known,
    AckLost,
    NotLanded,
}

struct Injecting {
    real: PgJournalBackend,
    next: Mutex<Fault>,
}

impl Injecting {
    fn arm(&self, fault: Fault) {
        *self.next.lock().unwrap() = fault;
    }
}

#[async_trait]
impl JournalBackend for Injecting {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        let fault = std::mem::replace(&mut *self.next.lock().unwrap(), Fault::Pass);
        match fault {
            Fault::Pass => self.real.commit(batch).await,
            Fault::Known => Err(CommitFailure::Known("injected".to_string())),
            Fault::AckLost => {
                self.real.commit(batch).await?;
                Err(CommitFailure::Uncertain("injected ack lost".to_string()))
            },
            Fault::NotLanded => Err(CommitFailure::Uncertain("injected".to_string())),
        }
    }

    async fn attempt_states(
        &self,
        venue_id: &str,
        account_scope: &str,
        client_order_id: &str,
    ) -> Result<Vec<bool>, String> {
        self.real.attempt_states(venue_id, account_scope, client_order_id).await
    }
}

#[derive(Clone, Copy, Debug)]
enum VenueAnswer {
    Accepted,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Attempt {
        id: u8,
        strict: bool,
        fault: Fault,
        answer: VenueAnswer,
    },
    Finish {
        id: u8,
        fault: Fault,
    },
    Replay,
    Lookup {
        id: u8,
    },
    Restart,
}

fn fault_strategy() -> impl Strategy<Value = Fault> {
    prop_oneof![
        3 => Just(Fault::Pass),
        1 => Just(Fault::Known),
        1 => Just(Fault::AckLost),
        1 => Just(Fault::NotLanded),
    ]
}

fn step_strategy() -> impl Strategy<Value = Step> {
    let id = 0..4u8;
    let answer = prop_oneof![
        Just(VenueAnswer::Accepted),
        Just(VenueAnswer::Rejected),
        Just(VenueAnswer::Unknown)
    ];
    prop_oneof![
        5 => (id.clone(), any::<bool>(), fault_strategy(), answer)
            .prop_map(|(id, strict, fault, answer)| Step::Attempt { id, strict, fault, answer }),
        2 => (id.clone(), fault_strategy()).prop_map(|(id, fault)| Step::Finish { id, fault }),
        2 => Just(Step::Replay),
        4 => id.prop_map(|id| Step::Lookup { id }),
        1 => Just(Step::Restart),
    ]
}

/// One process: the real journal, the real ledger, and a fault in front of the
/// real persistence.
struct Process {
    backend: Arc<Injecting>,
    journal: Arc<OrderJournal>,
    ledger: SubmitAttemptLedger,
    operation: Operation,
    intents: HashMap<u8, JournaledIntent>,
}

impl Process {
    fn start(pool: &PgPool) -> Self {
        let backend = Arc::new(Injecting {
            real: PgJournalBackend::new(pool, Uuid::now_v7()),
            next: Mutex::new(Fault::Pass),
        });
        let journal = Arc::new(OrderJournal::new(backend.clone(), &venue()));
        let ledger = SubmitAttemptLedger::new(16);
        ledger.attach_journal(journal.clone());
        let operation = journal.begin_operation();
        Self {
            backend,
            journal,
            ledger,
            operation,
            intents: HashMap::new(),
        }
    }
}

/// Ground truth the journal cannot see.
#[derive(Default)]
struct Truth {
    sent: HashMap<u8, u32>,
    unresolved: HashMap<u8, u32>,
    uncertain_commit: HashSet<u8>,
    /// Opened by an earlier process, or finished: nothing more may go out.
    closed: HashSet<u8>,
}

async fn row_exists(pool: &PgPool, id: &str) -> bool {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM order_intents WHERE client_order_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    count > 0
}

/// Generated sequential schedules over the real ledger and real persistence:
/// `Absent` never appears while an attempt is unresolved or unlanded, and
/// nothing is sent under an id after an uncertain commit on it. Each step is
/// awaited in turn; the concurrent test below covers overlapping futures.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn absent_and_id_reuse_hold_under_generated_sequential_schedules(pool: PgPool) {
    let rule = VenueFinalityRule::EveryAttemptRejected;
    let schedules = proptest::collection::vec(step_strategy(), 1..28);
    let mut runner = TestRunner::deterministic();

    for schedule_number in 0..64 {
        let schedule = schedules.new_tree(&mut runner).unwrap().current();
        let client_id = |id: u8| format!("s{schedule_number}-{id}");
        let mut process = Process::start(&pool);
        let mut truth = Truth::default();

        for step in &schedule {
            let case = format!("{step:?} in {schedule:?}");
            match *step {
                Step::Attempt { id, strict, fault, answer } => {
                    let client = client_id(id);
                    if truth.closed.contains(&id) {
                        // A later attempt can only be tried against a landed row,
                        // which refuses it. An id with no row is never minted
                        // again: ids are fresh.
                        if !row_exists(&pool, &client).await {
                            continue;
                        }
                        let operation = &process.operation;
                        let result = operation
                            .scope(async {
                                let intent =
                                    process.journal.open_intent(spec(&client, IntentKind::Exit));
                                let result =
                                    process.journal.before_send(&intent, WriteClass::Bounded).await;
                                process.journal.finish_failed(&intent).await;
                                result
                            })
                            .await;
                        assert!(result.is_err(), "reused a spent id: {case}");
                        continue;
                    }
                    if !process.intents.contains_key(&id) {
                        let intent = process
                            .operation
                            .scope(async {
                                process.journal.open_intent(spec(&client, IntentKind::Exit))
                            })
                            .await;
                        process.intents.insert(id, intent);
                    }
                    let intent = process.intents[&id].clone();
                    process.backend.arm(fault);
                    let class = if strict {
                        WriteClass::Strict
                    } else {
                        WriteClass::Bounded
                    };
                    let authorized =
                        process.operation.scope(process.journal.before_send(&intent, class)).await;
                    match authorized {
                        Ok(()) => {
                            assert!(
                                !truth.uncertain_commit.contains(&id),
                                "sent after an uncertain commit: {case}"
                            );
                            *truth.sent.entry(id).or_default() += 1;
                            if !matches!(answer, VenueAnswer::Rejected) {
                                *truth.unresolved.entry(id).or_default() += 1;
                            }
                            let _ = process
                                .operation
                                .scope(process.ledger.submit(
                                    &client,
                                    |rejected: &bool| *rejected,
                                    async {
                                        match answer {
                                            VenueAnswer::Accepted => Ok::<(), bool>(()),
                                            VenueAnswer::Rejected => Err(true),
                                            VenueAnswer::Unknown => Err(false),
                                        }
                                    },
                                ))
                                .await;
                        },
                        Err(SendRefusal::UncertainCommit(_)) => {
                            truth.uncertain_commit.insert(id);
                        },
                        Err(SendRefusal::NotSent(_)) => {},
                    }
                },
                Step::Finish { id, fault } => {
                    if let Some(intent) = process.intents.remove(&id) {
                        process.backend.arm(fault);
                        process.operation.scope(process.journal.finish_failed(&intent)).await;
                        process.backend.arm(Fault::Pass);
                        truth.closed.insert(id);
                    }
                },
                Step::Replay => {
                    process.backend.arm(Fault::Pass);
                    process.journal.flush_pending().await;
                },
                Step::Lookup { id } => {
                    let client = client_id(id);
                    let evidence = process.ledger.final_evidence(&client).await;
                    if matches!(rule.classify_not_found(evidence), OrderLookup::Absent { .. }) {
                        assert_eq!(
                            truth.unresolved.get(&id).copied().unwrap_or(0),
                            0,
                            "Absent over an unresolved attempt: {case}"
                        );
                        assert!(
                            truth.sent.get(&id).copied().unwrap_or(0) > 0,
                            "Absent over nothing sent: {case}"
                        );
                        assert!(!process.journal.is_live(&client), "unlanded: {case}");
                        let states = attempt_states_of(&pool, &client).await;
                        assert!(
                            !states.is_empty() && states.iter().all(|s| s == "rejected"),
                            "{states:?}: {case}"
                        );
                    }
                },
                Step::Restart => {
                    truth.closed.extend(process.intents.keys().copied());
                    process = Process::start(&pool);
                },
            }
        }
    }
}

// Concurrent persistence futures

#[derive(Default)]
struct ConcurrentTruth {
    sent: HashSet<String>,
    unresolved: HashSet<String>,
}

#[derive(Clone, Copy, Debug)]
struct IntentPlan {
    answer: VenueAnswerKind,
    abort_after_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
enum VenueAnswerKind {
    Accepted,
    Rejected,
    Unknown,
}

/// One order's whole path, in its own task so it can be aborted at any await.
async fn run_planned_intent(
    journal: Arc<OrderJournal>,
    ledger: Arc<SubmitAttemptLedger>,
    truth: Arc<Mutex<ConcurrentTruth>>,
    id: String,
    answer: VenueAnswerKind,
) {
    let operation = journal.begin_operation();
    operation
        .scope(async {
            let intent = journal.open_intent(spec(&id, IntentKind::Exit));
            if journal.before_send(&intent, WriteClass::Bounded).await.is_err() {
                journal.finish_failed(&intent).await;
                return;
            }
            {
                let mut truth = truth.lock().unwrap();
                truth.sent.insert(id.clone());
                if !matches!(answer, VenueAnswerKind::Rejected) {
                    truth.unresolved.insert(id.clone());
                }
            }
            let answered = ledger
                .submit(&id, |rejected: &bool| *rejected, async {
                    tokio::task::yield_now().await;
                    match answer {
                        VenueAnswerKind::Accepted => Ok(()),
                        VenueAnswerKind::Rejected => Err(true),
                        VenueAnswerKind::Unknown => Err(false),
                    }
                })
                .await;
            match answered {
                Ok(()) => journal.finish_success(&intent, Some(json!({"order": 1}))).await,
                Err(_) => journal.finish_failed(&intent).await,
            }
        })
        .await;
}

/// Overlapping orders, the retry loop and lookups on the real backend, with
/// tasks aborted at arbitrary awaits. Afterwards nothing is stranded, every
/// send has a row with an ending, `not_sent` is only ever true of an unsent
/// order, and `Absent` never answered for an order with an unresolved send.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn concurrent_orders_retries_and_lookups_survive_cancellation(pool: PgPool) {
    let rule = VenueFinalityRule::EveryAttemptRejected;
    let answer = prop_oneof![
        Just(VenueAnswerKind::Accepted),
        Just(VenueAnswerKind::Rejected),
        Just(VenueAnswerKind::Unknown)
    ];
    let plan = (answer, proptest::option::of(0..6u64))
        .prop_map(|(answer, abort_after_ms)| IntentPlan { answer, abort_after_ms });
    let plans = proptest::collection::vec(plan, 6..14);
    let mut runner = TestRunner::deterministic();

    for case in 0..24 {
        let plans = plans.new_tree(&mut runner).unwrap().current();
        let journal = real_journal(&pool);
        let ledger = Arc::new(SubmitAttemptLedger::new(64));
        ledger.attach_journal(journal.clone());
        let truth = Arc::new(Mutex::new(ConcurrentTruth::default()));
        let ids: Vec<String> = (0..plans.len()).map(|i| format!("c{case}-{i}")).collect();

        let stop = Arc::new(AtomicBool::new(false));
        let retries = tokio::spawn({
            let (journal, stop) = (journal.clone(), stop.clone());
            async move {
                while !stop.load(Ordering::SeqCst) {
                    journal.flush_pending().await;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        });
        let lookups = tokio::spawn({
            let (ledger, truth, ids, stop) =
                (ledger.clone(), truth.clone(), ids.clone(), stop.clone());
            async move {
                while !stop.load(Ordering::SeqCst) {
                    for id in &ids {
                        let evidence = ledger.final_evidence(id).await;
                        if matches!(rule.classify_not_found(evidence), OrderLookup::Absent { .. }) {
                            let truth = truth.lock().unwrap();
                            assert!(truth.sent.contains(id), "Absent over nothing sent: {id}");
                            assert!(!truth.unresolved.contains(id), "Absent over unresolved: {id}");
                        }
                    }
                    // A live intent answers from memory, with no await to yield at.
                    tokio::task::yield_now().await;
                }
            }
        });

        let mut orders = Vec::with_capacity(plans.len());
        for (id, plan) in ids.iter().zip(&plans) {
            let order = tokio::spawn(run_planned_intent(
                journal.clone(),
                ledger.clone(),
                truth.clone(),
                id.clone(),
                plan.answer,
            ));
            orders.push((order, plan.abort_after_ms));
        }
        for (order, abort_after_ms) in orders {
            if let Some(after) = abort_after_ms {
                tokio::time::sleep(Duration::from_millis(after)).await;
                order.abort();
            }
            if let Err(error) = order.await {
                assert!(error.is_cancelled(), "an order task panicked: {error}");
            }
        }
        stop.store(true, Ordering::SeqCst);
        retries.await.unwrap();
        lookups.await.unwrap();

        for _ in 0..400 {
            if journal.retry_set_depth() == 0 {
                break;
            }
            journal.flush_pending().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let case_text = format!("case {case}: {plans:?}");
        assert_eq!(journal.retry_set_depth(), 0, "{case_text}");
        assert_eq!(journal.operations_in_flight(), 0, "{case_text}");
        assert!(ids.iter().all(|id| !journal.is_live(id)), "stranded: {case_text}");

        let truth = std::mem::take(&mut *truth.lock().unwrap());
        for id in &ids {
            let row: Option<(Option<String>, bool, bool)> = sqlx::query_as(
                "SELECT venue_outcome, not_sent, uncertain_at IS NOT NULL
                 FROM order_intents WHERE client_order_id = $1",
            )
            .bind(id)
            .fetch_optional(&pool)
            .await
            .unwrap();
            match row {
                None => assert!(!truth.sent.contains(id), "a send with no row: {id} {case_text}"),
                Some((outcome, not_sent, uncertain)) => {
                    assert!(
                        outcome.is_some() || not_sent || uncertain,
                        "no ending: {id} {case_text}"
                    );
                    assert!(!(not_sent && truth.sent.contains(id)), "false no-send: {id}");
                },
            }
        }
    }
}
