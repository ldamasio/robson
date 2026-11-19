//! Inert ADR-0056 step 8e-4 writer acceptance. Every database belongs to
//! sqlx::test.
#![cfg(feature = "postgres")]
#[path = "venue_ledger_orders/fixture.rs"]
mod fixture;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use fixture::*;
use robson_domain::OrderSide;
use robson_exec::{
    venue::{CancelAnswer, OrderObservation, OrderPhase},
    venue_ledger::{LedgerError, LegOrderKind, OrderAnomaly, OrderOwner},
    ExecError,
};
use robson_testkit::db_proxy::ProxyMode;
use robsond::venue_ledger_store::PgVenueLedger;
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn spot_phase_evidence_preserves_the_native_active_order_predicate(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let order_link = f.link(LegOrderKind::Sell);
    let mut observed = record();
    observed.status = "PENDING_CANCEL".into();
    observed.phase = OrderPhase::Terminal;
    let before = snapshot(&pool).await;
    let version = f.version(&pool).await;
    let statements = f.ledger.statements_total();
    let result = f.ledger.record_order(&observed, &order_link, seen(1)).await;
    assert_eq!(
        snapshot(&pool).await,
        before,
        "contradictory Spot phase changed durable evidence"
    );
    assert!(
        matches!(
            result,
            Err(LedgerError::InvalidEvidence(ref reason))
                if reason == "incoming Spot phase disagrees with status"
        ),
        "{result:?}"
    );
    assert_eq!(f.ledger.statements_total() - statements, 1);

    const EVIDENCE: &str = "SELECT status,phase,NOT terminal FROM venue_orders
        WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4 AND exchange_order_id=$5";
    let read_evidence = || {
        sqlx::query_as::<_, (String, String, bool)>(EVIDENCE)
            .bind(f.tenant)
            .bind(VENUE)
            .bind(&order_link.account_scope)
            .bind(&observed.symbol)
            .bind(&observed.exchange_order_id)
            .fetch_one(&pool)
    };
    observed.phase = OrderPhase::Unknown;
    assert!(f.ledger.record_order(&observed, &order_link, seen(2)).await.unwrap().changed);
    assert_eq!(
        read_evidence().await.unwrap(),
        ("PENDING_CANCEL".to_owned(), "unknown".to_owned(), true)
    );
    assert_eq!(f.version(&pool).await, version + 1);

    observed.phase = OrderPhase::Terminal;
    observed.status = "EXPIRED_IN_MATCH".into();
    assert!(f.ledger.record_order(&observed, &order_link, seen(3)).await.unwrap().changed);
    assert_eq!(
        read_evidence().await.unwrap(),
        ("EXPIRED_IN_MATCH".to_owned(), "terminal".to_owned(), false)
    );
    assert_eq!(f.version(&pool).await, version + 2);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn spot_writer_refuses_to_reuse_an_existing_contradictory_phase(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let order_link = f.link(LegOrderKind::Sell);
    let mut observed = record();
    observed.status = "PENDING_CANCEL".into();
    observed.phase = OrderPhase::Unknown;
    f.ledger.record_order(&observed, &order_link, seen(1)).await.unwrap();
    // Model pre-existing contradictory evidence that the native schema can
    // represent.
    let affected = sqlx::query("UPDATE venue_orders SET phase='terminal', terminal_seen_at=NOW()
        WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4 AND exchange_order_id=$5")
        .bind(f.tenant)
        .bind(VENUE)
        .bind(&order_link.account_scope)
        .bind(&observed.symbol)
        .bind(&observed.exchange_order_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(affected.rows_affected(), 1);
    let before = snapshot(&pool).await;
    let statements = f.ledger.statements_total();
    let result = f.ledger.record_order(&observed, &order_link, seen(2)).await;
    assert_eq!(snapshot(&pool).await, before);
    assert!(
        matches!(
            result,
            Err(LedgerError::InvalidEvidence(ref reason))
                if reason == "stored Spot phase disagrees with status"
        ),
        "{result:?}"
    );
    assert_eq!(f.ledger.statements_total() - statements, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn order_and_contribution_are_atomic_and_replay_is_one_read(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let mut r = record();
    let version = f.version(&pool).await;
    let n = f.ledger.statements_total();
    assert!(f.ledger.record_order(&r, &link, seen(1)).await.unwrap().changed);
    assert_eq!(f.ledger.statements_total() - n, 8);
    assert_eq!(f.version(&pool).await, version + 1);
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
    let before = snapshot(&pool).await;
    let n = f.ledger.statements_total();
    assert!(!f.ledger.record_order(&r, &link, seen(99)).await.unwrap().changed);
    // BEGIN, the server bounds, one read and ROLLBACK.
    assert_eq!(f.ledger.statements_total() - n, 4);
    assert_eq!(snapshot(&pool).await, before);
    r.executed_qty = Decimal::from(3);
    r.cumulative_quote_qty = Some(Decimal::from(300));
    let n = f.ledger.statements_total();
    f.ledger.record_order(&r, &link, seen(2)).await.unwrap();
    assert_eq!(f.ledger.statements_total() - n, 8);
    assert_eq!(f.consumed(&pool).await, Decimal::from(3));
    assert_eq!(f.version(&pool).await, version + 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_fills")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn definite_commit_rejection_rolls_back_every_effect(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    let connections = proxy.stats().connections;
    let pid = backend_pid(&proxied).await;
    proxy.enable_query_trace(32);
    let wire = proxy.stats().statements;
    let before = snapshot(&pool).await;
    execute(&pool,"CREATE FUNCTION reject_order_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture deferred rejection' USING ERRCODE='23514'; END $$; CREATE CONSTRAINT TRIGGER reject_order_commit AFTER INSERT OR UPDATE ON venue_orders DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_order_commit()").await;
    assert!(matches!(
        ledger.record_order(&record(), &f.link(LegOrderKind::Sell), seen(1)).await,
        Err(LedgerError::Backend(_))
    ));
    retired(&pool, pid).await;
    wire_count(&ledger, &proxy, wire, 8);
    assert_eq!(snapshot(&pool).await, before);
    execute(&pool, "DROP TRIGGER reject_order_commit ON venue_orders").await;
    assert!(
        ledger
            .record_order(&record(), &f.link(LegOrderKind::Sell), seen(2))
            .await
            .unwrap()
            .changed
    );
    assert!(
        proxy.stats().connections > connections,
        "a failed ledger transaction must retire even a healthy connection"
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn zero_and_omitted_stop_placeholders_converge_on_real_rows(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let mut id = 10;
    for kind in ["MARKET", "LIMIT", "LIMIT_MAKER"] {
        for terminal in [false, true] {
            for zero_first in [false, true] {
                let mut r = record();
                r.exchange_order_id = id.to_string();
                r.client_order_id = format!("client-{id}");
                id += 1;
                r.order_type = kind.into();
                if terminal {
                    r.phase = OrderPhase::Terminal;
                    r.status = "CANCELED".into();
                }
                r.stop_price = zero_first.then_some(Decimal::ZERO);
                let mut link = unowned();
                link.client_order_id = r.client_order_id.clone();
                let n = f.ledger.statements_total();
                f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
                assert_eq!(f.ledger.statements_total() - n, 7);
                let before = snapshot(&pool).await;
                r.stop_price = (!zero_first).then_some(Decimal::ZERO);
                assert!(!f.ledger.record_order(&r, &link, seen(2)).await.unwrap().changed);
                assert_eq!(snapshot(&pool).await, before);
            }
        }
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM venue_orders WHERE stop_price IS NOT NULL"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn expired_zero_entry_is_settled_and_cancel_lookup_replay_converges(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let mut r = record();
    r.side = OrderSide::Buy;
    r.phase = OrderPhase::Terminal;
    r.status = "EXPIRED".into();
    r.executed_qty = Decimal::ZERO;
    r.cumulative_quote_qty = Some(Decimal::ZERO);
    r.expiry_reason = Some("expired".into());
    let ExecError::ExpiredUnfilled(r) = ExecError::ExpiredUnfilled(Box::new(r)) else {
        unreachable!()
    };
    f.ledger.record_order(&r, &f.link(LegOrderKind::Entry), seen(1)).await.unwrap();
    let row = sqlx::query("SELECT terminal,fills_complete,executed_cum FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(row.get::<bool, _>("terminal"));
    assert!(row.get::<bool, _>("fills_complete"));
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT cohort_month=decided_month FROM balance_legs")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let mut r = record();
    r.exchange_order_id = "8".into();
    r.phase = OrderPhase::Terminal;
    r.status = "CANCELED".into();
    r.client_order_id = "cancel".into();
    let mut link = unowned();
    link.client_order_id = "cancel".into();
    let CancelAnswer::Cancelled(answer) = CancelAnswer::Cancelled(Box::new(r.clone())) else {
        unreachable!()
    };
    f.ledger.record_order(&answer, &link, seen(2)).await.unwrap();
    let before = snapshot(&pool).await;
    let OrderObservation::Found(observed) = OrderObservation::Found(Box::new(r)) else {
        unreachable!()
    };
    assert!(!f.ledger.record_order(&observed, &link, seen(3)).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn stale_terminal_read_can_add_audit_and_adoption_but_never_financial_facts(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let mut terminal = record();
    terminal.phase = OrderPhase::Terminal;
    terminal.status = "CANCELED".into();
    f.ledger.record_order(&terminal, &unowned(), seen(1)).await.unwrap();
    let mut stale = record();
    stale.executed_qty = Decimal::ONE;
    stale.cumulative_quote_qty = Some(Decimal::from(100));
    stale.update_time_ms = Some(777);
    let link = f.link(LegOrderKind::Sell);
    let result = f.ledger.record_order(&stale, &link, seen(2)).await.unwrap();
    assert!(result.changed);
    assert!(result.anomalies.contains(&OrderAnomaly::TerminalReadAsLive));
    let row=sqlx::query("SELECT status,executed_cum,cumulative_quote,terminal_seen_at,venue_update_time_ms FROM venue_orders").fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "CANCELED");
    assert_eq!(row.get::<Decimal, _>("executed_cum"), Decimal::from(2));
    assert_eq!(row.get::<Decimal, _>("cumulative_quote"), Decimal::from(200));
    assert_eq!(row.get::<chrono::DateTime<chrono::Utc>, _>("terminal_seen_at"), seen(1).at);
    assert_eq!(row.get::<i64, _>("venue_update_time_ms"), 777);
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
    let before = snapshot(&pool).await;
    assert!(!f.ledger.record_order(&stale, &link, seen(3)).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn exact_native_decimals_survive_pre_read_locked_recheck_and_aggregate(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let tiny = Decimal::new(1, 28);
    execute(&pool, "UPDATE balance_legs SET consumed_cum=0.0000000000000000000000000001").await;
    let mut r = record();
    r.executed_qty = tiny + tiny;
    r.cumulative_quote_qty = Some(Decimal::MAX);
    let link = f.link(LegOrderKind::Sell);
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    assert_eq!(f.consumed(&pool).await, tiny + tiny + tiny);
    assert_eq!(
        sqlx::query_scalar::<_, Decimal>("SELECT cumulative_quote FROM venue_orders")
            .fetch_one(&pool)
            .await
            .unwrap(),
        Decimal::MAX
    );
    assert!(!f.ledger.record_order(&r, &link, seen(2)).await.unwrap().changed);
    let mut max = record();
    max.exchange_order_id = "8".into();
    max.client_order_id = "max".into();
    max.orig_qty = Decimal::MAX;
    max.executed_qty = Decimal::MAX;
    max.cumulative_quote_qty = Some(Decimal::MAX);
    let mut unowned = unowned();
    unowned.client_order_id = "max".into();
    f.ledger.record_order(&max, &unowned, seen(1)).await.unwrap();
    assert!(!f.ledger.record_order(&max, &unowned, seen(2)).await.unwrap().changed);
    execute(&pool, "UPDATE balance_legs SET consumed_cum=79228162514264337593543950335").await;
    let before = snapshot(&pool).await;
    r.executed_qty = Decimal::ONE;
    assert!(matches!(
        f.ledger.record_order(&r, &link, seen(3)).await,
        Err(LedgerError::Unrepresentable)
    ));
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn concurrent_adoption_counts_once_and_refuses_another_leg(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let r = record();
    f.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap();
    let link = f.link(LegOrderKind::Sell);
    let version = f.version(&pool).await;
    let a = PgVenueLedger::new(&pool, f.tenant);
    let b = PgVenueLedger::new(&pool, f.tenant);
    let (a, b) =
        tokio::join!(a.record_order(&r, &link, seen(2)), b.record_order(&r, &link, seen(2)));
    assert_eq!([a.unwrap().changed, b.unwrap().changed].into_iter().filter(|x| *x).count(), 1);
    assert_eq!(f.version(&pool).await, version + 1);
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
    let before = snapshot(&pool).await;
    let mut wrong = link.clone();
    wrong.owner = OrderOwner::Leg {
        intent_id: uuid::Uuid::now_v7(),
        position_id: uuid::Uuid::now_v7(),
        kind: LegOrderKind::Sell,
    };
    assert_eq!(
        f.ledger.record_order(&r, &wrong, seen(3)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn quote_floor_survives_an_unknown_total_across_row_persistence(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let mut r = record();
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    r.executed_qty = Decimal::from(3);
    r.cumulative_quote_qty = None;
    assert!(f.ledger.record_order(&r, &link, seen(2)).await.unwrap().changed);
    let row = sqlx::query("SELECT cumulative_quote,quote_floor FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<Decimal>, _>("cumulative_quote"), None);
    assert_eq!(row.get::<Option<Decimal>, _>("quote_floor"), Some(Decimal::from(200)));
    let before = snapshot(&pool).await;
    let version = f.version(&pool).await;
    let consumed = f.consumed(&pool).await;
    r.cumulative_quote_qty = Some(Decimal::from(150));
    for terminal in [false, true] {
        if terminal {
            r.phase = OrderPhase::Terminal;
            r.status = "CANCELED".into();
        }
        assert_eq!(
            f.ledger.record_order(&r, &link, seen(3)).await,
            Err(LedgerError::TradeConflict)
        );
        assert_eq!(snapshot(&pool).await, before);
        assert_eq!(f.version(&pool).await, version);
        assert_eq!(f.consumed(&pool).await, consumed);
    }
    r.phase = OrderPhase::Live;
    r.status = "PARTIALLY_FILLED".into();
    r.cumulative_quote_qty = Some(Decimal::from(300));
    assert!(f.ledger.record_order(&r, &link, seen(4)).await.unwrap().changed);
    let row = sqlx::query("SELECT cumulative_quote,quote_floor FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<Decimal>, _>("cumulative_quote"), Some(Decimal::from(300)));
    assert_eq!(row.get::<Option<Decimal>, _>("quote_floor"), Some(Decimal::from(300)));
    // A row written before 0039 has a known exact total and no floor. The
    // first larger observation without an exact total keeps that total.
    sqlx::query("UPDATE venue_orders SET quote_floor=NULL")
        .execute(&pool)
        .await
        .unwrap();
    r.executed_qty = Decimal::from(4);
    r.cumulative_quote_qty = None;
    assert!(f.ledger.record_order(&r, &link, seen(5)).await.unwrap().changed);
    let row = sqlx::query("SELECT cumulative_quote,quote_floor FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<Decimal>, _>("cumulative_quote"), None);
    assert_eq!(row.get::<Option<Decimal>, _>("quote_floor"), Some(Decimal::from(300)));
    // The NOT VALID checks still refuse new writes.
    let before = snapshot(&pool).await;
    for (sql, name) in [
        ("UPDATE venue_orders SET quote_floor=-1", "venue_orders_quote_floor_fit"),
        ("UPDATE venue_orders SET cumulative_quote=200", "venue_orders_quote_floor_bound"),
    ] {
        let error = sqlx::query(sql).execute(&pool).await.unwrap_err();
        let database = error.as_database_error().unwrap();
        assert_eq!(database.code().as_deref(), Some("23514"));
        assert_eq!(database.constraint(), Some(name));
    }
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn adoption_after_trade_evidence_counts_the_entire_stored_execution(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let mut r = record();
    f.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap();
    // Model the already-committed future trade writer without inventing timed fills
    // from FULL order responses. This is explicit fixture evidence with venue time.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1")
        .bind(f.tenant)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO order_fills(venue_id,account_scope,symbol,exchange_order_id,trade_id,tenant_id,price,qty,quote_qty,commission,commission_asset,trade_time,recorded_at) VALUES ('binance:spot','scope','BTCUSDT','7','11',$1,100,3,300,0,'USDT',$2,$2)").bind(f.tenant).bind(seen(2).at).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE venue_orders SET executed_cum=3,cumulative_quote=300,quote_floor=300,fills_qty=3,trades_cursor=12")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    r.executed_qty = Decimal::from(3);
    r.cumulative_quote_qty = Some(Decimal::from(300));
    let link = f.link(LegOrderKind::Sell);
    f.ledger.record_order(&r, &link, seen(3)).await.unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::from(3));
    let row = sqlx::query("SELECT fills_qty,trades_cursor FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Decimal, _>("fills_qty"), Decimal::from(3));
    assert_eq!(row.get::<i64, _>("trades_cursor"), 12);
    let before = snapshot(&pool).await;
    assert!(!f.ledger.record_order(&r, &link, seen(4)).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn conflicts_and_invalid_evidence_leave_no_durable_effect(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let r = record();
    let link = f.link(LegOrderKind::Sell);
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    let before = snapshot(&pool).await;
    let mut variants = Vec::new();
    let mut x = r.clone();
    x.orig_qty = Decimal::from(11);
    variants.push(x);
    let mut x = r.clone();
    x.side = OrderSide::Buy;
    variants.push(x);
    let mut x = r.clone();
    x.phase = OrderPhase::Terminal;
    x.status = "CANCELED".into();
    x.executed_qty = Decimal::ONE;
    variants.push(x);
    let mut x = r.clone();
    x.executed_qty = Decimal::from(11);
    variants.push(x);
    let mut x = r.clone();
    x.stop_price = Some(Decimal::NEGATIVE_ONE);
    variants.push(x);
    for x in variants {
        assert!(f.ledger.record_order(&x, &link, seen(2)).await.is_err());
        assert_eq!(snapshot(&pool).await, before);
    }
    let mut second_id = r.clone();
    second_id.exchange_order_id = "8".into();
    assert_eq!(
        f.ledger.record_order(&second_id, &link, seen(2)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
    let missing = PgVenueLedger::new(&pool, uuid::Uuid::now_v7());
    let mut x = r.clone();
    x.exchange_order_id = "99".into();
    assert_eq!(
        missing.record_order(&x, &unowned(), seen(2)).await,
        Err(LedgerError::NoAccountingRow)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn adoption_naming_an_unrelated_client_id_leaves_no_durable_effect(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let r = record();
    assert!(f.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap().changed);
    let before = snapshot(&pool).await;
    let mut link = f.link(LegOrderKind::Sell);
    link.client_order_id = "unrelated".into();
    assert_eq!(
        f.ledger.record_order(&r, &link, seen(2)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
    // The matching client lineage still adopts the whole execution.
    let link = f.link(LegOrderKind::Sell);
    assert!(f.ledger.record_order(&r, &link, seen(3)).await.unwrap().changed);
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_contradictory_known_quote_leaves_no_durable_effect(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let r = record();
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    let before = snapshot(&pool).await;
    for (executed, quote) in [(2, 150), (2, 201), (3, 150)] {
        let mut x = r.clone();
        x.executed_qty = Decimal::from(executed);
        x.cumulative_quote_qty = Some(Decimal::from(quote));
        assert_eq!(
            f.ledger.record_order(&x, &link, seen(2)).await,
            Err(LedgerError::TradeConflict)
        );
        assert_eq!(snapshot(&pool).await, before);
    }
    let mut x = r.clone();
    x.executed_qty = Decimal::from(3);
    x.cumulative_quote_qty = Some(Decimal::from(300));
    assert!(f.ledger.record_order(&x, &link, seen(3)).await.unwrap().changed);
    assert_eq!(f.consumed(&pool).await, Decimal::from(3));
    let quote: Option<Decimal> = sqlx::query_scalar("SELECT cumulative_quote FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(quote, Some(Decimal::from(300)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn first_clock_evidence_is_retained_even_when_later_audit_changes(pool: PgPool) {
    use robson_exec::venue_ledger::{ClockEvidence, ClockVerdict};
    let f = Fixture::new(&pool).await;
    // Both clock ids reference clock_samples on the same tenant, venue and
    // epoch. Idle wall/tick brackets make a matching server time valid.
    let epoch = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO process_epochs (epoch_id, tenant_id, binary_version) VALUES ($1, $2, 'clock-fixture')")
        .bind(epoch).bind(f.tenant).execute(&pool).await.unwrap();
    let [live_sample, terminal_sample] = [uuid::Uuid::now_v7(), uuid::Uuid::now_v7()];
    let boot = uuid::Uuid::now_v7();
    for (sample, valid) in [(live_sample, true), (terminal_sample, false)] {
        sqlx::query("INSERT INTO clock_samples (sample_id,tenant_id,venue_id,process_epoch,clock_domain,send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at)
            VALUES ($1,$2,$3,$4,ROW('boottime',$5::uuid,0,0)::budget_clock_domain_v38,1794614400,0,0,0,1794614400,0,0,0,
                CASE WHEN $6 THEN 1794614400000 ELSE 0 END,
                CASE WHEN $6 THEN 0 ELSE -3589228800000000000 END,0,0,
                CASE WHEN $6 THEN 200000000 ELSE 3589228800200000000 END,
                CASE WHEN $6 THEN 'valid' ELSE 'invalid' END,
                CASE WHEN $6 THEN 0 ELSE 1 END,1,now())")
            .bind(sample).bind(f.tenant).bind(VENUE).bind(epoch).bind(boot).bind(valid)
            .execute(&pool).await.unwrap();
    }
    let link = f.link(LegOrderKind::Sell);
    let mut r = record();
    let mut first = seen(1);
    first.clock = Some(ClockEvidence {
        sample_id: live_sample,
        wall_delta_ms: 5,
        mono_delta_ms: 6,
        verdict: ClockVerdict::Valid,
    });
    f.ledger.record_order(&r, &link, first.clone()).await.unwrap();
    r.phase = OrderPhase::Terminal;
    r.status = "CANCELED".into();
    let mut terminal = seen(2);
    terminal.clock = Some(ClockEvidence {
        sample_id: terminal_sample,
        wall_delta_ms: 9,
        mono_delta_ms: 10,
        verdict: ClockVerdict::Invalid,
    });
    f.ledger.record_order(&r, &link, terminal.clone()).await.unwrap();
    let evidence:String=sqlx::query_scalar("SELECT ROW(live_seen_at,live_clock_sample_id,live_clock_wall_delta_ms,live_clock_mono_delta_ms,live_clock_verdict,terminal_seen_at,terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,terminal_clock_verdict)::text FROM venue_orders").fetch_one(&pool).await.unwrap();
    r.update_time_ms = Some(123);
    f.ledger.record_order(&r, &link, seen(3)).await.unwrap();
    let after:String=sqlx::query_scalar("SELECT ROW(live_seen_at,live_clock_sample_id,live_clock_wall_delta_ms,live_clock_mono_delta_ms,live_clock_verdict,terminal_seen_at,terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,terminal_clock_verdict)::text FROM venue_orders").fetch_one(&pool).await.unwrap();
    assert_eq!(after, evidence);
    assert!(after.contains(&first.clock.unwrap().sample_id.to_string()));
    assert!(after.contains(&terminal.clock.unwrap().sample_id.to_string()));
}

/// The replay's pre-read runs under the server bounds: a table lock ends it
/// at the lock timeout on the server, well before the 2 s client bound.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_order_pre_read_lock_wait_ends_at_the_server_bound(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    f.ledger.record_order(&record(), &link, seen(1)).await.unwrap();
    let before = snapshot(&pool).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE venue_orders IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        f.ledger.record_order(&record(), &link, seen(2)),
    )
    .await
    .expect("record_order exceeded its outer deadline");
    let elapsed = start.elapsed();
    held.rollback().await.unwrap();
    assert!(
        matches!(&result, Err(LedgerError::Backend(reason)) if reason.contains("lock timeout")),
        "{result:?}"
    );
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(snapshot(&pool).await, before);
    assert!(!f.ledger.record_order(&record(), &link, seen(2)).await.unwrap().changed);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn lost_commit_ack_is_uncertain_and_replay_does_not_bump(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    let pid = backend_pid(&proxied).await;
    proxy.enable_query_trace(32);
    let wire = proxy.stats().statements;
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE balance_legs"));
    assert!(matches!(
        ledger.record_order(&record(), &link, seen(1)).await,
        Err(LedgerError::Uncertain(_))
    ));
    retired(&pool, pid).await;
    wire_count(&ledger, &proxy, wire, 8);
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
    let before = snapshot(&pool).await;
    let n = ledger.statements_total();
    assert!(!ledger.record_order(&record(), &link, seen(2)).await.unwrap().changed);
    assert_eq!(ledger.statements_total() - n, 4);
    assert_eq!(snapshot(&pool).await, before);
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn blackholed_preread_is_bounded_and_connections_are_retired(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    let before = snapshot(&pool).await;
    proxy.set_mode(ProxyMode::Blackhole);
    let start = Instant::now();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            ledger.record_order(&record(), &unowned(), seen(1))
        )
        .await
        .expect("outer ledger deadline must resolve a blackholed read"),
        Err(LedgerError::Backend(_))
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(snapshot(&pool).await, before);
    proxy.set_mode(ProxyMode::Pass);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            ledger.record_order(&record(), &unowned(), seen(2))
        )
        .await
        .unwrap()
        .unwrap()
        .changed
    );
}

async fn wait_for_query(pool: &PgPool, needle: &str, event: &str) {
    tokio::time::timeout(Duration::from_secs(2),async {loop {
  let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND query LIKE $1 AND wait_event=$2)").bind(format!("%{needle}%")).bind(event).fetch_one(pool).await.unwrap();if found {break;}tokio::time::sleep(Duration::from_millis(5)).await;
 }}).await.expect("writer reached expected database wait");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn accounting_row_is_locked_before_the_order(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let mut r = record();
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    r.executed_qty = Decimal::from(3);
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT * FROM accounting_state FOR UPDATE")
        .execute(&mut *held)
        .await
        .unwrap();
    let ledger = Arc::new(PgVenueLedger::new(&pool, f.tenant));
    let task = tokio::spawn(async move { ledger.record_order(&r, &link, seen(2)).await });
    wait_for_query(&pool, "UPDATE accounting_state", "transactionid").await;
    let mut inspect = pool.begin().await.unwrap();
    sqlx::query("SELECT * FROM venue_orders FOR UPDATE NOWAIT")
        .execute(&mut *inspect)
        .await
        .expect("order remains unlocked while waiting for accounting");
    inspect.rollback().await.unwrap();
    held.rollback().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::from(3));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancellation_during_leg_update_rolls_back_and_retires_connection(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let before = snapshot(&pool).await;
    execute(&pool,"CREATE FUNCTION sleep_leg_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(10); RETURN NEW; END $$; CREATE TRIGGER sleep_leg_write BEFORE UPDATE ON balance_legs FOR EACH ROW EXECUTE FUNCTION sleep_leg_write()").await;
    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let ledger = Arc::new(PgVenueLedger::new(&proxied, f.tenant));
    let connections = proxy.stats().connections;
    let pid = backend_pid(&proxied).await;
    proxy.enable_query_trace(32);
    let wire = proxy.stats().statements;
    let link = f.link(LegOrderKind::Sell);
    let copy = ledger.clone();
    let task = tokio::spawn(async move { copy.record_order(&record(), &link, seen(1)).await });
    wait_for_query(&pool, "UPDATE balance_legs", "PgSleep").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    retired(&pool, pid).await;
    wire_count(&ledger, &proxy, wire, 7);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if snapshot(&pool).await == before {
                let mut tx = pool.begin().await.unwrap();
                sqlx::query("SET LOCAL lock_timeout='100ms'").execute(&mut *tx).await.unwrap();
                if sqlx::query("SELECT * FROM accounting_state FOR UPDATE NOWAIT")
                    .execute(&mut *tx)
                    .await
                    .is_ok()
                {
                    tx.rollback().await.unwrap();
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(snapshot(&pool).await, before);
    execute(&pool, "DROP TRIGGER sleep_leg_write ON balance_legs").await;
    assert!(
        ledger
            .record_order(&record(), &f.link(LegOrderKind::Sell), seen(2))
            .await
            .unwrap()
            .changed
    );
    assert!(
        proxy.stats().connections > connections,
        "cancelled connection must be retired, not returned to the pool"
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn racing_adoptions_to_real_different_legs_have_one_winner(pool: PgPool) {
    let a = Fixture::new(&pool).await;
    let b = Fixture::for_tenant(&pool, a.tenant).await;
    let r = record();
    a.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap();
    let version = a.version(&pool).await;
    let al = a.link(LegOrderKind::Sell);
    let bl = b.link(LegOrderKind::Sell);
    let (ar, br) = tokio::join!(
        a.ledger.record_order(&r, &al, seen(2)),
        b.ledger.record_order(&r, &bl, seen(2))
    );
    assert!(matches!(
        (&ar, &br),
        (Ok(_), Err(LedgerError::IdentityConflict)) | (Err(LedgerError::IdentityConflict), Ok(_))
    ));
    assert_eq!(a.version(&pool).await, version + 1);
    let ac = a.consumed(&pool).await;
    let bc = b.consumed(&pool).await;
    assert_eq!(ac + bc, Decimal::from(2));
    assert!(ac == Decimal::ZERO || bc == Decimal::ZERO);
    let before = snapshot(&pool).await;
    let (winner, loser) = if ar.is_ok() { (&al, &bl) } else { (&bl, &al) };
    assert!(!a.ledger.record_order(&r, winner, seen(3)).await.unwrap().changed);
    assert_eq!(
        a.ledger.record_order(&r, loser, seen(3)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn unresolved_admission_gains_cohort_only_when_real_entry_record_arrives(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    // Admission committed with handed_off=false models the durable state after a
    // lost strict-journal ACK: the later venue record, not uncertainty, sets
    // cohort.
    assert!(sqlx::query_scalar::<_, Option<chrono::NaiveDate>>(
        "SELECT cohort_month FROM balance_legs"
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .is_none());
    let before = snapshot(&pool).await;
    let observations = [OrderObservation::NotFound, OrderObservation::Archived];
    for observation in observations {
        assert!(!matches!(observation, OrderObservation::Found(_)));
    }
    let answers = [
        CancelAnswer::Unknown { reason: "fixture timeout".into() },
        CancelAnswer::NotFound { code: -2011 },
    ];
    for answer in answers {
        assert!(!matches!(answer, CancelAnswer::Cancelled(_)));
    }
    assert_eq!(snapshot(&pool).await, before);
    let mut r = record();
    r.side = OrderSide::Buy;
    r.executed_qty = Decimal::ZERO;
    r.cumulative_quote_qty = Some(Decimal::ZERO);
    r.status = "NEW".into();
    let link = f.link(LegOrderKind::Entry);
    let version = f.version(&pool).await;
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    assert_eq!(f.version(&pool).await, version + 1);
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT cohort_month=decided_month FROM balance_legs")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let before = snapshot(&pool).await;
    assert!(!f.ledger.record_order(&r, &link, seen(2)).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn stalled_leg_write_is_bounded_atomic_and_pool_remains_usable(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let before = snapshot(&pool).await;
    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    let pid = backend_pid(&proxied).await;
    proxy.enable_query_trace(32);
    let wire = proxy.stats().statements;
    execute(&pool,"CREATE FUNCTION stall_leg_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(10); RETURN NEW; END $$; CREATE TRIGGER stall_leg_write BEFORE UPDATE ON balance_legs FOR EACH ROW EXECUTE FUNCTION stall_leg_write()").await;
    let start = Instant::now();
    assert!(matches!(
        ledger.record_order(&record(), &f.link(LegOrderKind::Sell), seen(1)).await,
        Err(LedgerError::Backend(_))
    ));
    assert!(start.elapsed() < Duration::from_millis(2500));
    retired(&pool, pid).await;
    wire_count(&ledger, &proxy, wire, 7);
    assert_eq!(snapshot(&pool).await, before);
    execute(&pool, "DROP TRIGGER stall_leg_write ON balance_legs").await;
    ledger
        .record_order(&record(), &f.link(LegOrderKind::Sell), seen(2))
        .await
        .unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn wire_statement_counts_match_the_counter_and_replay_contract(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let link = f.link(LegOrderKind::Sell);
    let (proxy, proxied) = through_proxy(&pool).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    let mut r = record();
    for (seen_at, expected) in [(1, 8), (2, 4), (3, 7)] {
        if seen_at == 3 {
            r.update_time_ms = Some(123);
        }
        let counter = ledger.statements_total();
        let wire = proxy.stats().statements;
        ledger.record_order(&r, &link, seen(seen_at)).await.unwrap();
        assert_eq!(ledger.statements_total() - counter, expected);
        assert_eq!(proxy.stats().statements - wire, expected);
    }
}

#[test]
fn order_ledger_has_no_production_callers() {
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != "venue_ledger_store" {
                    visit(&path);
                }
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !source.contains(".record_order("),
                    "ledger order writer activated in {}",
                    path.display()
                );
            }
        }
    }
    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn other_tenant_and_closed_leg_cannot_adopt_an_order(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let other = Fixture::new(&pool).await;
    let r = record();
    f.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap();
    let before = snapshot(&pool).await;
    assert_eq!(
        other.ledger.record_order(&r, &other.link(LegOrderKind::Sell), seen(2)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
    sqlx::query("UPDATE balance_legs SET closed_at=$1 WHERE position_id=$2")
        .bind(seen(3).at)
        .bind(f.position)
        .execute(&pool)
        .await
        .unwrap();
    let before = snapshot(&pool).await;
    assert_eq!(
        f.ledger.record_order(&r, &f.link(LegOrderKind::Sell), seen(4)).await,
        Err(LedgerError::IdentityConflict)
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn randomized_concurrent_evidence_conserves_the_single_leg_contribution(pool: PgPool) {
    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::TestRunner,
    };
    let f = Fixture::new(&pool).await;
    let mut runner = TestRunner::default();
    let strategy = proptest::collection::vec((1u32..10, 0u64..12), 2..7);
    for round in 0..16 {
        let schedule = strategy.new_tree(&mut runner).unwrap().current();
        let mut base = record();
        base.exchange_order_id = (100 + round).to_string();
        base.client_order_id = format!("race-{round}");
        let mut plain = unowned();
        plain.client_order_id = base.client_order_id.clone();
        f.ledger.record_order(&base, &plain, seen(1)).await.unwrap();
        let mut link = f.link(LegOrderKind::Sell);
        link.client_order_id = base.client_order_id.clone();
        let version = f.version(&pool).await;
        let consumed = f.consumed(&pool).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(schedule.len()));
        let ledger = Arc::new(PgVenueLedger::new(&pool, f.tenant));
        let mut tasks = Vec::new();
        for (qty, delay) in schedule.iter().copied() {
            let mut r = base.clone();
            r.executed_qty = Decimal::from(qty);
            r.cumulative_quote_qty = Some(Decimal::from(qty * 100));
            let link = link.clone();
            let ledger = ledger.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                ledger.record_order(&r, &link, seen(2)).await.unwrap()
            }));
        }
        let mut changed = 0;
        for task in tasks {
            changed += i64::from(task.await.unwrap().changed);
        }
        let maximum = Decimal::from(schedule.iter().map(|(qty, _)| *qty).max().unwrap().max(2));
        assert_eq!(f.consumed(&pool).await, consumed + maximum);
        assert_eq!(f.version(&pool).await, version + changed);
        base.executed_qty = maximum;
        base.cumulative_quote_qty = Some(maximum * Decimal::from(100));
        let before = snapshot(&pool).await;
        assert!(!f.ledger.record_order(&base, &link, seen(9)).await.unwrap().changed);
        assert_eq!(snapshot(&pool).await, before);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn adoption_with_a_lost_ack_converges_without_consuming_twice(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let r = record();
    f.ledger.record_order(&r, &unowned(), seen(1)).await.unwrap();
    let link = f.link(LegOrderKind::Sell);
    let version = f.version(&pool).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let ledger = PgVenueLedger::new(&proxied, f.tenant);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE balance_legs"));
    assert!(matches!(
        ledger.record_order(&r, &link, seen(2)).await,
        Err(LedgerError::Uncertain(_))
    ));
    assert_eq!(f.consumed(&pool).await, Decimal::from(2));
    assert_eq!(f.version(&pool).await, version + 1);
    let before = snapshot(&pool).await;
    assert!(!ledger.record_order(&r, &link, seen(3)).await.unwrap().changed);
    assert_eq!(snapshot(&pool).await, before);
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_executed_entry_never_consumes_the_leg_inventory(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let mut r = record();
    r.side = OrderSide::Buy;
    let link = f.link(LegOrderKind::Entry);
    f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
    r.executed_qty = Decimal::from(3);
    r.cumulative_quote_qty = Some(Decimal::from(300));
    f.ledger.record_order(&r, &link, seen(2)).await.unwrap();
    assert_eq!(f.consumed(&pool).await, Decimal::ZERO);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_partial_quote_is_not_frozen_as_a_larger_terminal_total(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    for (index, terminal_first) in [false, true].into_iter().enumerate() {
        let mut r = record();
        r.exchange_order_id = (90 + index).to_string();
        r.client_order_id = format!("quote-{index}");
        let mut link = unowned();
        link.client_order_id = r.client_order_id.clone();
        f.ledger.record_order(&r, &link, seen(1)).await.unwrap();
        r.cumulative_quote_qty = None;
        let before = snapshot(&pool).await;
        assert!(!f.ledger.record_order(&r, &link, seen(2)).await.unwrap().changed);
        assert_eq!(snapshot(&pool).await, before);
        r.executed_qty = Decimal::from(3);
        if terminal_first {
            r.phase = OrderPhase::Terminal;
            r.status = "CANCELED".into();
        }
        f.ledger.record_order(&r, &link, seen(3)).await.unwrap();
        let quote: Option<Decimal> = sqlx::query_scalar(
            "SELECT cumulative_quote FROM venue_orders WHERE exchange_order_id=$1",
        )
        .bind(&r.exchange_order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quote, None);
        r.phase = OrderPhase::Terminal;
        r.status = "CANCELED".into();
        f.ledger.record_order(&r, &link, seen(4)).await.unwrap();
        let before = snapshot(&pool).await;
        r.cumulative_quote_qty = Some(Decimal::from(300));
        assert!(!f.ledger.record_order(&r, &link, seen(5)).await.unwrap().changed);
        assert_eq!(snapshot(&pool).await, before);
        let mut owned = f.link(LegOrderKind::Sell);
        owned.client_order_id = r.client_order_id.clone();
        let consumed = f.consumed(&pool).await;
        let version = f.version(&pool).await;
        assert!(f.ledger.record_order(&r, &owned, seen(6)).await.unwrap().changed);
        assert_eq!(f.consumed(&pool).await, consumed + Decimal::from(3));
        assert_eq!(f.version(&pool).await, version + 1);
        let quote: Option<Decimal> = sqlx::query_scalar(
            "SELECT cumulative_quote FROM venue_orders WHERE exchange_order_id=$1",
        )
        .bind(&r.exchange_order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(quote, None);
        let before = snapshot(&pool).await;
        assert!(!f.ledger.record_order(&r, &owned, seen(7)).await.unwrap().changed);
        assert_eq!(snapshot(&pool).await, before);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn locked_noop_rollback_is_counted_once_even_when_its_ack_is_lost(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    for (index, lose_ack) in [false, true].into_iter().enumerate() {
        let mut r = record();
        r.exchange_order_id = (200 + index).to_string();
        r.client_order_id = format!("recheck-{index}");
        let mut unowned = unowned();
        unowned.client_order_id = r.client_order_id.clone();
        f.ledger.record_order(&r, &unowned, seen(1)).await.unwrap();
        let mut link = f.link(LegOrderKind::Sell);
        link.client_order_id = r.client_order_id.clone();
        let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
        let ledger = Arc::new(PgVenueLedger::new(&proxied, f.tenant));
        let pid = backend_pid(&proxied).await;
        proxy.enable_query_trace(32);
        let wire = proxy.stats().statements;
        let mut winner = pool.begin().await.unwrap();
        sqlx::query("SELECT * FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2 FOR UPDATE")
            .bind(f.tenant)
            .bind(VENUE)
            .execute(&mut *winner)
            .await
            .unwrap();
        if lose_ack {
            proxy.set_mode(ProxyMode::DropRollbackAckOf("UPDATE accounting_state"));
        }
        let copy = ledger.clone();
        let incoming = r.clone();
        let owner = link.clone();
        let task = tokio::spawn(async move { copy.record_order(&incoming, &owner, seen(2)).await });
        wait_for_query(&pool, "UPDATE accounting_state", "transactionid").await;
        // Force the winner between the losing writer's pre-read and locked recheck.
        // This fixture follows the same accounting-first atomic adoption protocol;
        // the real concurrent-writer invariants are separately randomized above.
        let OrderOwner::Leg { intent_id, position_id, .. } = link.owner else {
            unreachable!()
        };
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(f.tenant)
        .bind(VENUE)
        .execute(&mut *winner)
        .await
        .unwrap();
        sqlx::query("UPDATE venue_orders SET owner='leg',intent_id=$1,position_id=$2,kind='sell',last_checked_at=$3 WHERE exchange_order_id=$4").bind(intent_id).bind(position_id).bind(seen(2).at).bind(&r.exchange_order_id).execute(&mut *winner).await.unwrap();
        sqlx::query("UPDATE balance_legs SET consumed_cum=consumed_cum+2 WHERE position_id=$1")
            .bind(position_id)
            .execute(&mut *winner)
            .await
            .unwrap();
        winner.commit().await.unwrap();
        let before = snapshot(&pool).await;
        let outcome = task.await.unwrap();
        if lose_ack {
            assert!(matches!(outcome, Err(LedgerError::Backend(_))));
            retired(&pool, pid).await;
            assert_eq!(proxy.rollback_acks_dropped(), 1);
        } else {
            assert!(!outcome.unwrap().changed);
            tokio::time::timeout(Duration::from_secs(2), async {
                while proxied.num_idle() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        wire_count(&ledger, &proxy, wire, 6);
        assert_eq!(
            proxy
                .query_trace()
                .iter()
                .filter(|sql| sql.trim().eq_ignore_ascii_case("ROLLBACK"))
                .count(),
            1
        );
        assert_eq!(snapshot(&pool).await, before);
        assert!(!ledger.record_order(&r, &link, seen(3)).await.unwrap().changed);
        assert_eq!(snapshot(&pool).await, before);
    }
}
