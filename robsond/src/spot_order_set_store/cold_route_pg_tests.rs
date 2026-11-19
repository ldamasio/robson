//! Native route-cursor, current-leg and restore-audit constituent evidence.
//! The candidate index exists only inside these disposable databases.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    pg_tests::bounded_leaf, read_route_page, RouteClass, RouteDefect, ROUTES_FIRST, ROUTES_NEXT,
};

const INDEX: &str = "idx_leg_routes_spot_scope_position";
const PRIMARY: &str = "cold-spot-routes";
const DEEP: &str = "cold-spot-routes-deep";
const NOISE: i64 = 8192;
const DEEP_BASE: i64 = 100_000;

fn position(n: i64) -> Uuid {
    Uuid::from_u128(n.try_into().expect("fixture positive position"))
}

async fn setup(pool: &PgPool, tenant: Uuid, foreign: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.accounting_state (tenant_id,venue_id,budget_model)
         SELECT tenant,'binance:spot','hwm_v1' FROM unnest($1::uuid[]) tenant",
    )
    .bind(vec![tenant, foreign])
    .execute(pool)
    .await?;
    sqlx::raw_sql(
        "CREATE INDEX idx_leg_routes_spot_scope_position
         ON public.leg_routes (account_scope,position_id) WHERE venue_id='binance:spot'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn routes(
    pool: &PgPool,
    scope: &str,
    tenant: Uuid,
    foreign: Uuid,
    start: i64,
    size: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.leg_routes (position_id,tenant_id,venue_id,account_scope,routed_at)
         SELECT lpad(to_hex(i+$4::bigint),32,'0')::uuid,
                CASE WHEN i=$5::bigint-1 THEN $3 ELSE $2 END,'binance:spot',$1,now()
           FROM generate_series(1,$5::bigint) i ORDER BY (i*29)%($5*3+1)",
    )
    .bind(scope)
    .bind(tenant)
    .bind(foreign)
    .bind(start)
    .bind(size)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_admissions(pool: &PgPool, scope: &str, except: &[Uuid]) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.balance_leg_admissions (
             entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,
             taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,
             envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,
             reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
         SELECT md5('entry'||r.position_id::text)::uuid,
                r.position_id,r.tenant_id,r.venue_id,r.account_scope,'BTCUSDT','2026-11-01',
                10,10,2,10,0.001,'base','fixture',now(),'fixture',1,1,100,0.1,10,90,now()
           FROM public.leg_routes r WHERE r.account_scope=$1 AND r.position_id <> ALL($2::uuid[])",
    )
    .bind(scope)
    .bind(except)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_legs(pool: &PgPool, scope: &str, except: &[Uuid], closed: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.balance_legs (
            position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,
            decided_month,target_trigger,created_at,closed_at)
         SELECT a.position_id,a.tenant_id,a.venue_id,a.account_scope,a.symbol,a.entry_intent_id,
                a.decided_month,a.initial_trigger,now(),CASE WHEN a.position_id=$3 THEN now() END
         FROM public.balance_leg_admissions a WHERE a.account_scope=$1 AND a.position_id <> ALL($2::uuid[])",
    )
    .bind(scope).bind(except).bind(closed).execute(pool).await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_cold_route_native_pages_preserve_invalid_and_armed_rows(
    pool: PgPool,
) -> Result<()> {
    let tenant = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    setup(&pool, tenant, foreign).await?;
    routes(&pool, PRIMARY, tenant, foreign, 0, 10).await?;
    seed_admissions(&pool, PRIMARY, &[position(1), position(10)]).await?;
    seed_legs(&pool, PRIMARY, &[position(5)], position(2)).await?;

    // Keep the leg's original valid FK, but supersede its admission while a
    // different admission becomes current. Neither independent probe may hide it.
    let mut corrupt = pool.begin().await?;
    let successor = Uuid::now_v7();
    sqlx::query(
        "UPDATE public.balance_leg_admissions SET superseded_at=now(), superseded_by=$2
         WHERE position_id=$1",
    )
    .bind(position(6))
    .bind(successor)
    .execute(&mut *corrupt)
    .await?;
    sqlx::query(
        "INSERT INTO public.balance_leg_admissions (
             entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,
             taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,
             envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,
             reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
         SELECT $2,a.position_id,a.tenant_id,a.venue_id,a.account_scope,a.symbol,a.decided_month,
                a.taker_bps,a.gap_bps,a.notional_margin_bps,a.min_notional,a.lot_step,a.fee_asset_policy,
                a.envelope_source,a.envelope_as_of,a.envelope_hash,a.planned_qty,a.requested_qty,
                a.reference_price,a.remainder_reserve,a.admission_charge,a.initial_trigger,now()
           FROM public.balance_leg_admissions a WHERE a.position_id=$1",
    )
    .bind(position(6)).bind(successor).execute(&mut *corrupt).await?;
    corrupt.commit().await?;

    let mut conn = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    let first = read_route_page(&mut conn, PRIMARY, tenant, None).await?;
    ensure!(first.full && first.rows.len() == 8);
    ensure!(first.rows[0].classify() == RouteClass::ArmedOnly && first.rows[0].eligible());
    ensure!(matches!(first.rows[1].classify(), RouteClass::Leg { closed: true }));
    ensure!(first.rows[4].classify() == RouteClass::Diagnostic(RouteDefect::AdmissionWithoutLeg));
    ensure!(first.rows[5].classify() == RouteClass::Diagnostic(RouteDefect::StaleLegPointer));
    ensure!(first.rows[5].admission_intent_id == Some(successor));
    ensure!(first.rows[5].leg_admission_intent_id != Some(successor));
    ensure!(!first.rows[4].eligible() && !first.rows[5].eligible());
    let last = first.rows.last().context("full route page")?.position_id;
    ensure!(last == position(8));

    // A real native admission drawn after the first read must be absent from
    // both counterpart probes in this independent read-only snapshot.
    sqlx::query(
        "SELECT balance_leg_admit($1,'binance:spot',$2,'BTCUSDT',$3,$4,'2026-11-01',
            10,10,2,10,0.001,'base','fixture',now(),'fixture',1,1,100,0.1,10,90,now(),false)",
    )
    .bind(tenant)
    .bind(PRIMARY)
    .bind(position(10))
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await?;
    let second = read_route_page(&mut conn, PRIMARY, tenant, Some(last)).await?;
    ensure!(!second.full && second.rows.len() == 2);
    ensure!(second.rows[0].position_id == position(9) && second.rows[0].tenant_id == foreign);
    ensure!(second.rows[0].classify() == RouteClass::Diagnostic(RouteDefect::ForeignOwner));
    ensure!(
        second.rows[1].position_id == position(10)
            && second.rows[1].classify() == RouteClass::ArmedOnly
    );
    let exact_tail = read_route_page(&mut conn, PRIMARY, tenant, Some(position(2))).await?;
    ensure!(
        exact_tail.full
            && exact_tail.rows.last().context("exact full tail")?.position_id == position(10)
    );
    let end = read_route_page(&mut conn, PRIMARY, tenant, Some(position(10))).await?;
    ensure!(!end.full && end.rows.is_empty());
    conn.rollback().await?;

    let mut fresh = pool.acquire().await?;
    let row10 = read_route_page(&mut fresh, PRIMARY, tenant, Some(position(9))).await?;
    ensure!(
        row10.rows.len() == 1
            && matches!(row10.rows[0].classify(), RouteClass::Leg { closed: false })
    );
    ensure!(row10.rows[0].leg_admission_intent_id == row10.rows[0].admission_intent_id);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_cold_route_scans_eight_routes_and_two_point_probes_under_skew(
    pool: PgPool,
) -> Result<()> {
    let tenant = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    setup(&pool, tenant, foreign).await?;
    for (scope, base, size) in [
        (PRIMARY, 0, 10),
        (DEEP, DEEP_BASE, NOISE + 10),
        ("other-account", 2 * DEEP_BASE, NOISE),
    ] {
        routes(&pool, scope, tenant, foreign, base, size).await?;
        seed_admissions(&pool, scope, &[]).await?;
        seed_legs(&pool, scope, &[], Uuid::nil()).await?;
    }
    sqlx::raw_sql("ANALYZE public.leg_routes; ANALYZE public.balance_legs; ANALYZE public.balance_leg_admissions")
        .execute(&pool).await?;
    let mut conn = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    for (scope, base, cursor) in [
        (PRIMARY, 0, None),
        (DEEP, DEEP_BASE + NOISE, Some(position(DEEP_BASE + NOISE))),
    ] {
        let first = read_route_page(&mut conn, scope, tenant, cursor).await?;
        ensure!(first.full);
        ensure!(first.rows.iter().all(|row| row.eligible()));
        let last = first.rows.last().context("full source page")?.position_id;
        ensure!(last == position(base + 8));
        let second = read_route_page(&mut conn, scope, tenant, Some(last)).await?;
        ensure!(!second.full && second.rows.len() == 2);
        ensure!(
            second.rows[0].position_id == position(base + 9) && second.rows[0].tenant_id == foreign
        );
        ensure!(second.rows[0].classify() == RouteClass::Diagnostic(RouteDefect::ForeignOwner));
        ensure!(second.rows[1].position_id == position(base + 10) && second.rows[1].eligible());

        let raw = if cursor.is_some() {
            ROUTES_NEXT
        } else {
            ROUTES_FIRST
        };
        let explain = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {raw}");
        let mut query = sqlx::query_scalar::<_, Value>(&explain).bind(scope).bind(tenant);
        if let Some(cursor) = cursor {
            query = query.bind(cursor);
        }
        let plan = query.fetch_one(&mut *conn).await?;
        bounded_leaf(&plan, "leg_routes", &[INDEX], 1, 8)?;
        bounded_leaf(&plan, "balance_legs", &["balance_legs_pkey"], 8, 1)?;
        bounded_leaf(
            &plan,
            "balance_leg_admissions",
            &["idx_balance_leg_admissions_current"],
            8,
            1,
        )?;
    }
    conn.rollback().await?;
    Ok(())
}
