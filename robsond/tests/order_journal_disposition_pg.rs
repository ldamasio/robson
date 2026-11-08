//! Dispositions against real PostgreSQL (ADR-0056 step 7, PR J).
//!
//! The database comes from the environment, as for every `DATABASE_URL`-gated
//! test: `sqlx::test` creates one per test.

#![cfg(feature = "postgres")]

use chrono::Utc;
use robson_exec::order_journal::{
    AttemptRow, IntentKind, IntentRow, JournalBackend, JournalBatch, OutcomeRow,
    DEFAULT_ACCOUNT_SCOPE,
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use robsond::order_journal_store::{
    record_disposition, Disposition, DispositionError, PgJournalBackend, TerminalEvidence,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

fn intent_row(id: &str) -> IntentRow {
    IntentRow {
        intent_id: Uuid::now_v7(),
        venue_id: "test:venue".to_string(),
        account_scope: DEFAULT_ACCOUNT_SCOPE.to_string(),
        client_order_id: id.to_string(),
        position_id: None,
        kind: IntentKind::Exit,
        cancels_order_id: None,
        params: json!({ "quantity": "0.1" }),
        created_at: Utc::now(),
    }
}

async fn seeded(pool: &PgPool, id: &'static str, outcome: Option<OutcomeRow>) -> Uuid {
    let intent = intent_row(id);
    let intent_id = intent.intent_id;
    let backend = PgJournalBackend::new(pool, Uuid::now_v7());
    backend
        .commit(&JournalBatch {
            intent,
            attempts: vec![AttemptRow {
                attempt_id: Uuid::now_v7(),
                created_at: Utc::now(),
                rejected_at: None,
            }],
            outcome,
        })
        .await
        .unwrap();
    intent_id
}

async fn blockers(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT client_order_id FROM order_intents_blocking_rollback ORDER BY client_order_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn accepted_risk(intent_id: Uuid) -> Disposition {
    Disposition {
        intent_id,
        terminal_evidence: None,
        treatment: "unbooked_accepted",
        note: "accepted".to_string(),
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
    }
}

fn terminal(intent_id: Uuid, order_ref: &str, filled_qty: Decimal) -> Disposition {
    Disposition {
        terminal_evidence: Some(TerminalEvidence {
            order_ref: order_ref.to_string(),
            filled_qty,
            document: json!({
                "order_ref": order_ref,
                "terminal_status": "filled",
                "venue_record": { "status": "FILLED" },
            }),
        }),
        ..accepted_risk(intent_id)
    }
}

/// Every outstanding intent blocks until it has a disposition.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn boundary_b_holds_only_when_every_outstanding_intent_has_a_disposition(pool: PgPool) {
    let uncertain = seeded(
        &pool,
        "uncertain",
        Some(OutcomeRow {
            uncertain_at: Some(Utc::now()),
            ..OutcomeRow::default()
        }),
    )
    .await;
    let unstamped = seeded(&pool, "unstamped", None).await;
    assert_eq!(blockers(&pool).await, vec!["uncertain", "unstamped"]);

    let receipt = record_disposition(&pool, &accepted_risk(uncertain)).await.unwrap();
    assert_eq!(receipt.boundary_b_blockers, 1);
    assert_eq!(blockers(&pool).await, vec!["unstamped"]);

    let wrong = record_disposition(&pool, &terminal(unstamped, "another", dec!(0.1))).await;
    assert!(matches!(wrong, Err(DispositionError::EvidenceForAnotherOrder)), "{wrong:?}");
    let too_much = record_disposition(&pool, &terminal(unstamped, "unstamped", dec!(0.2))).await;
    assert!(matches!(too_much, Err(DispositionError::ImpossibleQuantity)), "{too_much:?}");
    let receipt = record_disposition(&pool, &terminal(unstamped, "unstamped", dec!(0.1)))
        .await
        .unwrap();
    assert_eq!(receipt.boundary_b_blockers, 0);
    assert_eq!(blockers(&pool).await, Vec::<String>::new());
}

/// A disposition decides nothing about the venue. It can be corrected, and a
/// repeated request appends a decision; the decisions keep what was said.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_disposition_never_writes_a_venue_outcome_and_is_append_only(pool: PgPool) {
    let intent_id = seeded(&pool, "a", None).await;

    record_disposition(&pool, &accepted_risk(intent_id)).await.unwrap();
    record_disposition(&pool, &accepted_risk(intent_id)).await.unwrap();
    record_disposition(&pool, &terminal(intent_id, "a", dec!(0.1))).await.unwrap();

    let outcome: (Option<String>, bool, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT venue_outcome, not_sent, venue_evidence FROM order_intents WHERE intent_id = $1",
    )
    .bind(intent_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outcome, (None, false, None));
    let decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM journal_decisions WHERE intent_id = $1")
            .bind(intent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(decisions, 3);
    let accepted: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT risk_accepted_at FROM order_intents WHERE intent_id = $1")
            .bind(intent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(accepted, None, "the terminal evidence replaced the accepted risk");
}

/// A definite outcome or a proven no-send is not outstanding.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_settled_intent_takes_no_disposition(pool: PgPool) {
    let not_sent =
        seeded(&pool, "n", Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() })).await;
    let refused = record_disposition(&pool, &accepted_risk(not_sent)).await;
    assert!(matches!(refused, Err(DispositionError::NotOutstanding)), "{refused:?}");
    let missing = record_disposition(&pool, &accepted_risk(Uuid::now_v7())).await;
    assert!(matches!(missing, Err(DispositionError::NotFound)), "{missing:?}");
}

/// A disposition is one explicit transaction too, and the proxy counts it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_disposition_is_one_simple_query_commit(pool: PgPool) {
    let intent_id = seeded(&pool, "a", None).await;
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
    let proxied = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();

    record_disposition(&proxied, &accepted_risk(intent_id)).await.unwrap();

    assert_eq!(proxy.stats().commits, 1);
}
