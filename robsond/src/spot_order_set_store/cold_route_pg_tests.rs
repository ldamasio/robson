//! Native route-cursor, current-leg and restore-audit constituent evidence.
//! The candidate index exists only inside these disposable databases.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    pg_tests::{bounded_leaf, plan_leaves},
    read_route_page, RouteClass, RouteDefect, ROUTES_FIRST, ROUTES_NEXT,
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
    stride: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO public.leg_routes (position_id,tenant_id,venue_id,account_scope,routed_at)
         SELECT lpad(to_hex(i*$6::bigint+$4::bigint),32,'0')::uuid,
                CASE WHEN i=$5::bigint-1 THEN $3 ELSE $2 END,'binance:spot',$1,now()
           FROM generate_series(1,$5::bigint) i ORDER BY (i*29)%($5*3+1)",
    )
    .bind(scope)
    .bind(tenant)
    .bind(foreign)
    .bind(start)
    .bind(size)
    .bind(stride)
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

async fn native_admission(
    pool: &PgPool,
    tenant: Uuid,
    scope: &str,
    position: Uuid,
    intent: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE public.accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(tenant).execute(&mut *tx).await?;
    sqlx::query(
        "SELECT balance_leg_admit($1,'binance:spot',$2,'BTCUSDT',$3,$4,'2026-11-01',
            10,10,2,10,0.001,'base','fixture',now(),'fixture',1,1,100,0.1,10,90,now(),false)",
    )
    .bind(tenant)
    .bind(scope)
    .bind(position)
    .bind(intent)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

type NativeKeys = (Uuid, Uuid, Option<Uuid>, Option<Uuid>);

fn exact_window(expected: &[NativeKeys], matching: &[NativeKeys], reached_end: bool) -> bool {
    reached_end && matching == expected
}

fn native_keys(row: &super::RouteSource) -> NativeKeys {
    (
        row.position_id,
        row.tenant_id,
        row.leg_admission_intent_id,
        row.admission_intent_id,
    )
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_cold_route_native_pages_preserve_invalid_and_armed_rows(
    pool: PgPool,
) -> Result<()> {
    let tenant = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    setup(&pool, tenant, foreign).await?;
    routes(&pool, PRIMARY, tenant, foreign, 0, 10, 1).await?;
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

    let mut bad = pool.begin().await?;
    // The constraints permit historical references while a corrupted
    // supersession chain has no current row.
    sqlx::query(
        "INSERT INTO public.balance_leg_admissions (
             entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,
             taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,
             envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,
             reference_price,remainder_reserve,admission_charge,initial_trigger,created_at,
             superseded_at,superseded_by)
         SELECT $2,a.position_id,a.tenant_id,a.venue_id,a.account_scope,a.symbol,a.decided_month,
                a.taker_bps,a.gap_bps,a.notional_margin_bps,a.min_notional,a.lot_step,a.fee_asset_policy,
                a.envelope_source,a.envelope_as_of,a.envelope_hash,a.planned_qty,a.requested_qty,
                a.reference_price,a.remainder_reserve,a.admission_charge,a.initial_trigger,now(),
                now(),a.entry_intent_id
           FROM public.balance_leg_admissions a WHERE a.position_id=$1",
    )
    .bind(position(7)).bind(Uuid::now_v7())
    .execute(&mut *bad).await?;
    sqlx::query(
        "UPDATE public.balance_leg_admissions a SET superseded_at=now(),superseded_by=other.entry_intent_id
         FROM public.balance_leg_admissions other
         WHERE a.position_id=$1 AND other.position_id=a.position_id AND a.superseded_at IS NULL
           AND other.superseded_at IS NOT NULL",
    )
    .bind(position(7)).execute(&mut *bad).await?;
    bad.commit().await?;

    // Native allowed supersession establishes an independently generated current
    // owner.
    sqlx::query(
        "INSERT INTO public.order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,
          position_id,kind,params,created_at,not_sent)
         SELECT entry_intent_id,tenant_id,venue_id,account_scope,entry_intent_id::text,
                position_id,'entry','{}',now(),true
           FROM public.balance_leg_admissions WHERE position_id=$1",
    )
    .bind(position(3)).execute(&pool).await?;
    let valid_successor = Uuid::now_v7();
    native_admission(&pool, tenant, PRIMARY, position(3), valid_successor).await?;

    let mut conn = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    let oracle: Vec<NativeKeys> = sqlx::query_as(
        "SELECT r.position_id,r.tenant_id,l.admission_intent_id,a.entry_intent_id
         FROM public.leg_routes r LEFT JOIN public.balance_legs l ON l.position_id=r.position_id
         LEFT JOIN public.balance_leg_admissions a ON a.position_id=r.position_id AND a.superseded_at IS NULL
         WHERE r.account_scope=$1 AND r.venue_id='binance:spot' ORDER BY r.position_id",
    )
    .bind(PRIMARY).fetch_all(&mut *conn).await?;
    let mut first = read_route_page(&mut conn, PRIMARY, tenant, None).await?;
    ensure!(first.full && first.rows.len() == 8);
    ensure!(first.rows[0].classify() == RouteClass::ArmedOnly && first.rows[0].eligible());
    ensure!(matches!(first.rows[1].classify(), RouteClass::Leg { closed: true }));
    ensure!(first.rows[4].classify() == RouteClass::Diagnostic(RouteDefect::AdmissionWithoutLeg));
    ensure!(first.rows[5].classify() == RouteClass::Diagnostic(RouteDefect::StaleLegPointer));
    ensure!(first.rows[5].admission_intent_id == Some(successor));
    ensure!(first.rows[5].leg_admission_intent_id != Some(successor));
    ensure!(!first.rows[4].eligible() && !first.rows[5].eligible());
    ensure!(
        first.rows[2].leg_admission_intent_id == Some(valid_successor)
            && first.rows[2].admission_intent_id == Some(valid_successor)
            && first.rows[2].classify() == (RouteClass::Leg { closed: false })
    );
    ensure!(first.rows[6].classify() == RouteClass::Diagnostic(RouteDefect::LegWithoutAdmission));
    // The FK forbids cross-scope SQL facts in the validated source. A caller
    // consuming typed raw evidence must still refuse these mismatched facts.
    let leg_owner = first.rows[2].leg_account_scope.replace("forged-scope".into());
    ensure!(first.rows[2].classify() == RouteClass::Diagnostic(RouteDefect::LegRouteMismatch));
    first.rows[2].leg_account_scope = leg_owner;
    let admission_symbol = first.rows[2].admission_symbol.replace("FORGED".into());
    ensure!(first.rows[2].classify() == RouteClass::Diagnostic(RouteDefect::PairMismatch));
    first.rows[2].admission_symbol = admission_symbol;
    let last = first.rows.last().context("full route page")?.position_id;
    ensure!(last == position(8));

    // A real native admission drawn after the first read must be absent from
    // both counterpart probes in this independent read-only snapshot.
    native_admission(&pool, tenant, PRIMARY, position(10), Uuid::now_v7()).await?;
    let second = read_route_page(&mut conn, PRIMARY, tenant, Some(last)).await?;
    ensure!(!second.full && second.rows.len() == 2);
    ensure!(second.rows[0].position_id == position(9) && second.rows[0].tenant_id == foreign);
    ensure!(second.rows[0].classify() == RouteClass::Diagnostic(RouteDefect::ForeignOwner));
    ensure!(
        second.rows[1].position_id == position(10)
            && second.rows[1].classify() == RouteClass::ArmedOnly
    );
    let actual: Vec<_> = first.rows.iter().chain(&second.rows).map(native_keys).collect();
    ensure!(exact_window(&oracle, &actual, !second.full));
    ensure!(!exact_window(&oracle, &actual, false));
    let skipped = read_route_page(&mut conn, PRIMARY, tenant, Some(position(9))).await?;
    let incomplete: Vec<_> = first.rows.iter().chain(&skipped.rows).map(native_keys).collect();
    ensure!(!exact_window(&oracle, &incomplete, !skipped.full));
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
    for (scope, base, size) in [(PRIMARY, 0, 10), (DEEP, DEEP_BASE, NOISE + 10)] {
        routes(&pool, scope, tenant, foreign, base, size, 4).await?;
        seed_admissions(&pool, scope, &[]).await?;
        seed_legs(&pool, scope, &[], Uuid::nil()).await?;
    }
    sqlx::query(
        "INSERT INTO public.accounting_state (tenant_id,venue_id,budget_model)
         VALUES ($1,'binance:usdm','hwm_v1')",
    )
    .bind(tenant)
    .execute(&pool)
    .await?;
    // Both kinds of noise are interleaved in the native PK, including around
    // the late selected page. Neither may enter the account/venue range.
    sqlx::query(
        "INSERT INTO public.leg_routes (position_id,tenant_id,venue_id,account_scope,routed_at)
         SELECT lpad(to_hex(x.base+i*4+noise.delta),32,'0')::uuid,
                $1,noise.venue,CASE WHEN noise.delta=1 THEN 'other-account' ELSE x.scope END,now()
           FROM (VALUES (0::bigint,$2::text),($3::bigint,$4::text)) x(base,scope)
           CROSS JOIN (VALUES (1,'binance:spot'),(2,'binance:usdm')) noise(delta,venue)
           CROSS JOIN generate_series(1,$5::bigint) i",
    )
    .bind(tenant)
    .bind(PRIMARY)
    .bind(DEEP_BASE)
    .bind(DEEP)
    .bind(NOISE + 10)
    .execute(&pool)
    .await?;
    sqlx::raw_sql("ANALYZE public.leg_routes; ANALYZE public.balance_legs; ANALYZE public.balance_leg_admissions")
        .execute(&pool).await?;
    let mut conn = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    for (scope, base, cursor) in [
        (PRIMARY, 0, None),
        (DEEP, DEEP_BASE + 4 * NOISE, Some(position(DEEP_BASE + 4 * NOISE))),
    ] {
        let first = read_route_page(&mut conn, scope, tenant, cursor).await?;
        ensure!(first.full);
        ensure!(first.rows.iter().all(|row| row.eligible()));
        let last = first.rows.last().context("full source page")?.position_id;
        ensure!(last == position(base + 4 * 8));
        let second = read_route_page(&mut conn, scope, tenant, Some(last)).await?;
        ensure!(!second.full && second.rows.len() == 2);
        ensure!(
            second.rows[0].position_id == position(base + 4 * 9)
                && second.rows[0].tenant_id == foreign
        );
        ensure!(second.rows[0].classify() == RouteClass::Diagnostic(RouteDefect::ForeignOwner));
        ensure!(second.rows[1].position_id == position(base + 4 * 10) && second.rows[1].eligible());

        for after in [cursor, Some(last)] {
            let raw = if after.is_some() {
                ROUTES_NEXT
            } else {
                ROUTES_FIRST
            };
            let explain = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {raw}");
            let mut query = sqlx::query_scalar::<_, Value>(&explain).bind(scope).bind(tenant);
            if let Some(after) = after {
                query = query.bind(after);
            }
            let plan = query.fetch_one(&mut *conn).await?;
            bounded_leaf(&plan, "leg_routes", &[INDEX], 1, 8)?;
            bounded_leaf(
                &plan,
                "balance_legs",
                &[
                    "balance_legs_pkey",
                    "balance_legs_position_id_tenant_id_venue_id_account_scope_s_key",
                ],
                8,
                1,
            )?;
            bounded_leaf(
                &plan,
                "balance_leg_admissions",
                &["idx_balance_leg_admissions_current"],
                8,
                1,
            )?;

            let condition = |relation| -> Result<&str> {
                let mut leaves = Vec::new();
                plan_leaves(&plan[0]["Plan"], relation, &mut leaves);
                ensure!(leaves.len() == 1);
                leaves[0]["Index Cond"].as_str().context("native seek condition")
            };
            let route_seek = condition("leg_routes")?;
            ensure!(route_seek.contains("account_scope ="), "{route_seek}");
            ensure!(route_seek.contains("position_id >") == after.is_some(), "{route_seek}");
            for relation in ["balance_legs", "balance_leg_admissions"] {
                let seek = condition(relation)?;
                ensure!(seek.contains("position_id = r.position_id"), "{seek}");
            }
        }
    }
    conn.rollback().await?;
    Ok(())
}
