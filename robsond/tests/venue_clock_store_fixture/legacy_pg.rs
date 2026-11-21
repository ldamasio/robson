//! Relocated clock regressions use the real private backend, never a public
//! fixture setter. All supplied coordinates remain unqualified test data.
use robson_exec::{
    clock_records::{RawClockCapture, RawClockSample},
    venue::{OrderPhase, VenueId},
    venue_clock::ClockDomain,
    venue_ledger::LegOrderKind,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::ClockSampleStore;
#[allow(
    dead_code,
    reason = "shared integration fixture keeps its other cases intact"
)]
#[path = "../venue_ledger_orders/fixture.rs"]
mod order_fixture;
#[allow(
    dead_code,
    reason = "shared integration fixture keeps its other cases intact"
)]
#[path = "../venue_ledger_schema/fixture.rs"]
mod schema_fixture;
use order_fixture::{record, seen, Fixture as OrderFixture};
use schema_fixture::{constraint, execute, refused, Fixture as SchemaFixture};

// All I/O is fixture setup, before ledger-operation measurement baselines.
// Diagnostic samples deliberately fail numerical bounds and never grant C8.
async fn diagnostic_samples(pool: &PgPool, tenant: Uuid) -> [Uuid; 2] {
    let start = crate::process_epoch::start_epoch(pool, tenant, "clock-fixture").await.unwrap();
    let mut store = ClockSampleStore::from_committed_epoch(pool, tenant, &start);
    let mark = RawClockCapture {
        epoch: start.epoch.epoch_id,
        wall_seconds: 1_794_614_400,
        wall_nanoseconds: 0,
        domain: ClockDomain::new(Uuid::now_v7(), 0, 0).unwrap(),
        before_ns: 0,
        after_ns: 0,
    };
    let ids = [Uuid::now_v7(), Uuid::now_v7()];
    for (index, id) in ids.iter().copied().enumerate() {
        let raw = RawClockSample {
            id,
            tenant,
            venue: VenueId::new(VenueId::BINANCE_SPOT),
            send: mark,
            receive: mark,
            server_ms: 0,
        };
        assert_ne!(raw.numbers().unwrap().failures, 0);
        store.persist(index as u64 + 1, raw, None).await.unwrap();
    }
    ids
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn first_clock_evidence_is_retained_even_when_later_audit_changes(pool: PgPool) {
    use robson_exec::venue_ledger::{ClockEvidence, ClockVerdict};
    let tenant = Uuid::now_v7();
    let [first_id, terminal_id] = diagnostic_samples(&pool, tenant).await;
    let f = OrderFixture::for_tenant(&pool, tenant).await;
    let link = f.link(LegOrderKind::Sell);
    let mut r = record();
    let mut first = seen(1);
    first.clock = Some(ClockEvidence {
        sample_id: first_id,
        wall_delta_ms: 5,
        mono_delta_ms: 6,
        verdict: ClockVerdict::Valid,
    });
    f.ledger.record_order(&r, &link, first.clone()).await.unwrap();
    r.phase = OrderPhase::Terminal;
    r.status = "CANCELED".into();
    let mut terminal = seen(2);
    terminal.clock = Some(ClockEvidence {
        sample_id: terminal_id,
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

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_clock_suffix_only_moves_forward(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let [initial_sample, alternate_sample] = diagnostic_samples(&pool, tenant).await;
    let f = SchemaFixture::for_tenant(&pool, tenant).await;
    f.order(&pool).await;
    execute(&pool, "UPDATE venue_orders SET trades_cursor=2").await;
    sqlx::query("UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=last_checked_at,terminal_clock_sample_id=$1,terminal_clock_wall_delta_ms=0,terminal_clock_mono_delta_ms=0,terminal_clock_verdict='valid'").bind(initial_sample).execute(&pool).await.unwrap();
    for field in [
        "terminal_clock_wall_delta_ms",
        "terminal_clock_mono_delta_ms",
    ] {
        refused(&pool, &format!("UPDATE venue_orders SET {field}=1"), "23514", None).await;
    }
    let rewrite =
        format!("UPDATE venue_orders SET terminal_clock_sample_id='{alternate_sample}'::uuid");
    for sql in [
        "UPDATE venue_orders SET phase='live',terminal_seen_at=NULL",
        "UPDATE venue_orders SET status='FILLED'",
        "UPDATE venue_orders SET executed_cum=0.4",
        "UPDATE venue_orders SET terminal_clock_verdict='invalid'",
        rewrite.as_str(),
    ] {
        refused(&pool, sql, "23514", None).await;
    }
    // Rotation and late fill coverage may advance after terminal; its final
    // evidence cannot.
    execute(&pool,"UPDATE venue_orders SET fills_qty=executed_cum,last_checked_at=last_checked_at+interval '1 second'").await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_frozen_order_and_leg_field_rejects_rewriting(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let [initial_sample, alternate_sample] = diagnostic_samples(&pool, tenant).await;
    let f = SchemaFixture::for_tenant(&pool, tenant).await;
    f.order(&pool).await;
    execute(&pool,"UPDATE balance_legs SET gross_qty=1.01,acquisition_cost=101,base_commission=0.01,base_commission_final=true,executable_qty=1,cohort_month=decided_month,closed_at=now()").await;
    sqlx::query("UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=now(),terminal_clock_sample_id=$1,terminal_clock_wall_delta_ms=0,terminal_clock_mono_delta_ms=0,terminal_clock_verdict='valid'").bind(initial_sample).execute(&pool).await.unwrap();
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
                "uuid"
                    if table == "venue_orders"
                        && matches!(field, "live_clock_sample_id" | "terminal_clock_sample_id") =>
                {
                    format!("'{alternate_sample}'::uuid")
                },
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
