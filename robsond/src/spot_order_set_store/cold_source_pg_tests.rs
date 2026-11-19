//! Local PG16 coverage for the physical source windows and existing btrees.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::PgPool;

use super::*;

const SCOPE: &str = "cold-scope";
const DEEP: &str = "cold-deep";
const NOISE: u32 = 8192;
const SYMBOL: &str = "ZUSDT";

fn client(n: u32) -> String {
    if n == 1 {
        String::new()
    } else {
        format!("{n:08}")
    }
}

fn exchange(n: u32) -> String {
    (100_000_000 + n).to_string()
}

async fn seed(pool: &PgPool, account: &str, owns: Uuid, foreign: Uuid, total: u32) -> Result<()> {
    let total = i64::from(total);
    sqlx::query(
        "INSERT INTO order_intents(tenant_id,venue_id,account_scope,client_order_id,
              intent_id,position_id,kind,params,created_at)
         SELECT CASE WHEN n=$4-1 THEN $3 ELSE $2 END,'binance:spot',$1,
              CASE WHEN n=1 THEN '' ELSE lpad(n::text,8,'0') END,
              md5($1||'-j-'||n)::uuid,md5($1||'-position-'||n)::uuid,'exit','{}',now()
         FROM generate_series(1,$4::bigint) n ORDER BY (n*13)%($4*2+1)",
    )
    .bind(account)
    .bind(owns)
    .bind(foreign)
    .bind(total)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO submit_attempts(attempt_id,venue_id,account_scope,client_order_id,
              state,created_at,rejected_at)
         SELECT md5(venue_id||account_scope||'-a-'||client_order_id)::uuid,venue_id,account_scope,
              client_order_id,'started',now(),NULL
         FROM order_intents WHERE venue_id='binance:spot' AND account_scope=$1",
    )
    .bind(account)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO venue_orders(venue_id,account_scope,symbol,exchange_order_id,tenant_id,
              client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,
              last_checked_at,live_seen_at)
         SELECT 'binance:spot',$1,'ZUSDT',(100000000+n)::text,
              CASE WHEN n=$4-1 THEN $3 ELSE $2 END,'o-'||n,'o-'||n,
              'discovered','SELL','LIMIT',1,'NEW','live',now(),now()
         FROM generate_series(1,$4::bigint) n ORDER BY (n*29)%($4*3+1)",
    )
    .bind(account)
    .bind(owns)
    .bind(foreign)
    .bind(total)
    .execute(pool)
    .await?;
    Ok(())
}

fn plan_leaves<'a>(node: &'a Value, relation: &str, found: &mut Vec<&'a Value>) {
    if node["Relation Name"] == relation {
        found.push(node);
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            plan_leaves(child, relation, found);
        }
    }
}

fn bounded_leaf(
    plan: &Value,
    relation: &str,
    indexes: &[&str],
    max_loops: u64,
    max_rows: u64,
) -> Result<()> {
    let mut leaves = Vec::new();
    plan_leaves(&plan[0]["Plan"], relation, &mut leaves);
    ensure!(leaves.len() == 1, "{relation} must appear as one native access: {plan}");
    let scan = leaves[0];
    let index = scan["Index Name"].as_str().context("default planner chose a non-index scan")?;
    let loops = scan["Actual Loops"].as_u64().context("actual native loops")?;
    let rows = scan["Actual Rows"].as_u64().context("actual native rows")?;
    ensure!(
        indexes.contains(&index)
            && scan["Index Cond"].as_str().is_some()
            && (1..=max_loops).contains(&loops)
            && (1..=max_rows).contains(&rows)
            && scan["Rows Removed by Filter"].as_u64().unwrap_or(0) == 0,
        "{relation} did not use a bounded native index: {plan}",
    );
    eprintln!(
        "Cold Spot native {relation} index={index} loops={loops} rows_per_loop={rows} shared_hits={} shared_reads={}",
        scan["Shared Hit Blocks"], scan["Shared Read Blocks"]
    );
    Ok(())
}

async fn explain_page(
    conn: &mut PgConnection,
    sql: &str,
    account: &str,
    tenant: Uuid,
    cursor: &[&str],
) -> Result<Value> {
    let explain = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {sql}");
    let mut query = sqlx::query_scalar(&explain).bind(account).bind(tenant);
    for part in cursor {
        query = query.bind(*part);
    }
    Ok(query.fetch_one(conn).await?)
}

async fn assert_page_family(
    conn: &mut PgConnection,
    account: &str,
    tenant: Uuid,
    foreign: Uuid,
    preceding: u32,
) -> Result<()> {
    let last_client = client(preceding);
    let last_id = exchange(preceding);
    let intent_cursor = (preceding != 0).then_some(last_client.as_str());
    let order_cursor = (preceding != 0).then_some((SYMBOL, last_id.as_str()));
    let j = read_intent_page(&mut *conn, account, tenant, intent_cursor).await?;
    let a = read_attempt_page(&mut *conn, account, tenant, intent_cursor).await?;
    let o = read_order_page(&mut *conn, account, tenant, order_cursor).await?;
    ensure!(j.full && a.full && o.full);
    for (offset, ((intent, attempt), order)) in j.rows.iter().zip(&a.rows).zip(&o.rows).enumerate()
    {
        let n = preceding + offset as u32 + 1;
        ensure!(intent.client_order_id == client(n) && intent.owner_matches);
        ensure!(attempt.client_order_id == client(n) && attempt.owner_matches);
        ensure!(
            attempt.parent_intent_id == Some(intent.intent_id)
                && attempt.parent_tenant_id == Some(tenant)
        );
        ensure!(
            order.symbol == SYMBOL && order.exchange_order_id == exchange(n) && order.owner_matches
        );
        ensure!(
            order.owner == "discovered" && order.intent_id.is_none() && order.position_id.is_none()
        );
    }
    let j = read_intent_page(
        &mut *conn,
        account,
        tenant,
        Some(&j.rows.last().context("first J page")?.client_order_id),
    )
    .await?;
    let a = read_attempt_page(
        &mut *conn,
        account,
        tenant,
        Some(&a.rows.last().context("first attempt page")?.client_order_id),
    )
    .await?;
    let last_order = o.rows.last().context("first native order page")?;
    let o = read_order_page(
        &mut *conn,
        account,
        tenant,
        Some((&last_order.symbol, &last_order.exchange_order_id)),
    )
    .await?;
    ensure!(
        !j.full && !a.full && !o.full && (j.rows.len(), a.rows.len(), o.rows.len()) == (2, 2, 2)
    );
    ensure!(j.rows[0].client_order_id == client(preceding + 9) && j.rows[0].tenant_id == foreign);
    ensure!(
        a.rows[0].parent_intent_id == Some(j.rows[0].intent_id)
            && a.rows[0].parent_tenant_id == Some(foreign)
    );
    ensure!(
        o.rows[0].exchange_order_id == exchange(preceding + 9) && o.rows[0].tenant_id == foreign
    );
    ensure!(!j.rows[0].owner_matches && !a.rows[0].owner_matches && !o.rows[0].owner_matches);
    ensure!(j.rows[1].owner_matches && a.rows[1].owner_matches && o.rows[1].owner_matches);
    ensure!(
        j.rows[1].client_order_id == client(preceding + 10)
            && a.rows[1].parent_intent_id == Some(j.rows[1].intent_id)
            && o.rows[1].exchange_order_id == exchange(preceding + 10)
    );

    ensure!(read_intent_page(&mut *conn, account, tenant, Some(&j.rows[1].client_order_id))
        .await?
        .rows
        .is_empty());
    ensure!(read_attempt_page(&mut *conn, account, tenant, Some(&a.rows[1].client_order_id))
        .await?
        .rows
        .is_empty());
    ensure!(read_order_page(
        &mut *conn,
        account,
        tenant,
        Some((&o.rows[1].symbol, &o.rows[1].exchange_order_id))
    )
    .await?
    .rows
    .is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_cold_source_index_and_owner_boundary(pool: PgPool) -> Result<()> {
    let tenant = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    for id in [tenant, foreign] {
        sqlx::query(
            "INSERT INTO accounting_state(tenant_id,venue_id,budget_model)
              VALUES($1,'binance:spot','net_from_start_non_expanding_v1')",
        )
        .bind(id)
        .execute(&pool)
        .await?;
    }
    seed(&pool, SCOPE, tenant, foreign, 10).await?;
    seed(&pool, DEEP, tenant, foreign, NOISE + 10).await?;
    seed(&pool, "separate-account", tenant, foreign, NOISE).await?;
    sqlx::raw_sql("ANALYZE order_intents; ANALYZE submit_attempts; ANALYZE venue_orders")
        .execute(&pool)
        .await?;

    let mut conn = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    assert_page_family(&mut conn, SCOPE, tenant, foreign, 0).await?;
    assert_page_family(&mut conn, DEEP, tenant, foreign, NOISE).await?;
    // An allowed empty client key must advance exactly one physical row.
    ensure!(
        read_intent_page(&mut conn, SCOPE, tenant, Some("")).await?.rows[0].client_order_id
            == client(2)
    );
    ensure!(
        read_attempt_page(&mut conn, SCOPE, tenant, Some("")).await?.rows[0].client_order_id
            == client(2)
    );

    let client_cursor = client(NOISE);
    let order_cursor = exchange(NOISE);
    for (account, j, a, o, c, n) in [
        (SCOPE, INTENTS_FIRST, ATTEMPTS_FIRST, ORDERS_FIRST, &[][..], &[][..]),
        (
            DEEP,
            INTENTS_NEXT,
            ATTEMPTS_NEXT,
            ORDERS_NEXT,
            &[client_cursor.as_str()][..],
            &[SYMBOL, order_cursor.as_str()][..],
        ),
    ] {
        let plan = explain_page(&mut conn, j, account, tenant, c).await?;
        bounded_leaf(&plan, "order_intents", &["order_intents_client_id_unique"], 1, 8)?;
        let plan = explain_page(&mut conn, a, account, tenant, c).await?;
        bounded_leaf(
            &plan,
            "submit_attempts",
            &[
                "idx_submit_attempts_one_per_spot_client_id",
                "idx_submit_attempts_client_id",
            ],
            1,
            8,
        )?;
        bounded_leaf(&plan, "order_intents", &["order_intents_client_id_unique"], 8, 1)?;
        let plan = explain_page(&mut conn, o, account, tenant, n).await?;
        bounded_leaf(
            &plan,
            "venue_orders",
            &[
                "venue_orders_pkey",
                "venue_orders_venue_id_account_scope_symbol_exchange_order_i_key",
            ],
            1,
            8,
        )?;
    }
    conn.rollback().await?;
    eprintln!("Cold Spot native six plans verified, all three sources preserve owner mismatch at row 9, noise after 8192");
    Ok(())
}
