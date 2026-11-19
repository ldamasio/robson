use super::{
    batch, cold_cancels, cold_sells, empty_budget, hot_query, install, linked_sell, scope, seen,
    through_proxy_limit, zero_execution_order, Fixture, IntentKind, JournalBackend, LegOrderKind,
    OrderKey, OrderOwner, OrderPhase, PgJournalBackend, PgPool, ProtectionKind, Scope, TradeFill,
    TradePage, HOT,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::time::{Duration, Instant};

use crate::reader::Snapshot;

const STOP_ID: &str = "900001";
const MISSING_ID: &str = "900002";

async fn append_closed_history(
    f: &Fixture,
    journal: &PgJournalBackend,
    s: Scope,
    history: std::ops::Range<i64>,
) {
    for n in history {
        let exit = batch(s, IntentKind::Exit, None);
        journal.commit(&exit).await.unwrap();
        let link = linked_sell(f, &exit.intent);
        let mut observed =
            zero_execution_order(&(1000 + n).to_string(), "CANCELED", OrderPhase::Terminal);
        observed.client_order_id = link.client_order_id.clone();
        assert!(f.ledger.record_order(&observed, &link, seen(3 + n)).await.unwrap().changed);
    }
}

async fn explain_hot(pool: &PgPool, s: Scope, candidate: &str) -> Value {
    let sql = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {HOT}");
    sqlx::query_scalar(&sql)
        .bind(s.tenant)
        .bind(s.venue)
        .bind(s.account)
        .bind(s.position)
        .bind(s.symbol)
        .bind(candidate)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn historical_nodes<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    if matches!(node["Relation Name"].as_str(), Some("venue_orders" | "order_intents")) {
        out.push(node);
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            historical_nodes(child, out);
        }
    }
}

fn explain_rows(node: &Value, field: &str) -> u64 {
    node[field].as_u64().unwrap_or(0)
}

async fn measure(pool: &PgPool, s: Scope, expected_closed: i64) {
    // The history inventory and its joins are fixture assertions outside the timed query.
    let closed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM venue_orders o JOIN order_intents j ON j.intent_id=o.intent_id
         WHERE (o.tenant_id,o.venue_id,o.account_scope,o.position_id,o.symbol)=($1,$2,$3,$4,$5)
           AND o.phase='terminal' AND o.status='CANCELED' AND o.owner='leg' AND o.kind='sell'
           AND (j.tenant_id,j.venue_id,j.account_scope,j.position_id,j.client_order_id)
               = (o.tenant_id,o.venue_id,o.account_scope,o.position_id,o.client_order_id)
           AND j.kind='exit'",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.position)
    .bind(s.symbol)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(closed, expected_closed);
    let started = Instant::now();
    let cold = tokio::time::timeout(Duration::from_secs(30), async {
        Snapshot::open(pool, s).await?.finish().await
    })
    .await
    .expect("the exhaustive scale oracle exceeded its fixture deadline")
    .unwrap();
    assert_eq!(cold_sells(&cold), (1, 0));
    assert_eq!(cold_cancels(&cold), (0, 0, 0));
    println!(
        "order-set closed_history={closed} exhausted_cold_ms={}",
        started.elapsed().as_millis()
    );
    let witness = hot_query(s, Some(STOP_ID)).fetch_one(pool).await.unwrap();
    assert_eq!((witness.leg_sell, witness.symbol_sell), (1, 0));
    assert_eq!(
        (witness.grouped_cancel, witness.ungroupable_cancel, witness.unbound_cancel),
        (0, 0, 0)
    );
    assert_eq!(witness.accounting_version, cold.version);
    assert!(witness.exact_stop_identity);

    let (proxy, proxied) = through_proxy_limit(pool, 1).await;
    let mut conn = proxied.acquire().await.unwrap();
    let start = proxy.stats().statements;
    proxy.enable_exact_query_trace(4);
    let exact = hot_query(s, Some(STOP_ID)).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(exact.stop_id.as_deref(), Some(STOP_ID));
    assert!(exact.exact_stop_identity);
    assert_eq!(proxy.stats().statements - start, 1);
    let absent = hot_query(s, Some(MISSING_ID)).fetch_one(&mut *conn).await.unwrap();
    assert!(absent.stop_id.is_none() && !absent.exact_stop_identity);
    assert_eq!(proxy.stats().statements - start, 2);
    assert_eq!(exact.accounting_version, witness.accounting_version);
    assert_eq!(absent.accounting_version, witness.accounting_version);
    let trace = proxy.query_trace();
    assert_eq!(trace.len(), 2);
    assert_eq!(trace[0], HOT);
    assert_eq!(trace[1], HOT);
    assert_eq!(proxy.query_trace_dropped(), 0);
    println!("order-set closed_history={closed} hot_sql_per_read=1 query={:?}", trace[0]);

    // EXPLAIN adds real execution and reporting overhead. Its calls are deliberately
    // outside the proxied application statement measurements and retain default plans.
    for candidate in [STOP_ID, MISSING_ID] {
        let explain = explain_hot(pool, s, candidate).await;
        println!("order-set closed_history={closed} candidate={candidate} explain={explain}");
        let plan = &explain[0]["Plan"];
        assert_eq!(plan["Actual Rows"].as_u64(), Some(1));
        if closed >= 512 {
            let mut nodes = Vec::new();
            historical_nodes(plan, &mut nodes);
            for table in ["venue_orders", "order_intents"] {
                assert!(
                    nodes.iter().any(|node| node["Relation Name"] == table),
                    "EXPLAIN must retain the exact {table} probe"
                );
            }
            for node in nodes {
                let loops = node["Actual Loops"].as_u64().expect("native loop count");
                assert!(loops <= 1, "{node}");
                assert!(node["Actual Rows"].as_u64().expect("native row count") <= 1, "{node}");
                if loops != 0 {
                    assert_ne!(node["Node Type"], "Seq Scan", "{node}");
                    assert_eq!(explain_rows(node, "Rows Removed by Filter"), 0, "{node}");
                    assert_eq!(explain_rows(node, "Rows Removed by Index Recheck"), 0, "{node}");
                }
            }
        }
    }
}

async fn observe_fixture_deficit(pool: &PgPool, f: &Fixture, s: Scope, key: &OrderKey) {
    let mut tx = pool.begin().await.unwrap();
    let version: i64 = sqlx::query_scalar(
        "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2
         RETURNING version",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let update = sqlx::query(
        "UPDATE balance_legs SET gross_qty=2, acquisition_cost=200, base_commission=1,
         executable_qty=1, base_commission_final=true
         WHERE (tenant_id,venue_id,account_scope,position_id,symbol)=($1,$2,$3,$4,$5)",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.position)
    .bind(s.symbol)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(update.rows_affected(), 1);
    tx.commit().await.unwrap();

    let before = hot_query(s, Some(STOP_ID)).fetch_one(pool).await.unwrap();
    assert_eq!(before.accounting_version, version);
    assert_eq!(before.consumed_cum, Decimal::ZERO);
    let at = seen(520).at;
    let fills = TradePage {
        trades: vec![TradeFill {
            trade_id: "550".into(),
            price: Decimal::from(100),
            qty: Decimal::from(2),
            quote_qty: Decimal::from(200),
            commission: Decimal::ZERO,
            commission_asset: "USDT".into(),
            time: Some(at),
        }],
        next_from_id: None,
    };
    assert!(f.ledger.record_trades(key, &fills, at).await.unwrap().changed);
    let after = hot_query(s, Some(STOP_ID)).fetch_one(pool).await.unwrap();
    assert_eq!(after.accounting_version, version + 1);
    assert_eq!(after.gross_qty, Some(Decimal::from(2)));
    assert_eq!(after.acquisition_cost, Some(Decimal::from(200)));
    assert_eq!(after.base_commission, Some(Decimal::ONE));
    assert_eq!(after.executable_qty, Some(Decimal::ONE));
    assert_eq!(after.consumed_cum, Decimal::from(2));
    let remaining = after.executable_qty.unwrap() - after.consumed_cum;
    assert_eq!(remaining, -Decimal::ONE);
    println!("order-set fixture_preseed_q_c_b_x=true remaining={remaining}");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn hot_order_set_retains_keyed_access_at_closed_history_scale(pool: PgPool) {
    empty_budget::initialize(&pool, seen(0).at).await.unwrap();
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;

    let protection = batch(s, IntentKind::Protection, None);
    journal.commit(&protection).await.unwrap();
    let opening = hot_query(s, None).fetch_one(&pool).await.unwrap();
    assert!(opening.executable_qty.is_none());
    let mut stop_link = linked_sell(&f, &protection.intent);
    stop_link.owner = OrderOwner::Leg {
        position_id: s.position,
        intent_id: protection.intent.intent_id,
        kind: LegOrderKind::Stop,
    };
    let mut stop = zero_execution_order(STOP_ID, "NEW", OrderPhase::Live);
    stop.client_order_id = stop_link.client_order_id.clone();
    stop.order_type = "STOP_LOSS".into();
    stop.protection_kind = Some(ProtectionKind::MarketOnTrigger);
    stop.stop_price = Some(opening.target_trigger);
    assert!(f.ledger.record_order(&stop, &stop_link, seen(2)).await.unwrap().changed);

    append_closed_history(&f, &journal, s, 0..2).await;
    measure(&pool, s, 2).await;
    append_closed_history(&f, &journal, s, 2..512).await;
    measure(&pool, s, 512).await;

    // This accounting-first preseed applies to the fixture only.
    let key = OrderKey::from_record(&stop, &stop_link).unwrap();
    observe_fixture_deficit(&pool, &f, s, &key).await;
}
