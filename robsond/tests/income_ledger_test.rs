//! Integration tests for the income ledger (ADR-0045 §1/§2).
//!
//! Exercises the ingest/match/alarm/transfer-explanation free functions
//! against a real Postgres. Mirrors the ADR's failure-modes table — each
//! test name maps to a specific row.
//!
//! Run with: `DATABASE_URL=postgresql://... cargo test -p robsond --features
//! postgres --test income_ledger_test -- --ignored`

#![cfg(feature = "postgres")]

use chrono::{Duration as ChronoDuration, Utc};
use robson_domain::Symbol;
use robson_exec::ports::{IncomeRecord, IncomeType};
use robsond::income_ledger::{
    checkpoint, count_confirmed_anomalies, ingest_items, match_pending_items,
    transfer_explains_delta,
};
use rust_decimal_macros::dec;
use uuid::Uuid;

fn btcusdt() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

async fn seed_position(
    pool: &sqlx::PgPool,
    symbol: &str,
    entry_filled_at: chrono::DateTime<Utc>,
    closed_at: Option<chrono::DateTime<Utc>>,
) -> Uuid {
    let position_id = Uuid::now_v7();
    let tenant_id = Uuid::now_v7();
    let account_id = Uuid::now_v7();
    let last_event_id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO positions_current (
            position_id, tenant_id, account_id, symbol, side,
            entry_price, entry_quantity, entry_filled_at,
            technical_stop_price, technical_stop_distance, current_quantity,
            trailing_stop_price, state, last_event_id, last_seq,
            created_at, updated_at, closed_at
        ) VALUES (
            $1, $2, $3, $4, 'long',
            50000, 0.1, $5,
            49000, 1000, 0.1,
            49000, 'active', $6, 1,
            NOW(), NOW(), $7
        )
        "#,
    )
    .bind(position_id)
    .bind(tenant_id)
    .bind(account_id)
    .bind(symbol)
    .bind(entry_filled_at)
    .bind(last_event_id)
    .bind(closed_at)
    .execute(pool)
    .await
    .unwrap();

    last_event_id
}

fn income_item(
    id: &str,
    symbol: Option<Symbol>,
    income_type: IncomeType,
    amount: rust_decimal::Decimal,
    income_time: chrono::DateTime<Utc>,
) -> IncomeRecord {
    IncomeRecord {
        exchange_income_id: id.to_string(),
        symbol,
        income_type,
        amount,
        asset: "USDT".to_string(),
        exchange_trade_id: None,
        income_time,
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_ingest_is_idempotent(pool: sqlx::PgPool) {
    let now = Utc::now();
    let items = vec![income_item(
        "tran-1",
        Some(btcusdt()),
        IncomeType::Commission,
        dec!(-0.22),
        now,
    )];

    let first = ingest_items(&pool, &items).await.unwrap();
    assert_eq!(first, 1);

    // Re-ingesting the exact same exchange_income_id must be a no-op —
    // this is what makes a re-poll of an overlapping window safe.
    let second = ingest_items(&pool, &items).await.unwrap();
    assert_eq!(second, 0);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM income_ledger")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_checkpoint_resumes_from_max_income_time(pool: sqlx::PgPool) {
    let now = Utc::now();
    let older = now - ChronoDuration::hours(2);

    // Empty table: bounded 24h lookback, not full history.
    let empty_checkpoint = checkpoint(&pool).await.unwrap();
    assert!(empty_checkpoint <= now - ChronoDuration::hours(23));

    ingest_items(&pool, &[income_item(
        "tran-2",
        Some(btcusdt()),
        IncomeType::FundingFee,
        dec!(-0.03),
        older,
    )])
    .await
    .unwrap();

    let resumed = checkpoint(&pool).await.unwrap();
    assert_eq!(resumed.timestamp_millis(), older.timestamp_millis());
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_funding_fee_always_recognized_no_governed_link_needed(pool: sqlx::PgPool) {
    // FUNDING_FEE never links to a governed fill by construction (module
    // docs) — it must match trivially, with no positions_current row at all.
    let now = Utc::now();
    ingest_items(&pool, &[income_item(
        "tran-3",
        Some(btcusdt()),
        IncomeType::FundingFee,
        dec!(-0.025),
        now,
    )])
    .await
    .unwrap();

    let matched = match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();
    assert_eq!(matched, 1);

    let anomalies = count_confirmed_anomalies(&pool, ChronoDuration::minutes(0)).await.unwrap();
    assert_eq!(anomalies, 0, "funding fee must never alarm");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_commission_matches_unique_position_in_window(pool: sqlx::PgPool) {
    let now = Utc::now();
    let last_event_id = seed_position(&pool, "BTCUSDT", now, None).await;

    ingest_items(&pool, &[income_item(
        "tran-4",
        Some(btcusdt()),
        IncomeType::Commission,
        dec!(-0.22),
        now + ChronoDuration::seconds(5),
    )])
    .await
    .unwrap();

    let matched = match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();
    assert_eq!(matched, 1);

    let (matched_event_id,): (Option<Uuid>,) = sqlx::query_as(
        "SELECT matched_event_id FROM income_ledger WHERE exchange_income_id = 'tran-4'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(matched_event_id, Some(last_event_id));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_ambiguous_match_stays_unmatched_and_eventually_alarms(pool: sqlx::PgPool) {
    // ADR-0045 failure mode: "income item matches nothing governed" — here
    // via ambiguity (two candidate positions in the same window). Mirrors
    // gather_user_trade_evidence's "don't guess" discipline.
    let now = Utc::now();
    seed_position(&pool, "BTCUSDT", now, None).await;
    seed_position(&pool, "BTCUSDT", now + ChronoDuration::seconds(10), None).await;

    let old_time = now - ChronoDuration::minutes(10);
    ingest_items(&pool, &[income_item(
        "tran-5",
        Some(btcusdt()),
        IncomeType::RealizedPnl,
        dec!(5.31),
        old_time,
    )])
    .await
    .unwrap();

    let matched = match_pending_items(&pool, ChronoDuration::minutes(15)).await.unwrap();
    assert_eq!(matched, 0, "ambiguous candidates must not be guessed");

    // Past the grace period -> confirmed anomaly, not a lagging fill.
    let anomalies = count_confirmed_anomalies(&pool, ChronoDuration::minutes(5)).await.unwrap();
    assert_eq!(anomalies, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_unmatched_item_within_grace_is_not_yet_an_anomaly(pool: sqlx::PgPool) {
    // ADR-0045 failure mode: "governed fill lagging its income record" — an
    // unmatched item younger than the grace period must not alarm yet.
    let now = Utc::now();
    ingest_items(&pool, &[income_item(
        "tran-6",
        Some(btcusdt()),
        IncomeType::RealizedPnl,
        dec!(5.31),
        now,
    )])
    .await
    .unwrap();

    match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();

    let anomalies = count_confirmed_anomalies(&pool, ChronoDuration::minutes(5)).await.unwrap();
    assert_eq!(anomalies, 0, "item is younger than the grace period");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_transfer_explains_delta_when_fully_matched(pool: sqlx::PgPool) {
    let since = Utc::now() - ChronoDuration::hours(1);
    let now = Utc::now();

    ingest_items(&pool, &[income_item(
        "tran-7",
        None,
        IncomeType::Transfer,
        dec!(100),
        now,
    )])
    .await
    .unwrap();
    match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();

    let explained = transfer_explains_delta(&pool, since, dec!(100), dec!(0.01)).await.unwrap();
    assert_eq!(explained, Some(dec!(100)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_residual_not_explained_when_other_items_unmatched(pool: sqlx::PgPool) {
    // ADR-0045 failure mode: "Residual != 0 with all items matched" is the
    // rare case; the far more common guard is this one — a TRANSFER alone
    // does NOT authorize a write while something else in the window is
    // still an open question.
    let since = Utc::now() - ChronoDuration::hours(1);
    let now = Utc::now();

    ingest_items(&pool, &[
        income_item("tran-8", None, IncomeType::Transfer, dec!(100), now),
        income_item("tran-9", Some(btcusdt()), IncomeType::RealizedPnl, dec!(5), now),
    ])
    .await
    .unwrap();
    // Only the TRANSFER auto-matches; REALIZED_PNL has no governed position
    // to match against here, so it stays unmatched.
    match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();

    let explained = transfer_explains_delta(&pool, since, dec!(100), dec!(0.01)).await.unwrap();
    assert_eq!(explained, None, "an unmatched non-transfer item must block the write");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL to be set"]
async fn test_residual_mismatch_not_explained(pool: sqlx::PgPool) {
    // ADR-0045 failure mode: "Residual != 0 with all items matched" —
    // invariant breach. The matched TRANSFER sum does not cover the
    // observed delta even though nothing is unmatched.
    let since = Utc::now() - ChronoDuration::hours(1);
    let now = Utc::now();

    ingest_items(&pool, &[income_item(
        "tran-10",
        None,
        IncomeType::Transfer,
        dec!(40),
        now,
    )])
    .await
    .unwrap();
    match_pending_items(&pool, ChronoDuration::seconds(120)).await.unwrap();

    let explained = transfer_explains_delta(&pool, since, dec!(100), dec!(0.01)).await.unwrap();
    assert_eq!(explained, None, "matched transfer sum (40) does not cover the delta (100)");
}
