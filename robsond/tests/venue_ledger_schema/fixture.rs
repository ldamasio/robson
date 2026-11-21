//! Shared test-only original schema setup; no readiness/source grant.
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[path = "empty_budget.rs"]
pub(crate) mod empty_budget;

pub(super) const VENUE: &str = "binance:spot";
// Synthetic chart fixture: the second support identified by its analysis.
pub(super) const CHART_STOP: &str = "90";

#[derive(Clone)]
pub(super) struct Fixture {
    pub(super) tenant: Uuid,
    pub(super) position: Uuid,
    pub(super) intent: Uuid,
    pub(super) at: DateTime<Utc>,
}

pub(super) async fn execute(pool: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(pool).await.unwrap();
}

impl Fixture {
    // Opt-in only for fresh macro-owned full-graph tests. Legacy/incomplete
    // tests continue to use new/for_tenant and never initialize automatically.
    pub(super) async fn new_for_all_rows(pool: &PgPool) -> Result<Self, sqlx::Error> {
        let at = DateTime::parse_from_rfc3339("2026-11-14T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        empty_budget::initialize(pool, at).await?;
        Ok(Self::new(pool).await)
    }

    pub(super) async fn new(pool: &PgPool) -> Self {
        Self::for_tenant(pool, Uuid::now_v7()).await
    }

    pub(super) async fn for_tenant(pool: &PgPool, tenant: Uuid) -> Self {
        let f = Self {
            tenant,
            position: Uuid::now_v7(),
            intent: Uuid::now_v7(),
            at: DateTime::parse_from_rfc3339("2026-11-14T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        sqlx::query("INSERT INTO accounting_state (tenant_id, venue_id, budget_model) VALUES ($1,$2,'net_from_start_non_expanding_v1')")
            .bind(f.tenant).bind(VENUE).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,$3,'scope',$4)")
            .bind(f.position)
            .bind(f.tenant)
            .bind(VENUE)
            .bind(f.at)
            .execute(pool)
            .await
            .unwrap();
        f.insert_intent(pool, f.intent).await;
        f.admit(pool, f.intent, "2026-11-01", false).await.unwrap();
        f
    }

    pub(super) async fn insert_intent(&self, pool: &PgPool, intent: Uuid) {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at) VALUES ($1,$2,$3,'scope',$4,$5,'entry','{}',$6)")
            .bind(intent).bind(self.tenant).bind(VENUE).bind(intent.to_string()).bind(self.position).bind(self.at)
            .execute(pool).await.unwrap();
    }

    pub(super) async fn admission(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        intent: Uuid,
        month: &str,
        handed_off: bool,
    ) -> Result<(), sqlx::Error> {
        // The real future writer's protocol: accounting row first, then admission.
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(self.tenant)
        .bind(VENUE)
        .execute(&mut **tx)
        .await?;
        sqlx::query("SELECT balance_leg_admit($1,$2,'scope','BTCUSDT',$3,$4,$5::text::date,10,10,200,5,0.01,'base','fixture-chart',$6,'fixture-hash',1,1.01,100,0.1,12,$7::text::numeric,$6,$8)")
            .bind(self.tenant).bind(VENUE).bind(self.position).bind(intent).bind(month).bind(self.at)
            .bind(CHART_STOP).bind(handed_off).execute(&mut **tx).await?;
        Ok(())
    }

    pub(super) async fn admit(
        &self,
        pool: &PgPool,
        intent: Uuid,
        month: &str,
        handed_off: bool,
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        self.admission(&mut tx, intent, month, handed_off).await?;
        tx.commit().await
    }

    pub(super) async fn order(&self, pool: &PgPool) {
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,cumulative_quote,fills_qty,last_checked_at,live_seen_at) VALUES ($1,'scope','BTCUSDT','1',$2,'stop','stop','leg',$3,$4,'stop','SELL','STOP_LOSS',1,$5::text::numeric,'NEW','live',0.3,30,0.3,$6,$6)")
            .bind(VENUE).bind(self.tenant).bind(Uuid::now_v7()).bind(self.position).bind(CHART_STOP).bind(self.at)
            .execute(pool).await.unwrap();
    }

    pub(super) async fn all_rows(&self, pool: &PgPool) -> Result<(), sqlx::Error> {
        self.order(pool).await;
        sqlx::query("INSERT INTO venue_accounts VALUES ($1,'scope',$2,'fixture-uid',$3,$4)")
            .bind(VENUE)
            .bind(self.tenant)
            .bind(self.at)
            .bind(Uuid::now_v7())
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO order_fills VALUES ($1,'scope','BTCUSDT','1','1',$2,100,0.3,30,0.03,'USDT',$3,$3)")
            .bind(VENUE).bind(self.tenant).bind(self.at).execute(pool).await.unwrap();
        let attempt = Uuid::now_v7();
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,$2,'scope',$3,'started',$4)")
            .bind(attempt).bind(VENUE).bind(self.intent.to_string()).bind(self.at).execute(&mut *tx).await?;
        // The actual AFTER INSERT trigger creates the charge in this transaction.
        let clock = sqlx::query("INSERT INTO spot_intent_clock (attempt_id,intent_id,tenant_id,venue_id,attempt_created_at) VALUES ($1,$2,$3,$4,$5)")
            .bind(attempt).bind(self.intent).bind(self.tenant).bind(VENUE).bind(self.at).execute(&mut *tx).await;
        if let Err(error) = clock {
            tx.rollback().await?;
            return Err(error);
        }
        tx.commit().await
    }
}

pub(super) fn constraint(error: &sqlx::Error, code: &str, name: Option<&str>) {
    let db = error.as_database_error().expect("database rejection");
    assert_eq!(db.code().as_deref(), Some(code), "{error}");
    if let Some(name) = name {
        assert_eq!(db.constraint(), Some(name), "{error}");
    }
}

/// Use a transaction per refusal, so the rejected statement cannot poison a
/// later assertion.
pub(super) async fn refused(pool: &PgPool, sql: &str, code: &str, name: Option<&str>) {
    let mut tx = pool.begin().await.unwrap();
    let err = sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap_err();
    constraint(&err, code, name);
    tx.rollback().await.unwrap();
}
