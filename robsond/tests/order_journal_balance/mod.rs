//! Balance-only PostgreSQL journal contracts (ADR-0056 step 8e-3).
use chrono::{DateTime, NaiveDate};
use robson_domain::MonthlyBudgetModel;
use robson_exec::{
    cancel::CancelClass,
    order_journal::{AdmissionRow, BalanceBatch, OrphanCancel},
    venue_ledger::LedgerError,
};
use robsond::venue_ledger_store::PgVenueLedger;
use rust_decimal_macros::dec;
use sqlx::Row;

use super::*;

const SPOT: &str = VenueId::BINANCE_SPOT;
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

fn balance_batch(id: &str, kind: IntentKind) -> JournalBatch {
    let mut batch = spot_batch(id);
    batch.intent.kind = kind;
    batch.balance = Some(BalanceBatch {
        admission: None,
        handed_off: false,
        commit_bound: BOUND,
    });
    batch
}

async fn seed_account(pool: &PgPool, tenant: Uuid, venue: &str) {
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

fn journal_on(
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

async fn version(pool: &PgPool, tenant: Uuid, venue: &str) -> i64 {
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
async fn a_spent_cancel_id_is_not_sent_on_a_balance_journal(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    sqlx::raw_sql("CREATE FUNCTION refuse_cancel_attempt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE unique_violation USING CONSTRAINT='idx_submit_attempts_one_per_spot_client_id'; END $$; CREATE TRIGGER refuse_cancel_attempt BEFORE INSERT ON submit_attempts FOR EACH ROW EXECUTE FUNCTION refuse_cancel_attempt();").execute(&pool).await.unwrap();
    let journal = journal_on(&pool, tenant, true, true, SPOT);
    let polled = std::sync::atomic::AtomicUsize::new(0);
    for orphan in [false, true] {
        let op = journal.begin_operation();
        op.scope(async {
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
        })
        .await;
    }
    assert_eq!(polled.load(Ordering::SeqCst), 0);
    assert_eq!(version(&pool, tenant, SPOT).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_intents")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recovery_incidents")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
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
