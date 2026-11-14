use chrono::DateTime;
use robson_domain::{MonthlyBudgetModel, OrderSide};
use robson_exec::{
    venue::{OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{LegOrderKind, OrderLink, OrderOwner, SeenAt},
};
use robsond::venue_ledger_store::PgVenueLedger;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

pub const VENUE: &str = "binance:spot";
pub fn seen(second: i64) -> SeenAt {
    SeenAt {
        at: DateTime::from_timestamp(1_794_614_400 + second, 0).unwrap(),
        clock: None,
    }
}
pub fn record() -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: "7".into(),
        client_order_id: "original".into(),
        orig_client_order_id: None,
        symbol: "BTCUSDT".into(),
        side: OrderSide::Sell,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "PARTIALLY_FILLED".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::from(10),
        executed_qty: Decimal::from(2),
        cumulative_quote_qty: Some(Decimal::from(200)),
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    }
}
pub fn unowned() -> OrderLink {
    OrderLink {
        venue: VenueId::new(VENUE),
        account_scope: "scope".into(),
        owner: OrderOwner::Discovered,
        client_order_id: "original".into(),
    }
}
pub struct Fixture {
    pub tenant: Uuid,
    pub position: Uuid,
    pub admission: Uuid,
    pub ledger: PgVenueLedger,
}
impl Fixture {
    pub async fn new(pool: &PgPool) -> Self {
        Self::for_tenant(pool, Uuid::now_v7()).await
    }
    pub async fn for_tenant(pool: &PgPool, tenant: Uuid) -> Self {
        let position = Uuid::now_v7();
        let admission = Uuid::now_v7();
        let ledger = PgVenueLedger::new(pool, tenant);
        ledger
            .ensure_accounting_row(
                &VenueId::new(VENUE),
                MonthlyBudgetModel::NetFromStartNonExpandingV1,
            )
            .await
            .unwrap();
        ledger
            .insert_route(position, &VenueId::new(VENUE), "scope", seen(0).at)
            .await
            .unwrap();
        sqlx::query("INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at) VALUES($1,$2,$3,'scope',$4,$5,'entry','{}',$6)")
   .bind(admission).bind(tenant).bind(VENUE).bind(admission.to_string()).bind(position).bind(seen(0).at).execute(pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(tenant)
        .bind(VENUE)
        .execute(&mut *tx)
        .await
        .unwrap();
        // Synthetic chart fixture: 90 is its analysis's second support, never a
        // percentage stop. BTCUSDT is only an example symbol; other constants
        // are fixture exchange metadata.
        sqlx::query("SELECT balance_leg_admit($1,$2,'scope','BTCUSDT',$3,$4,'2026-11-01',10,10,200,5,0.01,'base','fixture-chart',$5,'fixture-hash',1,1.01,100,0.1,12,90,$5,false)")
   .bind(tenant).bind(VENUE).bind(position).bind(admission).bind(seen(0).at).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        Self { tenant, position, admission, ledger }
    }
    pub fn link(&self, kind: LegOrderKind) -> OrderLink {
        OrderLink {
            owner: OrderOwner::Leg {
                intent_id: if kind == LegOrderKind::Entry {
                    self.admission
                } else {
                    Uuid::now_v7()
                },
                position_id: self.position,
                kind,
            },
            ..unowned()
        }
    }
    pub async fn consumed(&self, pool: &PgPool) -> Decimal {
        sqlx::query_scalar("SELECT consumed_cum FROM balance_legs WHERE position_id=$1")
            .bind(self.position)
            .fetch_one(pool)
            .await
            .unwrap()
    }
    pub async fn version(&self, pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(self.tenant)
        .bind(VENUE)
        .fetch_one(pool)
        .await
        .unwrap()
    }
}
pub async fn snapshot(pool: &PgPool) -> Vec<String> {
    // Textual JSON preserves every NUMERIC digit; never deserialize through f64.
    sqlx::query_scalar("SELECT 'a:'||to_jsonb(t)::text FROM accounting_state t UNION ALL SELECT 'l:'||to_jsonb(t)::text FROM balance_legs t UNION ALL SELECT 'o:'||to_jsonb(t)::text FROM venue_orders t UNION ALL SELECT 'f:'||to_jsonb(t)::text FROM order_fills t ORDER BY 1").fetch_all(pool).await.unwrap()
}
pub async fn execute(pool: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(pool).await.unwrap();
}
pub async fn through_proxy(pool: &PgPool) -> (robson_testkit::db_proxy::DbProxy, PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = robson_testkit::db_proxy::DbProxy::start(upstream).await.unwrap();
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL").unwrap(),
        proxy.addr(),
        options.get_database().unwrap(),
    )
    .unwrap();
    let proxied = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    (proxy, proxied)
}
