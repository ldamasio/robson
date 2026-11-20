//! C1 calibration for the balance-account session bounds (PLAN r4 §6). Not a
//! gate: ignored, run on demand against a disposable PostgreSQL 16.

#![cfg(feature = "postgres")]

#[tokio::test]
#[ignore = "calibration probe; requires DATABASE_URL"]
async fn c1_schema_probe() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DROP DATABASE IF EXISTS robson_c1_probe")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("CREATE DATABASE robson_c1_probe").execute(&admin).await.unwrap();
    let probe_url = format!("{}/robson_c1_probe", url.rsplit_once('/').unwrap().0);
    let pool = sqlx::PgPool::connect(&probe_url).await.unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();
    for table in [
        "event_log",
        "stream_state",
        "positions_current",
        "stream_suspensions",
        "monthly_state",
        "projection_checkpoints",
        "order_intents",
        "income_ledger",
        "queries_current",
        "funding_sagas",
        "process_epochs",
        "recovery_incidents",
        "snapshots",
    ] {
        let columns: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT column_name::text, data_type::text, is_nullable::text, column_default::text \
             FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 \
             ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        let checks: Vec<(String, String)> = sqlx::query_as(
            "SELECT conname::text, pg_get_constraintdef(oid) FROM pg_constraint \
             WHERE conrelid = $1::regclass ORDER BY conname",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        let indexes: Vec<(String,)> = sqlx::query_as(
            "SELECT indexdef FROM pg_indexes WHERE schemaname = 'public' AND tablename = $1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        let triggers: Vec<(String,)> = sqlx::query_as(
            "SELECT tgname::text FROM pg_trigger WHERE tgrelid = $1::regclass AND NOT tgisinternal",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .unwrap();
        eprintln!("C1_PROBE_TABLE {table}");
        for c in columns {
            eprintln!("C1_PROBE_COL {table} {c:?}");
        }
        for c in checks {
            eprintln!("C1_PROBE_CON {table} {c:?}");
        }
        for i in indexes {
            eprintln!("C1_PROBE_IDX {table} {}", i.0);
        }
        for t in triggers {
            eprintln!("C1_PROBE_TRG {table} {}", t.0);
        }
    }
    let partitions: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
         JOIN pg_class p ON p.oid = i.inhparent WHERE p.relname IN ('event_log','snapshots') ORDER BY 1",
    )
    .fetch_all(&pool).await.unwrap();
    eprintln!("C1_PROBE_PARTITIONS {partitions:?}");
    pool.close().await;
    sqlx::query("DROP DATABASE robson_c1_probe WITH (FORCE)")
        .execute(&admin)
        .await
        .unwrap();
}
