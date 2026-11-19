//! Balance-only PostgreSQL journal contracts (ADR-0056 step 8e-3).
use chrono::{DateTime, NaiveDate};
use robson_domain::MonthlyBudgetModel;
use robson_exec::{
    order_journal::{AdmissionRow, BalanceBatch},
    venue_ledger::LedgerError,
};
use robsond::venue_ledger_store::PgVenueLedger;
use rust_decimal_macros::dec;
use sqlx::Row;

use super::*;

const SPOT: &str = VenueId::BINANCE_SPOT;
// empty_budget.rs reads its venue from its parent module.
const VENUE: &str = SPOT;

#[path = "../venue_ledger_schema/empty_budget.rs"]
mod empty_budget;

/// Migration 38 refuses Spot clock rows until the venue budget is initialized;
/// initialize it once per fresh test database, before any intent exists.
async fn spot_budget_ready(pool: &PgPool) {
    let state: Option<String> = sqlx::query_scalar(
        "SELECT initialization_state FROM budget_venue_state WHERE venue_id = $1",
    )
    .bind(SPOT)
    .fetch_optional(pool)
    .await
    .unwrap();
    if state.as_deref() != Some("complete") {
        empty_budget::initialize(pool, Utc::now()).await.unwrap();
    }
}
const BOUND: Duration = Duration::from_millis(500);

fn terms(position_id: Uuid) -> AdmissionRow {
    AdmissionRow {
        position_id,
        symbol: "ETHUSDT".to_string(),
        decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        taker_bps: dec!(10),
        gap_bps: dec!(10),
        notional_margin_bps: dec!(2),
        min_notional: dec!(10),
        lot_step: dec!(0.001),
        fee_asset_policy: "base".to_string(),
        envelope_source: "test rules".to_string(),
        envelope_as_of: Utc::now(),
        envelope_hash: "test-hash".to_string(),
        planned_qty: dec!(1),
        requested_qty: dec!(1.01),
        reference_price: dec!(1900),
        remainder_reserve: dec!(0.01),
        admission_charge: dec!(110),
        initial_trigger: dec!(1800),
    }
}

pub(super) fn balance_batch(id: &str, kind: IntentKind) -> JournalBatch {
    let mut batch = spot_batch(id);
    batch.intent.kind = kind;
    batch.balance = Some(BalanceBatch {
        admission: None,
        handed_off: false,
        commit_bound: BOUND,
    });
    batch
}

pub(super) async fn seed_account(pool: &PgPool, tenant: Uuid, venue: &str) {
    if venue == SPOT {
        spot_budget_ready(pool).await;
    }
    PgVenueLedger::new(pool, tenant)
        .ensure_accounting_row(&VenueId::new(venue), MonthlyBudgetModel::HwmV1)
        .await
        .unwrap();
}

async fn seed_route(pool: &PgPool, tenant: Uuid, position: Uuid) {
    PgVenueLedger::new(pool, tenant)
        .insert_route(position, &VenueId::new(SPOT), DEFAULT_ACCOUNT_SCOPE, Utc::now())
        .await
        .unwrap();
}

async fn entry_batch(pool: &PgPool, tenant: Uuid, id: &str) -> JournalBatch {
    let position = Uuid::now_v7();
    seed_route(pool, tenant, position).await;
    let mut batch = balance_batch(id, IntentKind::Entry);
    batch.intent.position_id = Some(position);
    batch.balance.as_mut().unwrap().admission = Some(Arc::new(terms(position)));
    batch
}

pub(super) fn journal_on(
    pool: &PgPool,
    tenant: Uuid,
    balance: bool,
    one_attempt: bool,
    venue: &str,
) -> Arc<OrderJournal> {
    Arc::new(
        OrderJournal::new(Arc::new(PgJournalBackend::new(pool, tenant)), &VenueId::new(venue))
            .with_balance_account(balance)
            .with_one_attempt_per_client_id(one_attempt),
    )
}

pub(super) async fn version(pool: &PgPool, tenant: Uuid, venue: &str) -> i64 {
    sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2")
        .bind(tenant)
        .bind(venue)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn cohort(pool: &PgPool, position: Uuid) -> Option<NaiveDate> {
    sqlx::query_scalar("SELECT cohort_month FROM balance_legs WHERE position_id=$1")
        .bind(position)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_for_lock(pool: &PgPool, query: &str) -> i32 {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let pid = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE $1 LIMIT 1")
                .bind(format!("%{query}%")).fetch_optional(pool).await.unwrap();
            if let Some(pid) = pid { return pid; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("writer did not reach its expected lock")
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_spot_commit_bumps_the_version_once(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let backend = PgJournalBackend::new(&pool, tenant);
    let mut batch = entry_batch(&pool, tenant, "entry").await;
    for expected in 1..=3 {
        if expected == 2 {
            batch.outcome = Some(OutcomeRow {
                venue_outcome: Some(VenueOutcome::Success),
                ..OutcomeRow::default()
            });
        }
        backend.commit(&batch).await.unwrap();
        assert_eq!(version(&pool, tenant, SPOT).await, expected);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM spot_intent_clock")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM balance_leg_admissions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_spot_commit_without_its_accounting_row_lands_nothing(pool: PgPool) {
    let backend = PgJournalBackend::new(&pool, Uuid::now_v7());
    // Also exercise a result-like batch without attempts or admission: no
    // downstream ledger FK can substitute for refusing the zero-row bump.
    for has_attempt in [true, false] {
        let mut batch = balance_batch("missing", IntentKind::Exit);
        if !has_attempt {
            batch.attempts.clear();
        }
        assert!(batch.balance.as_ref().unwrap().admission.is_none());
        assert!(matches!(backend.commit(&batch).await, Err(CommitFailure::Known(_))));
    }
    for table in [
        "order_intents",
        "submit_attempts",
        "spot_intent_clock",
        "balance_legs",
        "balance_leg_admissions",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "{table}"
        );
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_entry_s_admission_lands_with_its_intent_or_not_at_all(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let backend = PgJournalBackend::new(&pool, tenant);
    let mut batch = entry_batch(&pool, tenant, "entry").await;
    let admission = batch.balance.as_ref().unwrap().admission.as_ref().unwrap().clone();
    Arc::make_mut(batch.balance.as_mut().unwrap().admission.as_mut().unwrap()).fee_asset_policy =
        "invalid".into();
    assert!(matches!(backend.commit(&batch).await, Err(CommitFailure::Known(_))));
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_intents")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM spot_intent_clock")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    batch.balance.as_mut().unwrap().admission = Some(admission.clone());
    backend.commit(&batch).await.unwrap();
    let row = sqlx::query("SELECT entry_intent_id, admission_charge, remainder_reserve, initial_trigger, symbol, requested_qty FROM balance_leg_admissions WHERE position_id=$1")
        .bind(admission.position_id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<Uuid, _>("entry_intent_id"), batch.intent.intent_id);
    assert_eq!(row.get::<Decimal, _>("admission_charge"), admission.admission_charge);
    assert_eq!(row.get::<Decimal, _>("remainder_reserve"), admission.remainder_reserve);
    assert_eq!(row.get::<Decimal, _>("initial_trigger"), admission.initial_trigger);
    assert_eq!(row.get::<Decimal, _>("requested_qty"), admission.requested_qty);
    assert_eq!(row.get::<String, _>("symbol"), admission.symbol);
    assert_eq!(cohort(&pool, admission.position_id).await, None);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_attempt_on_a_balance_journal_has_its_clock_row(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let backend = PgJournalBackend::new(&pool, tenant);
    for (n, kind) in [
        IntentKind::Exit,
        IntentKind::Protection,
        IntentKind::Cancel,
        IntentKind::SweepClose,
    ]
    .into_iter()
    .enumerate()
    {
        let batch = balance_batch(&format!("clock-{n}"), kind);
        backend.commit(&batch).await.unwrap();
        backend.commit(&batch).await.unwrap();
        let row=sqlx::query("SELECT intent_id, tenant_id, venue_id, attempt_created_at, clock_sample_id, clock_verdict FROM spot_intent_clock WHERE attempt_id=$1")
            .bind(batch.attempts[0].attempt_id).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<Uuid, _>("intent_id"), batch.intent.intent_id);
        assert_eq!(row.get::<Uuid, _>("tenant_id"), tenant);
        assert_eq!(row.get::<String, _>("venue_id"), SPOT);
        assert_eq!(
            row.get::<DateTime<Utc>, _>("attempt_created_at").timestamp_micros(),
            batch.attempts[0].created_at.timestamp_micros()
        );
        assert!(row.get::<Option<Uuid>, _>("clock_sample_id").is_none());
        assert!(row.get::<Option<String>, _>("clock_verdict").is_none());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM spot_intent_clock")
            .fetch_one(&pool)
            .await
            .unwrap(),
        4
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_planned_statements_are_the_ones_sent_on_spot(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, tenant);
    backend.check_ready().await.unwrap();
    for kind in [IntentKind::Entry, IntentKind::Exit, IntentKind::Cancel] {
        let mut batch = if kind == IntentKind::Entry {
            entry_batch(&pool, tenant, "entry").await
        } else {
            balance_batch(kind.as_str(), kind)
        };
        for outcome in [false, true] {
            if outcome {
                batch.outcome = Some(OutcomeRow {
                    venue_outcome: Some(VenueOutcome::Success),
                    ..OutcomeRow::default()
                });
            }
            let before = proxy.stats().statements;
            backend.commit(&batch).await.unwrap();
            assert_eq!(proxy.stats().statements - before, u64::from(batch.statements()));
            assert_eq!(
                batch.statements(),
                if kind == IntentKind::Entry {
                    if outcome {
                        9
                    } else {
                        8
                    }
                } else if outcome {
                    8
                } else {
                    7
                }
            );
        }
    }
}

fn db_attempts_total() -> u64 {
    prometheus::gather()
        .iter()
        .find(|family| family.get_name() == "robsond_reconciliation_db_attempts_total")
        .map_or(0, |family| family.get_metric()[0].get_counter().get_value() as u64)
}

/// Every statement a balance commit sends is an observed operation attempt,
/// as on the unflagged path: the proxy's count is the independent oracle.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_balance_commit_counts_every_statement_as_operation_io(pool: PgPool) {
    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::layer::SubscriberExt as _;

    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, tenant);
    backend.check_ready().await.unwrap();
    robsond::operation_observation::initialize_metrics();
    // Retain a second registered dispatcher for the whole observation, as the
    // crate's own probe does: with one registered dispatcher, tracing-core
    // 0.1.36 asks the registering thread's default, so a concurrent
    // unobserved test could cache NEVER for a shared balance SQL callsite.
    // The guard is never installed as a default and enables no callsite.
    let _registration_guard = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let observed_dispatch = tracing::Dispatch::new(
        tracing_subscriber::registry().with(robsond::operation_observation::layer()),
    );
    for (id, kind) in [
        ("observed-cancel", IntentKind::Cancel),
        ("observed-exit", IntentKind::Exit),
    ] {
        let batch = balance_batch(id, kind);
        let (sent, observed) = async {
            robson_exec::observation::reconciliation(async {
                let statements = proxy.stats().statements;
                let attempts = db_attempts_total();
                backend.commit(&batch).await.unwrap();
                (proxy.stats().statements - statements, db_attempts_total() - attempts)
            })
            .await
        }
        .with_subscriber(observed_dispatch.clone())
        .await;
        assert_eq!(sent, u64::from(batch.statements()));
        assert_eq!(observed, sent, "{id}: every sent statement is one observed attempt");
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_usdm_commit_issues_exactly_its_pre_8e_statements(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, VenueId::BINANCE_USDM).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = journal_on(&proxied, tenant, false, false, VenueId::BINANCE_USDM);
    journal.check_ready().await.unwrap();
    for cancel in [false, true] {
        let op = journal.begin_operation();
        let before = proxy.stats().statements;
        op.scope(async {
            if cancel {
                journal
                    .journaled_cancel(None, "ins-1", async {
                        Ok(StopCancelOutcome::Cancelled {
                            filled_before_cancel: Decimal::ZERO,
                            fills: None,
                        })
                    })
                    .await;
            } else {
                let intent = journal.open_intent(spec("entry", IntentKind::Entry));
                journal.before_send(&intent, WriteClass::Strict).await.unwrap();
                journal.finish_failed(&intent).await;
            }
        })
        .await;
        assert_eq!(proxy.stats().statements - before, 9);
    }
    assert_eq!(journal.statements_total(), 18);
    assert_eq!(version(&pool, tenant, VenueId::BINANCE_USDM).await, 0);
    for table in [
        "spot_intent_clock",
        "balance_leg_admissions",
        "balance_legs",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_ack_on_a_spot_commit_converges_on_replay(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let backend = PgJournalBackend::new(&proxied, tenant);
    let batch = entry_batch(&pool, tenant, "entry").await;
    proxy.set_mode(ProxyMode::DropCommitAckOf("balance_leg_admit"));
    assert!(matches!(backend.commit(&batch).await, Err(CommitFailure::Uncertain(_))));
    assert_eq!(version(&pool, tenant, SPOT).await, 1);
    backend.commit(&batch).await.unwrap();
    assert_eq!(version(&pool, tenant, SPOT).await, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM balance_leg_admissions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM spot_intent_clock")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_spent_flagged_cancel_rolls_back_on_the_native_index_error(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    sqlx::raw_sql("CREATE FUNCTION refuse_cancel_attempt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE unique_violation USING CONSTRAINT='idx_submit_attempts_one_per_spot_client_id'; END $$; CREATE TRIGGER refuse_cancel_attempt BEFORE INSERT ON submit_attempts FOR EACH ROW EXECUTE FUNCTION refuse_cancel_attempt();").execute(&pool).await.unwrap();
    // The generic cancel writers now refuse Spot before any write; the flagged
    // commit of the cancel they would have journaled keeps the index oracle.
    let backend = PgJournalBackend::new(&pool, tenant);
    let mut batch = balance_batch(&format!("cxl-{}", Uuid::now_v7().simple()), IntentKind::Cancel);
    batch.intent.cancels_order_id = Some("77".to_string());
    batch.intent.params = serde_json::Value::Null;
    let tables = [
        "order_intents",
        "submit_attempts",
        "spot_intent_clock",
        "recovery_incidents",
    ];
    assert_eq!(backend.commit(&batch).await, Err(CommitFailure::IdSpent));
    assert_eq!(row_counts(&pool, &tables).await, [0, 0, 0, 0]);
    assert_eq!(version(&pool, tenant, SPOT).await, 0);

    // Positive control: without the fault the same batch lands whole.
    sqlx::query("DROP TRIGGER refuse_cancel_attempt ON submit_attempts")
        .execute(&pool)
        .await
        .unwrap();
    backend.commit(&batch).await.unwrap();
    assert_eq!(row_counts(&pool, &tables).await, [1, 1, 1, 0]);
    assert_eq!(version(&pool, tenant, SPOT).await, 1);
}

pub(super) async fn row_counts(pool: &PgPool, tables: &[&str]) -> Vec<i64> {
    let mut counts = Vec::with_capacity(tables.len());
    for table in tables {
        counts.push(
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    counts
}

#[path = "admission.rs"]
mod admission;
#[path = "budget.rs"]
mod budget;
#[path = "readiness.rs"]
mod readiness;
#[path = "store.rs"]
mod store;
#[path = "store_retirement.rs"]
mod store_retirement;
