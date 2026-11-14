//! ADR-0056 step 8e-1: the additive schema, before any production writer
//! exists. Every database is owned by sqlx::test; DATABASE_URL is supplied by
//! infrastructure.
#![cfg(feature = "postgres")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const VENUE: &str = "binance:spot";
// Synthetic chart fixture: the second support identified by the fixture's
// analysis.
const CHART_STOP: &str = "90";
const TABLES: &[&str] = &[
    "accounting_state",
    "venue_accounts",
    "leg_routes",
    "balance_leg_admissions",
    "balance_legs",
    "venue_orders",
    "order_fills",
    "spot_intent_clock",
];

#[derive(Clone)]
struct Fixture {
    tenant: Uuid,
    position: Uuid,
    intent: Uuid,
    at: DateTime<Utc>,
}

async fn execute(pool: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(pool).await.unwrap();
}

impl Fixture {
    async fn new(pool: &PgPool) -> Self {
        let f = Self {
            tenant: Uuid::now_v7(),
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

    async fn insert_intent(&self, pool: &PgPool, intent: Uuid) {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at) VALUES ($1,$2,$3,'scope',$4,$5,'entry','{}',$6)")
            .bind(intent).bind(self.tenant).bind(VENUE).bind(intent.to_string()).bind(self.position).bind(self.at)
            .execute(pool).await.unwrap();
    }

    async fn admission(
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

    async fn admit(
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

    async fn order(&self, pool: &PgPool) {
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,cumulative_quote,fills_qty,last_checked_at,live_seen_at) VALUES ($1,'scope','BTCUSDT','1',$2,'stop','stop','leg',$3,$4,'stop','SELL','STOP_LOSS',1,$5::text::numeric,'NEW','live',0.3,30,0.3,$6,$6)")
            .bind(VENUE).bind(self.tenant).bind(Uuid::now_v7()).bind(self.position).bind(CHART_STOP).bind(self.at)
            .execute(pool).await.unwrap();
    }

    async fn all_rows(&self, pool: &PgPool) {
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
        sqlx::query("INSERT INTO spot_intent_clock (attempt_id,intent_id,tenant_id,venue_id,attempt_created_at) VALUES ($1,$2,$3,$4,$5)")
            .bind(Uuid::now_v7()).bind(self.intent).bind(self.tenant).bind(VENUE).bind(self.at).execute(pool).await.unwrap();
    }
}

fn constraint(error: &sqlx::Error, code: &str, name: Option<&str>) {
    let db = error.as_database_error().expect("database rejection");
    assert_eq!(db.code().as_deref(), Some(code), "{error}");
    if let Some(name) = name {
        assert_eq!(db.constraint(), Some(name), "{error}");
    }
}

/// Use a transaction per refusal, so the rejected statement cannot poison a
/// later assertion.
async fn refused(pool: &PgPool, sql: &str, code: &str, name: Option<&str>) {
    let mut tx = pool.begin().await.unwrap();
    let err = sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap_err();
    constraint(&err, code, name);
    tx.rollback().await.unwrap();
}

async fn row(pool: &PgPool, table: &str) -> Value {
    sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t LIMIT 1"))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Generated columns are always computed by PostgreSQL, never inserted by the
/// fixture.
async fn insert_row(pool: &PgPool, table: &str, value: Value) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = insert_value(&mut tx, table, value).await;
    // Even a successful probe is rolled back. No inserted fixture can mask a later
    // refusal.
    tx.rollback().await?;
    result
}

async fn insert_value(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    value: Value,
) -> Result<(), sqlx::Error> {
    let columns: String = sqlx::query_scalar("SELECT string_agg(quote_ident(attname), ',' ORDER BY attnum) FROM pg_attribute WHERE attrelid=$1::regclass AND attnum>0 AND NOT attisdropped AND attgenerated=''")
        .bind(table).fetch_one(&mut **tx).await?;
    sqlx::query(&format!("INSERT INTO {table} ({columns}) SELECT {columns} FROM jsonb_populate_record(NULL::{table}, $1)"))
        .bind(value).execute(&mut **tx).await.map(|_| ())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_ledger_schema_installs_fresh(pool: PgPool) {
    for table in TABLES {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table}: migrations are inert");
    }
    let versions: Vec<i64>=sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version>=20240101000036 AND success ORDER BY version")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(versions, [20240101000036, 20240101000037]);
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
    let volatility: String=sqlx::query_scalar("SELECT provolatile::text FROM pg_proc WHERE oid='balance_leg_admit(uuid,text,text,text,uuid,uuid,date,numeric,numeric,numeric,numeric,numeric,text,text,timestamptz,text,numeric,numeric,numeric,numeric,numeric,numeric,timestamptz,boolean)'::regprocedure AND prorettype='void'::regtype")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(volatility, "v");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn no_new_constraint_references_a_table_the_daemon_writes(pool: PgPool) {
    let refs: Vec<String>=sqlx::query_scalar("SELECT c.conname FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid WHERE t.relname=ANY($1) AND c.contype='f' AND c.confrelid NOT IN (SELECT oid FROM pg_class WHERE relname=ANY($1))")
        .bind(TABLES).fetch_all(&pool).await.unwrap();
    assert!(refs.is_empty(), "{refs:?}");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_amount_rejects_nonfinite_and_unrepresentable_values(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
    let version: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut bad = vec![
        "NaN",
        "0.00000000000000000000000000001",
        "79228162514264337593543950336",
    ];
    if version >= 140000 {
        bad.extend(["Infinity", "-Infinity"]);
    }
    let columns=sqlx::query("SELECT c.relname,a.attname FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid WHERE c.relname=ANY($1) AND a.atttypid='numeric'::regtype AND a.attnum>0 ORDER BY 1,2")
        .bind(TABLES).fetch_all(&pool).await.unwrap();
    assert!(columns.len() > 20);
    for col in columns {
        let table: String = col.get(0);
        let column: String = col.get(1);
        for value in &bad {
            let mut data = row(&pool, &table).await;
            data[&column] = json!(value);
            let error = insert_row(&pool, &table, data).await.expect_err("invalid amount accepted");
            constraint(&error, "23514", None);
        }
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_ledger_schema_refuses_what_the_design_forbids(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
    // Every single-column domain; CHECK failures happen before duplicate primary
    // keys.
    let cases = &[
        ("accounting_state", "version", json!(-1)),
        ("accounting_state", "budget_model", json!("other")),
        ("venue_accounts", "uid", json!("")),
        ("balance_leg_admissions", "symbol", json!("")),
        ("balance_leg_admissions", "decided_month", json!("2026-11-02")),
        ("balance_leg_admissions", "fee_asset_policy", json!("BNB")),
        ("balance_leg_admissions", "envelope_source", json!("")),
        ("balance_leg_admissions", "envelope_hash", json!("")),
        ("balance_legs", "cohort_month", json!("2026-12-01")),
        ("balance_legs", "recovery_state", json!("other")),
        ("venue_orders", "exchange_order_id", json!("0123")),
        ("venue_orders", "owner", json!("other")),
        ("venue_orders", "kind", json!("other")),
        ("venue_orders", "side", json!("other")),
        ("venue_orders", "phase", json!("other")),
        ("venue_orders", "trades_cursor", json!(-1)),
        ("venue_orders", "live_clock_verdict", json!("other")),
        ("venue_orders", "terminal_clock_verdict", json!("other")),
        ("order_fills", "trade_id", json!("0123")),
        ("order_fills", "commission_asset", json!("")),
        ("spot_intent_clock", "clock_verdict", json!("other")),
    ];
    for (table, col, value) in cases {
        let mut data = row(&pool, table).await;
        data[col] = value.clone();
        constraint(&insert_row(&pool, table, data).await.unwrap_err(), "23514", None);
    }
    for (table, cols) in [
        ("balance_leg_admissions", vec![
            "taker_bps",
            "gap_bps",
            "notional_margin_bps",
            "min_notional",
            "lot_step",
            "planned_qty",
            "requested_qty",
            "reference_price",
            "remainder_reserve",
            "admission_charge",
            "initial_trigger",
        ]),
        ("balance_legs", vec![
            "target_trigger",
            "gross_qty",
            "acquisition_cost",
            "base_commission",
            "executable_qty",
            "consumed_cum",
        ]),
        ("venue_orders", vec![
            "orig_qty",
            "stop_price",
            "executed_cum",
            "cumulative_quote",
            "fills_qty",
        ]),
        ("order_fills", vec!["price", "qty", "quote_qty", "commission"]),
    ] {
        for col in cols {
            let mut data = row(&pool, table).await;
            data[col] = json!(-1);
            constraint(&insert_row(&pool, table, data).await.unwrap_err(), "23514", None);
        }
    }
    // Upper bounds and multi-column constraints, named to avoid passing on a
    // different refusal.
    for (table, patch, name) in [
        (
            "balance_leg_admissions",
            json!({"taker_bps":10000}),
            "balance_leg_admissions_taker_bps_check",
        ),
        (
            "balance_leg_admissions",
            json!({"gap_bps":10000}),
            "balance_leg_admissions_gap_bps_check",
        ),
        (
            "balance_leg_admissions",
            json!({"notional_margin_bps":10000}),
            "balance_leg_admissions_notional_margin_bps_check",
        ),
        (
            "balance_leg_admissions",
            json!({"requested_qty":0.5}),
            "balance_leg_admissions_check",
        ),
        (
            "balance_leg_admissions",
            json!({"remainder_reserve":2}),
            "balance_leg_admissions_check1",
        ),
        (
            "balance_leg_admissions",
            json!({"superseded_at":"2026-11-15T00:00:00Z"}),
            "balance_leg_admissions_supersession_whole",
        ),
        (
            "balance_leg_admissions",
            json!({"superseded_by":f.intent,"superseded_at":"2026-11-15T00:00:00Z"}),
            "balance_leg_admissions_not_by_itself",
        ),
        ("balance_legs", json!({"gross_qty":1}), "balance_legs_quantities_together"),
        (
            "balance_legs",
            json!({"gross_qty":1,"acquisition_cost":100,"base_commission":0.1,"executable_qty":1}),
            "balance_legs_dust_not_negative",
        ),
        (
            "balance_legs",
            json!({"base_commission_final":true}),
            "balance_legs_final_has_quantities",
        ),
        ("venue_orders", json!({"executed_cum":2}), "venue_orders_executed_within_order"),
        ("venue_orders", json!({"fills_qty":0.4}), "venue_orders_fills_within_executed"),
        ("venue_orders", json!({"kind":"entry"}), "venue_orders_kind_matches_side"),
        ("venue_orders", json!({"stop_price":null}), "venue_orders_stop_has_trigger"),
        (
            "venue_orders",
            json!({"terminal_seen_at":"2026-11-15T00:00:00Z"}),
            "venue_orders_terminal_was_seen",
        ),
        (
            "venue_orders",
            json!({"live_clock_verdict":"valid"}),
            "venue_orders_live_evidence_whole",
        ),
        (
            "venue_orders",
            json!({"phase":"terminal","terminal_seen_at":"2026-11-15T00:00:00Z","terminal_clock_verdict":"valid"}),
            "venue_orders_terminal_evidence_whole",
        ),
        (
            "venue_orders",
            json!({"terminal_clock_wall_delta_ms":1}),
            "venue_orders_terminal_evidence_needs_its_time",
        ),
        (
            "spot_intent_clock",
            json!({"clock_verdict":"valid"}),
            "spot_intent_clock_evidence_whole",
        ),
    ] {
        let mut data = row(&pool, table).await;
        data.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        constraint(&insert_row(&pool, table, data).await.unwrap_err(), "23514", Some(name));
    }
    for present in 0u8..7 {
        let mut data = row(&pool, "venue_orders").await;
        for (bit, field) in ["intent_id", "position_id", "kind"].iter().enumerate() {
            if present & (1 << bit) == 0 {
                data[*field] = Value::Null;
            }
        }
        constraint(
            &insert_row(&pool, "venue_orders", data).await.unwrap_err(),
            "23514",
            Some("venue_orders_ownership_whole"),
        );
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn numbers_and_canonical_ids_round_trip_exactly(pool: PgPool) {
    for value in [
        "0.0000000001",
        "0.0000000000000000000000000001",
        "79228162514264337593543950335",
        "8.0612411",
    ] {
        let fits: bool = sqlx::query_scalar("SELECT ledger_amount_fits($1::text::numeric)")
            .bind(value)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(fits, "{value}");
        let actual: Decimal = sqlx::query_scalar("SELECT $1::text::numeric")
            .bind(value)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, value.parse::<Decimal>().unwrap());
    }
    for (id, expected) in [
        ("0", true),
        ("18446744073709551615", true),
        ("18446744073709551616", false),
        ("0123", false),
        ("+1", false),
        ("-1", false),
        ("1.0", false),
        ("", false),
        ("1e2", false),
        ("١", false),
    ] {
        let actual: bool = sqlx::query_scalar("SELECT ledger_canonical_id($1)")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, expected, "{id}");
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn ledger_rows_only_move_forward(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
    for table in TABLES {
        refused(&pool, &format!("DELETE FROM {table}"), "23514", None).await;
    }
    for table in [
        "venue_accounts",
        "leg_routes",
        "order_fills",
        "spot_intent_clock",
    ] {
        refused(&pool, &format!("UPDATE {table} SET tenant_id=tenant_id"), "23514", None).await;
    }
    for sql in [
        "UPDATE accounting_state SET version=version",
        "UPDATE accounting_state SET version=version-1",
        "UPDATE accounting_state SET version=version+1,venue_id='other'",
        "UPDATE balance_leg_admissions SET admission_charge=13",
        "UPDATE balance_leg_admissions SET entry_intent_id=gen_random_uuid()",
        "UPDATE balance_legs SET tenant_id=gen_random_uuid()",
        "UPDATE balance_legs SET decided_month='2026-12-01'",
        "UPDATE balance_legs SET admission_intent_id=gen_random_uuid()",
        "UPDATE venue_orders SET exchange_order_id='2'",
        "UPDATE venue_orders SET client_order_id='other'",
        "UPDATE venue_orders SET stop_price=89",
        "UPDATE venue_orders SET live_clock_wall_delta_ms=1",
        "UPDATE venue_orders SET live_seen_at=live_seen_at+interval '1 second'",
        "UPDATE venue_orders SET owner='unauthored',intent_id=NULL,position_id=NULL,kind=NULL",
        "UPDATE venue_orders SET executed_cum=0.2",
        "UPDATE venue_orders SET fills_qty=0.2",
        "UPDATE venue_orders SET last_checked_at=last_checked_at-interval '1 second'",
    ] {
        refused(&pool, sql, "23514", None).await;
    }
    execute(&pool, "UPDATE venue_orders SET trades_cursor=2").await;
    refused(&pool, "UPDATE venue_orders SET trades_cursor=1", "23514", None).await;
    execute(&pool,"UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=last_checked_at,terminal_clock_sample_id=gen_random_uuid(),terminal_clock_wall_delta_ms=0,terminal_clock_mono_delta_ms=0,terminal_clock_verdict='valid'").await;
    for field in [
        "terminal_clock_wall_delta_ms",
        "terminal_clock_mono_delta_ms",
    ] {
        refused(&pool, &format!("UPDATE venue_orders SET {field}=1"), "23514", None).await;
    }
    for sql in [
        "UPDATE venue_orders SET phase='live',terminal_seen_at=NULL",
        "UPDATE venue_orders SET status='FILLED'",
        "UPDATE venue_orders SET executed_cum=0.4",
        "UPDATE venue_orders SET terminal_clock_verdict='invalid'",
        "UPDATE venue_orders SET terminal_clock_sample_id=gen_random_uuid()",
    ] {
        refused(&pool, sql, "23514", None).await;
    }
    // Rotation and late fill coverage may advance after terminal; its final
    // evidence cannot.
    execute(&pool,"UPDATE venue_orders SET fills_qty=executed_cum,last_checked_at=last_checked_at+interval '1 second'").await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_owners_future_writes_fit_the_schema(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.order(&pool).await;
    execute(&pool,"UPDATE balance_legs SET gross_qty=1.01,acquisition_cost=101,base_commission=0.01,executable_qty=1").await;
    execute(&pool,"UPDATE balance_legs SET base_commission=0.02,base_commission_final=true,executable_qty=0.98").await;
    for sql in [
        "UPDATE balance_legs SET gross_qty=2",
        "UPDATE balance_legs SET acquisition_cost=102",
        "UPDATE balance_legs SET executable_qty=0.99",
        "UPDATE balance_legs SET base_commission=0.01",
        "UPDATE balance_legs SET base_commission_final=false",
    ] {
        refused(&pool, sql, "23514", None).await;
    }
    execute(&pool, "UPDATE balance_legs SET consumed_cum=1,recovery_state='unresolved'").await;
    let deficit: bool = sqlx::query_scalar("SELECT consumed_cum>executable_qty FROM balance_legs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(deficit);
    refused(&pool, "UPDATE balance_legs SET consumed_cum=0.9", "23514", None).await;
    execute(&pool, "UPDATE balance_legs SET cohort_month=decided_month,closed_at=now()").await;
    refused(&pool, "UPDATE balance_legs SET cohort_month=NULL", "23514", None).await;
    refused(&pool, "UPDATE balance_legs SET closed_at=NULL", "23514", None).await;
    let derived: (bool, bool) = sqlx::query_as("SELECT terminal,fills_complete FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(derived, (false, true));
    execute(&pool, "UPDATE venue_orders SET executed_cum=0.5").await;
    let complete: bool = sqlx::query_scalar("SELECT fills_complete FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!complete);
    execute(&pool,"UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=now(),fills_qty=0.5").await;
    let derived: (bool, bool) = sqlx::query_as("SELECT terminal,fills_complete FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(derived, (true, true));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_unowned_order_is_adopted_once_and_nontrigger_placeholders_are_null(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at) VALUES ($1,'scope','BTCUSDT','1',$2,'external','external','discovered','SELL','MARKET',1,'NEW','live',$3,$3)")
        .bind(VENUE).bind(f.tenant).bind(f.at).execute(&pool).await.unwrap();
    let stop: Option<Decimal> = sqlx::query_scalar("SELECT stop_price FROM venue_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stop, None);
    refused(&pool, "UPDATE venue_orders SET owner='unauthored'", "23514", None).await;
    sqlx::query("UPDATE venue_orders SET owner='leg',intent_id=$1,position_id=$2,kind='sell'")
        .bind(Uuid::now_v7())
        .bind(f.position)
        .execute(&pool)
        .await
        .unwrap();
    refused(&pool, "UPDATE venue_orders SET kind='stop',stop_price=90", "23514", None).await;
    // Raw zero is forbidden at storage; 8e-4's writer must canonicalize before
    // insertion AND comparison.
    let mut data = row(&pool, "venue_orders").await;
    data["stop_price"] = json!(0);
    constraint(
        &insert_row(&pool, "venue_orders", data).await.unwrap_err(),
        "23514",
        Some("venue_orders_stop_price_check"),
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_leg_admit_inserts_replays_and_supersedes_only_proven_unsent(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.admit(&pool, f.intent, "2026-11-01", false).await.unwrap();
    let second = Uuid::now_v7();
    f.insert_intent(&pool, second).await;
    for state in ["not_sent=false","not_sent=true,uncertain_at=now()","not_sent=true,venue_outcome='success'","not_sent=true,venue_outcome='rejected'","not_sent=true,disposition_kind='accepted_risk',disposition_treatment='unbooked_accepted',risk_accepted_at=now(),risk_accepted_by='fixture',risk_accepted_auth='asserted'"] {
        // Each negative case rolls back its deliberately contradictory journal evidence too.
        let mut tx=pool.begin().await.unwrap();
        sqlx::query(&format!("UPDATE order_intents SET {state} WHERE intent_id=$1")).bind(f.intent).execute(&mut *tx).await.unwrap();
        let error=f.admission(&mut tx,second,"2026-12-01",false).await.unwrap_err();constraint(&error,"23514",None);
        tx.rollback().await.unwrap();
    }
    sqlx::query("UPDATE order_intents SET not_sent=true WHERE intent_id=$1")
        .bind(f.intent)
        .execute(&pool)
        .await
        .unwrap();
    f.admit(&pool, second, "2026-12-01", false).await.unwrap();
    let current: (Uuid, String) =
        sqlx::query_as("SELECT admission_intent_id,decided_month::text FROM balance_legs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, (second, "2026-12-01".into()));
    let old:(String,Uuid)=sqlx::query_as("SELECT decided_month::text,superseded_by FROM balance_leg_admissions WHERE entry_intent_id=$1").bind(f.intent).fetch_one(&pool).await.unwrap();
    assert_eq!(old, ("2026-11-01".into(), second));
    // A historical replay cannot move the pointer or set the successor's cohort.
    f.admit(&pool, f.intent, "2026-11-01", true).await.unwrap();
    let cohort: Option<String> = sqlx::query_scalar("SELECT cohort_month::text FROM balance_legs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(cohort.is_none());
    let third = Uuid::now_v7();
    f.insert_intent(&pool, third).await;
    sqlx::query("UPDATE order_intents SET not_sent=true WHERE intent_id=$1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    f.admit(&pool, third, "2027-01-01", true).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM balance_leg_admissions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let cohort: String = sqlx::query_scalar("SELECT cohort_month::text FROM balance_legs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cohort, "2027-01-01");
    let error = f.admit(&pool, third, "2027-02-01", false).await.unwrap_err();
    constraint(&error, "23514", None);
    refused(
        &pool,
        "UPDATE balance_leg_admissions SET superseded_at=now() WHERE superseded_at IS NOT NULL",
        "23514",
        None,
    )
    .await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn tenant_relationships_and_trade_identity_cannot_be_crossed(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,$2,'net_from_start_non_expanding_v1')")
        .bind(other).bind(VENUE).execute(&pool).await.unwrap();
    for table in [
        "balance_leg_admissions",
        "balance_legs",
        "venue_orders",
        "order_fills",
    ] {
        let mut data = row(&pool, table).await;
        data["tenant_id"] = json!(other);
        match table {
            "balance_leg_admissions" => {
                data["entry_intent_id"] = json!(Uuid::now_v7());
                // A historical admission avoids the separate current-admission uniqueness
                // check.
                data["superseded_at"] = json!("2026-11-15T00:00:00Z");
                data["superseded_by"] = json!(f.intent);
            },
            "balance_legs" => {
                data["position_id"] = json!(Uuid::now_v7());
                data["admission_intent_id"] = json!(Uuid::now_v7());
            },
            "venue_orders" => {
                data["exchange_order_id"] = json!("2");
                data["intent_id"] = json!(Uuid::now_v7());
            },
            "order_fills" => {
                data["trade_id"] = json!("2");
            },
            _ => unreachable!(),
        }
        constraint(&insert_row(&pool, table, data).await.unwrap_err(), "23503", None);
    }
    // Same economic trade cannot be claimed by another, otherwise valid order.
    let mut order = row(&pool, "venue_orders").await;
    order["exchange_order_id"] = json!("2");
    order["intent_id"] = json!(Uuid::now_v7());
    let cols:String=sqlx::query_scalar("SELECT string_agg(quote_ident(attname),',' ORDER BY attnum) FROM pg_attribute WHERE attrelid='venue_orders'::regclass AND attnum>0 AND NOT attisdropped AND attgenerated=''").fetch_one(&pool).await.unwrap();
    sqlx::query(&format!("INSERT INTO venue_orders ({cols}) SELECT {cols} FROM jsonb_populate_record(NULL::venue_orders,$1)")).bind(order).execute(&pool).await.unwrap();
    let mut trade = row(&pool, "order_fills").await;
    trade["exchange_order_id"] = json!("2");
    constraint(
        &insert_row(&pool, "order_fills", trade).await.unwrap_err(),
        "23505",
        Some("order_fills_pkey"),
    );
    // Every table's primary key rejects an identical insert.
    for table in TABLES {
        constraint(
            &insert_row(&pool, table, row(&pool, table).await).await.unwrap_err(),
            "23505",
            None,
        );
    }
    let mut second_intent = row(&pool, "venue_orders").await;
    second_intent["exchange_order_id"] = json!("3");
    constraint(
        &insert_row(&pool, "venue_orders", second_intent).await.unwrap_err(),
        "23505",
        Some("idx_venue_orders_intent"),
    );
    let mut second_admission = row(&pool, "balance_leg_admissions").await;
    second_admission["entry_intent_id"] = json!(Uuid::now_v7());
    constraint(
        &insert_row(&pool, "balance_leg_admissions", second_admission).await.unwrap_err(),
        "23505",
        Some("idx_balance_leg_admissions_current"),
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn supersession_is_refused_after_any_inventory_or_handoff_evidence(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    sqlx::query("UPDATE order_intents SET not_sent=true WHERE intent_id=$1")
        .bind(f.intent)
        .execute(&pool)
        .await
        .unwrap();
    let second = Uuid::now_v7();
    f.insert_intent(&pool, second).await;
    for evidence in [
        "UPDATE balance_legs SET cohort_month=decided_month",
        "UPDATE balance_legs SET gross_qty=0,acquisition_cost=0,base_commission=0,executable_qty=0",
        "UPDATE balance_legs SET consumed_cum=0.1",
    ] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::raw_sql(evidence).execute(&mut *tx).await.unwrap();
        constraint(
            &f.admission(&mut tx, second, "2026-12-01", false).await.unwrap_err(),
            "23514",
            None,
        );
        tx.rollback().await.unwrap();
    }
    f.order(&pool).await;
    constraint(&f.admit(&pool, second, "2026-12-01", false).await.unwrap_err(), "23514", None);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM balance_leg_admissions),version FROM accounting_state",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1), "refused supersession rolls its accounting bump back");
}

async fn legacy_intents(pool: &PgPool, ids: &[&str]) {
    for id in ids {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,'binance:usdm','scope',$3,'exit','{}',now())")
            .bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(id).execute(pool).await.unwrap();
    }
}

/// sqlx still owns this database. Only automatic migration is disabled so the
/// test can exercise the real upgrade boundary instead of an already-upgraded
/// schema.
fn through_35() -> sqlx::migrate::Migrator {
    let mut old = sqlx::migrate!("../migrations");
    old.migrations = std::borrow::Cow::Owned(
        old.iter().filter(|m| m.version <= 20240101000035).cloned().collect(),
    );
    old
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn the_ledger_schema_upgrades_from_35_and_preserves_the_rollback_boundary(pool: PgPool) {
    let old = through_35();
    old.run(&pool).await.unwrap();
    legacy_intents(&pool, &["legacy"]).await;

    sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,'binance:usdm','scope','legacy','started',now())")
        .bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    let before: Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t)) FROM submit_attempts t")
        .fetch_one(&pool)
        .await
        .unwrap();
    robson_db::migrate(&pool).await.unwrap();
    let after: Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t)) FROM submit_attempts t")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert!(matches!(
        old.run(&pool).await,
        Err(sqlx::migrate::MigrateError::VersionMissing(20240101000036))
            | Err(sqlx::migrate::MigrateError::VersionMissing(20240101000037))
    ));
    // An artifact embedding the applied files validates their unchanged checksums.
    robson_db::migrate(&pool).await.unwrap();
    for table in TABLES {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let f = Fixture::new(&pool).await;
    f.all_rows(&pool).await;
}

#[sqlx::test(migrations = false)]
#[ignore = "requires DATABASE_URL"]
async fn old_pod_writes_remain_compatible_during_36_and_37(pool: PgPool) {
    use std::time::Duration;
    through_35().run(&pool).await.unwrap();
    legacy_intents(&pool, &["predecessor", "during-migration"]).await;

    // Hold a pre-existing writer: concurrent index creation must wait for it,
    // without excluding ordinary DML from other journal transactions.
    let mut predecessor = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,'binance:usdm','scope','predecessor','started',now())")
        .bind(Uuid::now_v7()).execute(&mut *predecessor).await.unwrap();
    let migration_pool = pool.clone();
    let migrating = tokio::spawn(async move { robson_db::migrate(&migration_pool).await });
    tokio::time::timeout(Duration::from_secs(10),async {
        loop {
            let lock:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='submit_attempts'::regclass AND mode='ShareUpdateExclusiveLock' AND granted)")
                .fetch_one(&pool).await.unwrap();
            if lock {break;}
            assert!(!migrating.is_finished(),"migration finished without the concurrent-build lock");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("concurrent build reaches its writer wait");
    // 36 already committed. Its additive objects do not alter the old writer's
    // schema.
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('venue_orders') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(exists);
    tokio::time::timeout(Duration::from_millis(500),async {
        let mut old_writer=pool.begin().await.unwrap();
        sqlx::query("INSERT INTO submit_attempts (attempt_id,venue_id,account_scope,client_order_id,state,created_at) VALUES ($1,'binance:usdm','scope','during-migration','started',now())")
            .bind(Uuid::now_v7()).execute(&mut *old_writer).await.unwrap();
        old_writer.commit().await.unwrap();
    }).await.expect("migration must not block ordinary journal DML");
    predecessor.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), migrating)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM submit_attempts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);
    for table in TABLES {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_frozen_order_and_leg_field_rejects_rewriting(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    f.order(&pool).await;
    execute(&pool,"UPDATE balance_legs SET gross_qty=1.01,acquisition_cost=101,base_commission=0.01,base_commission_final=true,executable_qty=1,cohort_month=decided_month,closed_at=now()").await;
    execute(&pool,"UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=now(),terminal_clock_sample_id=gen_random_uuid(),terminal_clock_wall_delta_ms=0,terminal_clock_mono_delta_ms=0,terminal_clock_verdict='valid'").await;
    for (table, fields) in [
        ("accounting_state", vec!["tenant_id", "venue_id", "created_at"]),
        ("balance_legs", vec![
            "position_id",
            "tenant_id",
            "venue_id",
            "account_scope",
            "symbol",
            "created_at",
            "gross_qty",
            "acquisition_cost",
            "base_commission",
            "cohort_month",
            "closed_at",
        ]),
        ("venue_orders", vec![
            "venue_id",
            "account_scope",
            "symbol",
            "exchange_order_id",
            "tenant_id",
            "client_order_id",
            "side",
            "order_type",
            "orig_qty",
            "stop_price",
            "live_seen_at",
            "live_clock_sample_id",
            "live_clock_wall_delta_ms",
            "live_clock_mono_delta_ms",
            "live_clock_verdict",
            "intent_id",
            "position_id",
            "kind",
            "phase",
            "status",
            "executed_cum",
            "cumulative_quote",
            "expiry_reason",
            "terminal_seen_at",
            "terminal_clock_sample_id",
            "terminal_clock_wall_delta_ms",
            "terminal_clock_mono_delta_ms",
            "terminal_clock_verdict",
        ]),
    ] {
        for field in fields {
            let ty:String=sqlx::query_scalar("SELECT t.typname FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid WHERE a.attrelid=$1::regclass AND a.attname=$2")
                .bind(table).bind(field).fetch_one(&pool).await.unwrap();
            let expr = match ty.as_str() {
                "uuid" => "gen_random_uuid()".into(),
                "timestamptz" => format!("COALESCE({field},now())+interval '1 second'"),
                "date" => format!("{field}+1"),
                "numeric" | "int8" => format!("COALESCE({field},0)+1"),
                "text" => format!("COALESCE({field},'')||'-changed'"),
                other => panic!("unexpected fixture type {other}"),
            };
            let mut tx = pool.begin().await.unwrap();
            let version = if table == "accounting_state" {
                ",version=version+1"
            } else {
                ""
            };
            let error = sqlx::query(&format!("UPDATE {table} SET {field}={expr}{version}"))
                .execute(&mut *tx)
                .await
                .unwrap_err();
            constraint(&error, "23514", None);
            assert!(
                error.as_database_error().unwrap().message().starts_with("ledger invariant:"),
                "{table}.{field}: {error}"
            );
            tx.rollback().await.unwrap();
        }
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn supersession_cannot_rewrite_any_admission_term(pool: PgPool) {
    let _f = Fixture::new(&pool).await;
    let fields:Vec<(String,String)>=sqlx::query_as("SELECT a.attname::text,t.typname::text FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid WHERE a.attrelid='balance_leg_admissions'::regclass AND a.attnum>0 AND NOT a.attisdropped AND a.attname NOT IN ('superseded_at','superseded_by') ORDER BY a.attnum")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(fields.len(), 23);
    for (field, ty) in fields {
        let expr = match ty.as_str() {
            "uuid" => "gen_random_uuid()".into(),
            "timestamptz" => format!("{field}+interval '1 second'"),
            "date" => format!("{field}+interval '1 month'"),
            "numeric" => format!("{field}+1"),
            "text" => format!("{field}||'-changed'"),
            other => panic!("unexpected fixture type {other}"),
        };
        let mut tx = pool.begin().await.unwrap();
        // Supplying the permitted supersession mark ensures the frozen term,
        // rather than a missing mark, is what the trigger refuses.
        let error=sqlx::query(&format!("UPDATE balance_leg_admissions SET superseded_at=now(),superseded_by=gen_random_uuid(),{field}={expr}"))
            .execute(&mut *tx).await.unwrap_err();
        constraint(&error, "23514", None);
        assert!(
            error.as_database_error().unwrap().message().starts_with("ledger invariant:"),
            "{field}: {error}"
        );
        tx.rollback().await.unwrap();
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn admission_and_leg_keys_require_the_exact_route_and_month(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let position = Uuid::now_v7();
    let intent = Uuid::now_v7();
    let mut admission = row(&pool, "balance_leg_admissions").await;
    admission["position_id"] = json!(position);
    admission["entry_intent_id"] = json!(intent);
    constraint(
        &insert_row(&pool, "balance_leg_admissions", admission.clone())
            .await
            .unwrap_err(),
        "23503",
        Some("balance_leg_admissions_route"),
    );
    let mut leg = row(&pool, "balance_legs").await;
    leg["position_id"] = json!(position);
    leg["admission_intent_id"] = json!(intent);
    leg["decided_month"] = json!("2026-12-01");
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,$3,'scope',now())")
        .bind(position)
        .bind(f.tenant)
        .bind(VENUE)
        .execute(&mut *tx)
        .await
        .unwrap();
    insert_value(&mut tx, "balance_leg_admissions", admission).await.unwrap();
    constraint(
        &insert_value(&mut tx, "balance_legs", leg).await.unwrap_err(),
        "23503",
        Some("balance_legs_admission"),
    );
    tx.rollback().await.unwrap();
}
