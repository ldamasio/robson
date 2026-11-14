//! Trade persistence and accounting invariants against isolated sqlx databases.
#![cfg(feature = "postgres")]
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use robson_exec::{
    venue::OrderPhase,
    venue_ledger::{LedgerError, OrderOwner},
};
use robson_testkit::db_proxy::ProxyMode;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::{PgPool, Row};
#[path = "venue_ledger_trades/support.rs"]
mod support;
use support::*;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn trades_land_once_and_complete_the_order(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(2), OrderPhase::Terminal).await;
    let ledger = f.ledger(&pool);
    let v = f.version(&pool).await;
    assert!(ledger.record_trades(&key, &page(1, dec!(2)), at()).await.unwrap().changed);
    assert_eq!(f.version(&pool).await, v + 1);
    assert_eq!(f.consumed(&pool).await, dec!(2));
    let row =
        sqlx::query("SELECT fills_qty,executed_cum,fills_complete,trades_cursor FROM venue_orders")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.get::<Decimal, _>("fills_qty"), dec!(2));
    assert_eq!(row.get::<Decimal, _>("executed_cum"), dec!(2));
    assert!(row.get::<bool, _>("fills_complete"));
    assert_eq!(row.get::<i64, _>("trades_cursor"), 2);
    let before = snapshot(&pool).await;
    assert!(!ledger.record_trades(&key, &page(1, dec!(2)), at()).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_trade_booked_under_another_order_is_a_conflict(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let a = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let b = f.seed_order(&pool, 2, Decimal::ZERO, OrderPhase::Live).await;
    let ledger = f.ledger(&pool);
    ledger.record_trades(&a, &page(1, dec!(1)), at()).await.unwrap();
    let before = snapshot(&pool).await;
    assert_eq!(
        ledger.record_trades(&b, &page(1, dec!(1)), at()).await,
        Err(LedgerError::TradeConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

// The inert ledger has no venue-account binding yet. Two tenants with the
// same scope exercise the global fill key while holding different account
// locks; no production caller or cutover is enabled by this fixture.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_concurrent_global_trade_collision_rolls_back_the_loser(pool: PgPool) {
    let a = Fixture::new(&pool).await;
    let b = Fixture::for_tenant_scope(&pool, uuid::Uuid::now_v7(), Some(a.scope.clone())).await;
    let ka = a.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let kb = b.seed_order(&pool, 2, Decimal::ZERO, OrderPhase::Live).await;
    let versions = [a.version(&pool).await, b.version(&pool).await];
    sqlx::raw_sql("CREATE FUNCTION hold_fill_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(875601); RETURN NEW; END $$; CREATE TRIGGER hold_fill_insert BEFORE INSERT ON order_fills FOR EACH ROW EXECUTE FUNCTION hold_fill_insert()")
        .execute(&pool).await.unwrap();
    let mut barrier = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(875601)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let la = a.ledger(&pool);
    let lb = b.ledger(&pool);
    let first = tokio::spawn(async move { la.record_trades(&ka, &page(1, dec!(1)), at()).await });
    let second = tokio::spawn(async move { lb.record_trades(&kb, &page(1, dec!(1)), at()).await });
    tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let waiting:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND query LIKE 'INSERT INTO order_fills%' AND wait_event='advisory'")
                .fetch_one(&pool).await.unwrap();
            if waiting == 2 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("both writers must pass their read/recheck before INSERT");
    sqlx::query("SELECT pg_advisory_unlock(875601)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let results = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(results.iter().filter(|r| r.as_ref().is_ok_and(|r| r.changed)).count(), 1);
    assert_eq!(
        results.iter().filter(|r| matches!(r, Err(LedgerError::TradeConflict))).count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_fills")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    for (index, f) in [&a, &b].iter().enumerate() {
        let landed = i64::from(results[index].is_ok());
        assert_eq!(f.version(&pool).await, versions[index] + landed);
        assert_eq!(f.consumed(&pool).await, Decimal::from(landed));
        let (fills, executed, cursor): (Decimal, Decimal, i64) = sqlx::query_as(
            "SELECT fills_qty,executed_cum,trades_cursor FROM venue_orders WHERE tenant_id=$1",
        )
        .bind(f.tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((fills, executed), (Decimal::from(landed), Decimal::from(landed)));
        assert_eq!(cursor, if landed == 1 { 2 } else { 0 });
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_conflicting_duplicate_trade_lands_nothing(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let ledger = f.ledger(&pool);
    ledger.record_trades(&key, &page(1, dec!(1)), at()).await.unwrap();
    let before = snapshot(&pool).await;
    let mut changed = page(1, dec!(1));
    changed.trades[0].commission = dec!(0.02);
    changed.trades.push(fill(2, dec!(1)));
    assert_eq!(
        ledger.record_trades(&key, &changed, at()).await,
        Err(LedgerError::TradeConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn trades_beyond_a_terminal_record_are_a_conflict(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Terminal).await;
    let before = snapshot(&pool).await;
    assert_eq!(
        f.ledger(&pool).record_trades(&key, &page(1, dec!(2)), at()).await,
        Err(LedgerError::TradeConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn trades_on_a_live_order_raise_executed_and_consumed(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Live).await;
    f.ledger(&pool).record_trades(&key, &page(1, dec!(2)), at()).await.unwrap();
    assert_eq!(f.consumed(&pool).await, dec!(2));
    let (fills, executed): (Decimal, Decimal) =
        sqlx::query_as("SELECT fills_qty,executed_cum FROM venue_orders")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((fills, executed), (dec!(2), dec!(2)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_ack_on_record_trades_converges(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let v = f.version(&pool).await;
    proxy.set_mode(ProxyMode::DropCommitAckOf("order_fills"));
    assert!(matches!(
        ledger.record_trades(&key, &page(1, dec!(2)), at()).await,
        Err(LedgerError::Uncertain(_))
    ));
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
    let landed = snapshot(&pool).await;
    assert_eq!(f.version(&pool).await, v + 1);
    assert_eq!(f.consumed(&pool).await, dec!(2));
    assert!(
        !ledger
            .record_trades(&key, &page(1, dec!(2)), at() + chrono::Duration::seconds(1))
            .await
            .unwrap()
            .changed
    );
    assert_eq!(snapshot(&pool).await, landed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_failed_trades_commit_rolls_back_the_cursor(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    sqlx::raw_sql("CREATE FUNCTION refuse_fill_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture refuses commit' USING ERRCODE='23514'; END $$; CREATE CONSTRAINT TRIGGER refuse_fill_commit AFTER INSERT ON order_fills DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_fill_commit()").execute(&pool).await.unwrap();
    let before = snapshot(&pool).await;
    assert!(matches!(
        f.ledger(&pool).record_trades(&key, &page(1, dec!(2)), at()).await,
        Err(LedgerError::Backend(_))
    ));
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_cancelled_record_trades_between_its_aggregate_statements_lands_nothing(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    sqlx::raw_sql("CREATE FUNCTION sleep_leg_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(20); RETURN NEW; END $$; CREATE TRIGGER sleep_leg_write BEFORE UPDATE ON balance_legs FOR EACH ROW EXECUTE FUNCTION sleep_leg_write()").execute(&pool).await.unwrap();
    let before = snapshot(&pool).await;
    let (_proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = Arc::new(f.ledger(&proxied));
    let task_ledger = ledger.clone();
    let task_key = key.clone();
    let task =
        tokio::spawn(
            async move { task_ledger.record_trades(&task_key, &page(1, dec!(2)), at()).await },
        );
    wait_for_query(&pool, "UPDATE balance_legs SET consumed_cum").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND query LIKE '%UPDATE balance_legs SET consumed_cum%' AND state='active')").fetch_one(&pool).await.unwrap();
            if !active {break;} tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(snapshot(&pool).await, before);
    sqlx::query("DROP TRIGGER sleep_leg_write ON balance_legs")
        .execute(&pool)
        .await
        .unwrap();
    assert!(ledger.record_trades(&key, &page(1, dec!(2)), at()).await.unwrap().changed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_duplicate_page_without_cursor_advance_writes_nothing(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    ledger.record_trades(&key, &page(1, dec!(1)), at()).await.unwrap();
    let before = snapshot(&pool).await;
    let wire = proxy.stats().statements;
    let count = ledger.statements_total();
    assert!(
        !ledger
            .record_trades(&key, &page(1, dec!(1)), at() + chrono::Duration::days(1))
            .await
            .unwrap()
            .changed
    );
    assert_eq!(proxy.stats().statements - wire, 1);
    assert_eq!(ledger.statements_total() - count, 1);
    assert_eq!(snapshot(&pool).await, before);
    let recorded: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT recorded_at FROM order_fills")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recorded, at());
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn touch_checked_changes_rotation_only(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Terminal).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let v = f.version(&pool).await;
    let before: String =
        sqlx::query_scalar("SELECT (to_jsonb(o)-'last_checked_at')::text FROM venue_orders o")
            .fetch_one(&pool)
            .await
            .unwrap();
    let count = proxy.stats().statements;
    let later = at() + chrono::Duration::seconds(5);
    ledger.touch_checked(&key, later).await.unwrap();
    assert_eq!(proxy.stats().statements - count, 1);
    assert_eq!(ledger.statements_total(), 1);
    ledger.touch_checked(&key, at()).await.unwrap();
    let (after, checked): (String, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT (to_jsonb(o)-'last_checked_at')::text,last_checked_at FROM venue_orders o",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(checked, later);
    assert_eq!(f.version(&pool).await, v);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn record_trades_issues_its_reviewed_statements(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    for n in [1, 1000] {
        let key = f.seed_order(&pool, n, Decimal::ZERO, OrderPhase::Live).await;
        let (proxy, proxied) = through_proxy(&pool, 1).await;
        let ledger = f.ledger(&proxied);
        let page = robson_exec::venue::TradePage {
            trades: (1..=n).map(|id| fill(10000 * n + id, dec!(0.001))).collect(),
            next_from_id: None,
        };
        let before = proxy.stats().statements;
        assert!(ledger.record_trades(&key, &page, at()).await.unwrap().changed);
        assert_eq!(proxy.stats().statements - before, 10);
        assert_eq!(ledger.statements_total(), 10);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn trade_values_round_trip_without_numeric_precision_loss(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let mut p = page(1, dec!(0.0000000000000000000000000001));
    p.trades[0].price = dec!(79228162514264337593543950335);
    p.trades[0].quote_qty = dec!(7.9228162514264337593543950335);
    p.trades[0].commission = dec!(0.0000000000000000000000000001);
    let ledger = f.ledger(&pool);
    ledger.record_trades(&key, &p, at()).await.unwrap();
    let (price, qty, quote, fee): (Decimal, Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT price,qty,quote_qty,commission FROM order_fills")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (price, qty, quote, fee),
        (
            p.trades[0].price,
            p.trades[0].qty,
            p.trades[0].quote_qty,
            p.trades[0].commission
        )
    );
    assert!(!ledger.record_trades(&key, &p, at()).await.unwrap().changed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn trade_pre_read_has_a_bounded_deadline_and_retires_connection(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, Decimal::ZERO, OrderPhase::Live).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let before = snapshot(&pool).await;
    proxy.set_mode(ProxyMode::Blackhole);
    let started = Instant::now();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            ledger.record_trades(&key, &page(1, dec!(1)), at())
        )
        .await
        .expect("trade pre-read exceeded its outer deadline"),
        Err(LedgerError::Backend(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(3));
    proxy.set_mode(ProxyMode::Pass);
    assert!(ledger.record_trades(&key, &page(1, dec!(1)), at()).await.unwrap().changed);
    assert!(proxy.stats().connections >= 2);
    assert_ne!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn adoption_after_trade_pages_counts_the_whole_execution(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    let record = f.record(1, Decimal::ZERO, OrderPhase::Live);
    ledger
        .record_order(&record, &f.link(1, OrderOwner::Discovered), seen())
        .await
        .unwrap();
    ledger.record_trades(&f.key(1), &page(1, dec!(2)), at()).await.unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
    ledger
        .record_order(&f.record(1, dec!(2), OrderPhase::Live), &f.owned(1), seen())
        .await
        .unwrap();
    assert_eq!(f.consumed(&pool).await, dec!(2));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_autocommit_answer_on_touch_converges(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Terminal).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let v = f.version(&pool).await;
    let later = at() + chrono::Duration::seconds(10);
    proxy.set_mode(ProxyMode::DropAutocommitAckOf("UPDATE venue_orders SET last_checked_at"));
    assert!(matches!(
        ledger.touch_checked(&key, later).await,
        Err(LedgerError::Uncertain(_))
    ));
    assert_eq!(proxy.autocommit_acks_dropped(), 1);
    let landed = snapshot(&pool).await;
    assert_eq!(
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT last_checked_at FROM venue_orders"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        later
    );
    ledger.touch_checked(&key, later).await.unwrap();
    assert_eq!(snapshot(&pool).await, landed);
    assert_eq!(f.version(&pool).await, v);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_touch_timeout_is_uncertain_and_retires_its_connection(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Terminal).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let before = snapshot(&pool).await;
    proxy.set_mode(ProxyMode::Blackhole);
    let started = Instant::now();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            ledger.touch_checked(&key, at() + chrono::Duration::seconds(1))
        )
        .await
        .expect("touch exceeded its outer deadline"),
        Err(LedgerError::Uncertain(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(3));
    proxy.set_mode(ProxyMode::Pass);
    assert_eq!(snapshot(&pool).await, before);
    ledger.touch_checked(&key, at() + chrono::Duration::seconds(1)).await.unwrap();
    assert!(proxy.stats().connections >= 2);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_cancelled_touch_retires_its_connection(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let key = f.seed_order(&pool, 1, dec!(1), OrderPhase::Terminal).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = Arc::new(f.ledger(&proxied));
    let task_ledger = ledger.clone();
    let task_key = key.clone();
    proxy.set_mode(ProxyMode::Blackhole);
    let task = tokio::spawn(async move { task_ledger.touch_checked(&task_key, at()).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while ledger.statements_total() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    proxy.set_mode(ProxyMode::Pass);
    ledger.touch_checked(&key, at()).await.unwrap();
    assert!(proxy.stats().connections >= 2);
}

#[test]
fn trade_ledger_has_no_production_callers() {
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                let production = source.split("#[cfg(test)]").next().unwrap();
                for method in [".record_trades(", ".touch_checked("] {
                    assert!(
                        !production.contains(method),
                        "trade ledger activated in {}",
                        path.display()
                    );
                }
            }
        }
    }
    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

#[path = "venue_ledger_trades/generated.rs"]
mod generated;
