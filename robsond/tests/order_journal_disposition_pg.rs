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

// ---- ADR-0056 step 7, PR F: incidents and the audited release

use robson_exec::{
    gate::{Incident, IncidentReason},
    order_journal::ReleaseRecord,
};

/// An intent of `tenant` with one attempt, its outcome `outcome`.
async fn seeded_for(
    pool: &PgPool,
    tenant: Uuid,
    id: &'static str,
    outcome: Option<OutcomeRow>,
) -> Uuid {
    let intent = intent_row(id);
    let intent_id = intent.intent_id;
    PgJournalBackend::new(pool, tenant)
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

fn uncertain_at(generation: i64) -> Option<OutcomeRow> {
    Some(OutcomeRow {
        uncertain_at: Some(Utc::now()),
        uncertain_generation: Some(generation),
        ..OutcomeRow::default()
    })
}

fn incident(reason: IncidentReason, count: i64, first: i64, last: i64) -> Incident {
    Incident {
        reason,
        first_at: Utc::now(),
        last_at: Utc::now(),
        count,
        first_generation: first,
        last_generation: last,
    }
}

fn release_at(generation: i64) -> ReleaseRecord {
    ReleaseRecord {
        decision_id: Uuid::now_v7(),
        generation,
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
        note: "reviewed the pending list".to_string(),
        treatment: "unbooked_accepted",
        decided_at: Utc::now(),
    }
}

/// Open incidents of `tenant`: reason, count, first and last generation.
async fn open_incidents(pool: &PgPool, tenant: Uuid) -> Vec<(String, i64, i64, i64)> {
    sqlx::query_as(
        "SELECT reason, count, first_generation, last_generation FROM recovery_incidents \
         WHERE tenant_id = $1 AND released_at IS NULL ORDER BY reason",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// An incident's upsert only ever grows its row, so a replay, an older
/// snapshot landing late or a reordering converge; against a released row it
/// opens a new one.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_incident_upsert_converges_and_reopens_after_a_release(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let backend = PgJournalBackend::new(&pool, tenant);
    let reason = IncidentReason::UncertainOutcome;
    backend.upsert_incident(&incident(reason, 2, 10, 11)).await.unwrap();
    backend.upsert_incident(&incident(reason, 2, 10, 11)).await.unwrap();
    backend.upsert_incident(&incident(reason, 1, 10, 10)).await.unwrap();
    assert_eq!(open_incidents(&pool, tenant).await, [(
        "uncertain_outcome".to_string(),
        2,
        10,
        11
    )]);
    // Another tenant's incidents are its own.
    PgJournalBackend::new(&pool, Uuid::now_v7())
        .upsert_incident(&incident(reason, 7, 1, 1))
        .await
        .unwrap();

    let counts = backend.release(&release_at(11)).await.unwrap();
    assert_eq!(counts.incidents_released, 1);
    assert!(open_incidents(&pool, tenant).await.is_empty());
    backend.upsert_incident(&incident(reason, 3, 10, 12)).await.unwrap();
    assert_eq!(open_incidents(&pool, tenant).await, [(
        "uncertain_outcome".to_string(),
        3,
        10,
        12
    )]);
}

/// Design §2.7: the release's one transaction tombstones exactly the intents
/// stamped at or below its generation with no outcome, not proven unsent and
/// no disposition, as accepted risk with the operator's identity, note and
/// treatment; releases the incidents whose last generation is at or below it;
/// and appends its decision. It sets no venue outcome. What it covered no
/// longer blocks boundary B; the rest still does.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_release_tombstones_what_it_covers_and_nothing_else(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let backend = PgJournalBackend::new(&pool, tenant);
    let covered = seeded_for(&pool, tenant, "covered", uncertain_at(20)).await;
    seeded_for(&pool, tenant, "later", uncertain_at(30)).await;
    seeded_for(&pool, tenant, "unstamped", None).await;
    seeded_for(
        &pool,
        tenant,
        "settled",
        Some(OutcomeRow {
            venue_outcome: Some(robson_exec::order_journal::VenueOutcome::Success),
            ..OutcomeRow::default()
        }),
    )
    .await;
    let disposed = seeded_for(&pool, tenant, "disposed", uncertain_at(20)).await;
    record_disposition(&pool, &accepted_risk(disposed)).await.unwrap();
    seeded_for(&pool, Uuid::now_v7(), "theirs", uncertain_at(20)).await;
    // A stamp with no `uncertain_at` is never tombstoned.
    sqlx::query(
        "UPDATE order_intents SET uncertain_generation = 20 WHERE client_order_id = 'unstamped'",
    )
    .execute(&pool)
    .await
    .unwrap();
    backend
        .upsert_incident(&incident(IncidentReason::UncleanStart, 1, 20, 20))
        .await
        .unwrap();
    backend
        .upsert_incident(&incident(IncidentReason::UncertainOutcome, 2, 20, 30))
        .await
        .unwrap();

    let release = release_at(25);
    let counts = backend.release(&release).await.unwrap();
    assert_eq!((counts.intents_tombstoned, counts.incidents_released), (1, 1));

    let tombstone: (Option<String>, String, String, String, String, String) = sqlx::query_as(
        "SELECT venue_outcome, disposition_kind, disposition_treatment, risk_accepted_by, \
         risk_accepted_auth, risk_accepted_note FROM order_intents WHERE intent_id = $1",
    )
    .bind(covered)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        tombstone,
        (
            None,
            "accepted_risk".to_string(),
            "unbooked_accepted".to_string(),
            "operator@example.test".to_string(),
            "verified".to_string(),
            "reviewed the pending list".to_string(),
        )
    );
    let untouched: Vec<String> = sqlx::query_scalar(
        "SELECT client_order_id FROM order_intents WHERE disposition_kind IS NULL \
         ORDER BY client_order_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(untouched, ["later", "settled", "theirs", "unstamped"]);
    assert_eq!(blockers(&pool).await, ["later", "theirs", "unstamped"]);
    assert_eq!(open_incidents(&pool, tenant).await, [(
        "uncertain_outcome".to_string(),
        2,
        20,
        30
    )]);

    let decision: (String, String, serde_json::Value, i64) = sqlx::query_as(
        "SELECT kind, actor, payload, (SELECT COUNT(*) FROM recovery_incidents \
             WHERE decision_id = $1) FROM journal_decisions WHERE decision_id = $1",
    )
    .bind(release.decision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (decision.0.as_str(), decision.1.as_str(), decision.3),
        ("release", "operator@example.test", 1)
    );
    assert_eq!(decision.2["generation"], 25);
    assert_eq!(decision.2["intents_tombstoned"], 1);
}

/// The release is one explicit transaction: one simple-query `COMMIT`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_release_is_one_simple_query_commit(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seeded_for(&pool, tenant, "a", uncertain_at(5)).await;
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

    let counts = PgJournalBackend::new(&proxied, tenant).release(&release_at(5)).await.unwrap();

    assert_eq!(counts.intents_tombstoned, 1);
    assert_eq!(proxy.stats().commits, 1);
}
