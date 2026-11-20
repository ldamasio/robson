//! Actual global nonempty refusal in a sqlx-owned fresh database. The source
//! row is deliberately another tenant/venue; no filtering can make it empty.
use robson_exec::venue_clock::NativeClock;
use sqlx::PgPool;
use tokio::time::{Duration, Instant};
use uuid::Uuid;

use super::*;

#[test]
fn production_context_stays_unavailable() {
    assert!(matches!(bootstrap::production_input(), Err(BudgetError::PrerequisiteRefusal)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn isolated_fixture_refuses_global_nonspot_source(pool: PgPool) {
    let clock = NativeClock::initialize().unwrap();
    let tenant = Uuid::now_v7();
    let _committed = crate::process_epoch::start_epoch(&pool, tenant, "I1-refusal-test")
        .await
        .unwrap();
    sqlx::query("INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
        VALUES($1,$2,'test:other-venue','operated','global-source','exit','{}',NOW())")
        .bind(Uuid::now_v7()).bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    let trace = Trace::default();
    assert!(matches!(
        fixture::prepare(&pool, &clock, Instant::now() + Duration::from_secs(2), &trace).await,
        Err(BudgetError::Invalid)
    ));
    assert_eq!(trace.issued(), 4, "refusal before audit/state writes and COMMIT");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM budget_source_audits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "test observer is outside bootstrap application-call count");
}
