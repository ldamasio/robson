use std::{sync::Arc, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::{MonthlyBudgetModel, OrderSide};
use robson_exec::{
    order_journal::{
        AdmissionRow, BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch,
    },
    ports::TradeFill,
    venue::{OrderPhase, TradePage, VenueId, VenueOrderRecord},
    venue_ledger::{LegOrderKind, OrderKey, OrderLink, OrderOwner, SeenAt},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use robsond::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

pub const SYMBOL: &str = "BTCUSDT"; // A labelled fixture pair, never a policy restriction.
pub fn at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_794_614_400, 0).unwrap()
}
pub fn venue() -> VenueId {
    VenueId::new(VenueId::BINANCE_SPOT)
}
pub fn seen() -> SeenAt {
    SeenAt { at: at(), clock: None }
}
pub fn fill(id: u64, qty: Decimal) -> TradeFill {
    TradeFill {
        trade_id: id.to_string(),
        price: dec!(100),
        qty,
        quote_qty: qty * dec!(100),
        commission: dec!(0.01),
        commission_asset: "USDT".into(),
        time: Some(at()),
    }
}
pub fn page(id: u64, qty: Decimal) -> TradePage {
    TradePage {
        trades: vec![fill(id, qty)],
        next_from_id: None,
    }
}

pub struct Fixture {
    pub tenant: Uuid,
    pub position: Uuid,
    pub scope: String,
    pub entry: JournalBatch,
}
impl Fixture {
    pub async fn new(pool: &PgPool) -> Self {
        Self::for_tenant(pool, Uuid::now_v7()).await
    }
    pub async fn for_tenant(pool: &PgPool, tenant: Uuid) -> Self {
        Self::for_tenant_scope(pool, tenant, None).await
    }
    pub async fn for_tenant_scope(pool: &PgPool, tenant: Uuid, scope: Option<String>) -> Self {
        let position = Uuid::now_v7();
        let scope = scope.unwrap_or_else(|| position.to_string());
        let ledger = PgVenueLedger::new(pool, tenant);
        ledger.ensure_accounting_row(&venue(), MonthlyBudgetModel::HwmV1).await.unwrap();
        ledger.insert_route(position, &venue(), &scope, at()).await.unwrap();
        let entry = JournalBatch {
            intent: IntentRow {
                intent_id: Uuid::now_v7(),
                venue_id: venue().to_string(),
                account_scope: scope.clone(),
                client_order_id: Uuid::now_v7().simple().to_string(),
                position_id: Some(position),
                kind: IntentKind::Entry,
                cancels_order_id: None,
                params: serde_json::json!({"symbol":SYMBOL}),
                created_at: at(),
            },
            attempts: vec![],
            outcome: None,
            balance: Some(BalanceBatch {
                admission: Some(Arc::new(AdmissionRow {
                    position_id: position,
                    symbol: SYMBOL.into(),
                    decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
                    taker_bps: dec!(10),
                    gap_bps: dec!(10),
                    notional_margin_bps: dec!(2),
                    min_notional: dec!(10),
                    lot_step: dec!(0.001),
                    fee_asset_policy: "base".into(),
                    envelope_source: "fixture exchange rules".into(),
                    envelope_as_of: at(),
                    envelope_hash: "fixture-hash".into(),
                    planned_qty: dec!(10000),
                    requested_qty: dec!(10001),
                    reference_price: dec!(100),
                    remainder_reserve: dec!(1),
                    admission_charge: dec!(1000),
                    // Fixture chart analysis: second 15-minute support is 90.
                    initial_trigger: dec!(90),
                })),
                handed_off: false,
                commit_bound: Duration::from_millis(500),
            }),
        };
        PgJournalBackend::new(pool, tenant).commit(&entry).await.unwrap();
        Self { tenant, position, scope, entry }
    }
    pub fn ledger(&self, pool: &PgPool) -> PgVenueLedger {
        PgVenueLedger::new(pool, self.tenant)
    }
    pub fn record(&self, id: u64, executed: Decimal, phase: OrderPhase) -> VenueOrderRecord {
        VenueOrderRecord {
            exchange_order_id: id.to_string(),
            client_order_id: format!("order-{id}"),
            orig_client_order_id: None,
            symbol: SYMBOL.into(),
            side: OrderSide::Sell,
            order_type: "MARKET".into(),
            protection_kind: None,
            status: if phase == OrderPhase::Terminal {
                "CANCELED"
            } else {
                "PARTIALLY_FILLED"
            }
            .into(),
            phase,
            orig_qty: dec!(10000),
            executed_qty: executed,
            cumulative_quote_qty: None,
            stop_price: None,
            expiry_reason: None,
            transact_time_ms: None,
            time_ms: None,
            update_time_ms: None,
            working_time_ms: None,
        }
    }
    pub fn link(&self, id: u64, owner: OrderOwner) -> OrderLink {
        OrderLink {
            venue: venue(),
            account_scope: self.scope.clone(),
            owner,
            client_order_id: format!("order-{id}"),
        }
    }
    pub fn owned(&self, id: u64) -> OrderLink {
        self.link(id, OrderOwner::Leg {
            position_id: self.position,
            intent_id: Uuid::now_v7(),
            kind: LegOrderKind::Sell,
        })
    }
    pub fn key(&self, id: u64) -> OrderKey {
        OrderKey {
            venue: venue(),
            account_scope: self.scope.clone(),
            symbol: SYMBOL.into(),
            exchange_order_id: id.to_string().parse().unwrap(),
        }
    }
    pub async fn seed_order(
        &self,
        pool: &PgPool,
        id: u64,
        executed: Decimal,
        phase: OrderPhase,
    ) -> OrderKey {
        self.ledger(pool)
            .record_order(&self.record(id, executed, phase), &self.owned(id), seen())
            .await
            .unwrap();
        self.key(id)
    }
    pub async fn version(&self, pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(self.tenant)
        .bind(venue().as_str())
        .fetch_one(pool)
        .await
        .unwrap()
    }
    pub async fn consumed(&self, pool: &PgPool) -> Decimal {
        sqlx::query_scalar("SELECT consumed_cum FROM balance_legs WHERE position_id=$1")
            .bind(self.position)
            .fetch_one(pool)
            .await
            .unwrap()
    }
}

pub async fn snapshot(pool: &PgPool) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    for table in [
        "accounting_state",
        "balance_legs",
        "venue_orders",
        "order_fills",
    ] {
        let rows = sqlx::query_scalar(&format!(
            "SELECT row_to_json(t)::text FROM {table} t ORDER BY row_to_json(t)::text"
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        result.push(rows);
    }
    result
}

pub async fn through_proxy(pool: &PgPool, max: u32) -> (DbProxy, PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = DbProxy::start(upstream).await.unwrap();
    let database = options.get_database().unwrap();
    let url = database_url_through(&std::env::var("DATABASE_URL").unwrap(), proxy.addr(), database)
        .unwrap();
    // Fault tests target the operation, not an acquisition ping.
    let proxied = PgPoolOptions::new()
        .max_connections(max)
        .test_before_acquire(false)
        .connect(&url)
        .await
        .unwrap();
    (proxy, proxied)
}

pub async fn wait_for_query(pool: &PgPool, fragment: &str) {
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND query LIKE $1 AND wait_event IS NOT NULL)")
                .bind(format!("%{fragment}%")).fetch_one(pool).await.unwrap();
            if found {break;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("writer never reached injected wait");
}

/// Count through asynchronous retirement: a return from the writer alone does
/// not prove SQLx has finished flushing/dropping the old physical connection.
pub async fn wait_for_backend_exit(pool: &PgPool, pid: i32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)")
                    .bind(pid)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if !exists {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("retired backend did not exit within its deadline");
}
