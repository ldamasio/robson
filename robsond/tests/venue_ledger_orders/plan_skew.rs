use super::*;

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_native_order_default_plan_bounds_exact_terminal_target(
    pool: PgPool,
) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    use robson_exec::venue_ledger::merge_order;
    use serde_json::Value;

    const NOISE: i32 = 4_096;
    const TARGET_MEMBERS: i32 = 512;
    let f = setup(&pool).await?;
    let intent = Uuid::now_v7();
    journal(&pool, &f, intent, "order-1", "exit", Some(f.position), None, false).await?;
    let native = f.ledger(&pool);
    let link = linked(&f, 1, intent);
    ensure!(
        native
            .record_order(&f.record(1, Decimal::ZERO, OrderPhase::Live), &link, seen())
            .await?
            .changed
    );

    // Real same-account/symbol paths and a populated group via the accepted shadow
    // hooks.
    sqlx::query(
        "INSERT INTO public.venue_orders (
             tenant_id,venue_id,account_scope,symbol,exchange_order_id,client_order_id,
             last_client_order_id,owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at)
         SELECT $1,'binance:spot',$2,$3,(10000+i)::text,'skew-native-'||i,'skew-native-'||i,
             'discovered','SELL','LIMIT',1,'NEW','live',now(),now()
         FROM generate_series(1,$4::integer) i",
    )
    .bind(f.tenant).bind(&f.scope).bind(SYMBOL).bind(NOISE).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO public.order_intents (
             intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,
             cancels_order_id,params,created_at)
         SELECT md5($2||'skew-cancel-'||i)::uuid,$1,'binance:spot',$2,'skew-cancel-'||i,$3,
             'cancel',CASE WHEN i<=$4 THEN '1' ELSE (20000+i)::text END,'{}',now()
         FROM generate_series(1,$5::integer+$4::integer) i",
    )
    .bind(f.tenant)
    .bind(&f.scope)
    .bind(f.position)
    .bind(TARGET_MEMBERS)
    .bind(NOISE)
    .execute(&pool)
    .await?;
    let b = generation(&pool, &f).await?;

    // Feed the planner realistic cardinality while keeping this generation's rows
    // under the independent shadow. All extra projection keys are other scopes.
    sqlx::query(
        "WITH k AS MATERIALIZED (
             SELECT $1::uuid AS t, 'binance:spot'::text AS v, $2::text||'-noise-'||i AS a,
                    md5('plan-g-'||i)::uuid AS g, md5('plan-p-'||i)::uuid AS p,
                    md5('plan-j-'||i)::uuid AS j, $3::text AS s
             FROM generate_series(1,$4::integer) i
         ), new_routes AS (
             INSERT INTO public.leg_routes (position_id,tenant_id,venue_id,account_scope,routed_at)
             SELECT p,t,v,a,now() FROM k RETURNING position_id
         ), new_admissions AS (
             INSERT INTO public.balance_leg_admissions
             SELECT (jsonb_populate_record(NULL::public.balance_leg_admissions,
                       to_jsonb(src) || jsonb_build_object('entry_intent_id',k.j,
                           'position_id',k.p,'account_scope',k.a))).*
             FROM public.balance_leg_admissions src CROSS JOIN k
             JOIN new_routes r ON r.position_id=k.p
             WHERE src.entry_intent_id=$5
             RETURNING entry_intent_id
         ), new_legs AS (
             INSERT INTO public.balance_legs
             SELECT (jsonb_populate_record(NULL::public.balance_legs,
                       to_jsonb(src) || jsonb_build_object('admission_intent_id',k.j,
                           'position_id',k.p,'account_scope',k.a))).*
             FROM public.balance_legs src CROSS JOIN k
             JOIN new_admissions a ON a.entry_intent_id=k.j
             WHERE src.admission_intent_id=$5
         ), epochs AS (
             INSERT INTO public.process_epochs (tenant_id,epoch_id,binary_version)
             SELECT p,j,'order-plan-decoy' FROM k
         ), generations AS (
             INSERT INTO public.spot_order_set_generation (tenant_id,venue_id,account_scope,generation_id)
             SELECT t,v,a,g FROM k
         ), sl AS (
             INSERT INTO public.spot_order_set_sell_legs
                 (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,potential_count,conflict_count)
             SELECT t,v,a,g,s,p,1,0 FROM k
         ), si AS (
             INSERT INTO public.spot_order_set_sell_intents
                 (tenant_id,venue_id,account_scope,generation_id,intent_id,scope_kind,potential,conflict)
             SELECT t,v,a,g,j,'unbound',true,false FROM k
         ), ss AS (
             INSERT INTO public.spot_order_set_sell_symbols
                 (tenant_id,venue_id,account_scope,generation_id,symbol,potential_count,conflict_count)
             SELECT t,v,a,g,s,1,0 FROM k
         ), su AS (
             INSERT INTO public.spot_order_set_sell_unbound
                 (tenant_id,venue_id,account_scope,generation_id,potential_count,conflict_count)
             SELECT t,v,a,g,1,0 FROM k
         ), cl AS (
             INSERT INTO public.spot_order_set_cancel_legs
                 (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,active_count,conflict_count)
             SELECT t,v,a,g,s,p,1,0 FROM k
         ), cx AS (
             INSERT INTO public.spot_order_set_cancel_ungroupable
                 (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,active_count,conflict_count)
             SELECT t,v,a,g,s,p,1,0 FROM k
         )
         INSERT INTO public.spot_order_set_cancel_unbound
             (tenant_id,venue_id,account_scope,generation_id,active_count,conflict_count)
         SELECT t,v,a,g,1,0 FROM k",
    )
    .bind(b.tenant_id).bind(&b.account_scope).bind(SYMBOL).bind(NOISE).bind(f.entry.intent.intent_id)
    .execute(&pool).await?;
    sqlx::raw_sql(
        "ANALYZE public.process_epochs, public.venue_orders, public.order_intents,
           public.balance_legs, public.spot_order_set_generation,
           public.spot_order_set_sell_orders, public.spot_order_set_sell_intents,
           public.spot_order_set_sell_legs, public.spot_order_set_sell_symbols,
           public.spot_order_set_sell_unbound, public.spot_order_set_cancel_members,
           public.spot_order_set_cancel_groups, public.spot_order_set_cancel_legs,
           public.spot_order_set_cancel_ungroupable, public.spot_order_set_cancel_unbound",
    )
    .execute(&pool)
    .await?;

    let group = |terminal| {
        sqlx::query_as::<_, (i64, i64, bool)>(
            "SELECT pending_count, conflict_count, terminal_mask
             FROM public.spot_order_set_cancel_groups
             WHERE (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id)
                 = ($1,'binance:spot',$2,$3,$4,$5,'1') AND terminal_mask=$6",
        )
        .bind(b.tenant_id)
        .bind(&b.account_scope)
        .bind(b.generation_id)
        .bind(SYMBOL)
        .bind(f.position)
        .bind(terminal)
        .fetch_one(&pool)
    };
    ensure!(group(false).await? == (i64::from(TARGET_MEMBERS), 0, false));
    let before = version(&pool, &b).await?;
    let before_projection = projection_snapshot(&pool, &b).await?;
    let mut tx = pool.begin().await?;
    let advanced: i64 = sqlx::query_scalar(
        "UPDATE public.accounting_state SET version=version+1
         WHERE tenant_id=$1 AND venue_id='binance:spot' RETURNING version",
    )
    .bind(b.tenant_id)
    .fetch_one(&mut *tx)
    .await?;
    ensure!(advanced == before + 1);

    let key = f.key(1);
    let locked = native.read_order_context(&mut tx, &key, Some(f.position), true).await?;
    let prior = locked.order.as_ref().context("exact locked live order")?;
    ensure!(
        matches!(&prior.owner, OrderOwner::Leg {position_id, intent_id, kind: LegOrderKind::Sell}
        if *position_id == f.position && *intent_id == intent)
    );
    ensure!(prior.phase == OrderPhase::Live && prior.status == "PARTIALLY_FILLED");
    let merged = merge_order(
        Some(prior),
        &f.record(1, Decimal::ZERO, OrderPhase::Terminal),
        &link,
        &later(1),
        locked.leg.as_ref(),
    )?;
    ensure!(merged.changed && merged.consumed_delta == Decimal::ZERO && !merged.set_cohort);
    native.write_order(&mut tx, &merged.order, true).await?;

    // Execute the very same maintenance SQL and original fifteen binds. A plain
    // EXPLAIN only predicts work; this exercise performs all DML then rolls back.
    let explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT JSON) {}",
        crate::venue_ledger_store::orders::ORDER_PROJECTION,
    );
    let plan: Value = sqlx::query_scalar(&explain)
        .bind(b.tenant_id)
        .bind(&b.account_scope)
        .bind(b.generation_id)
        .bind(b.process_epoch)
        .bind(b.source_audit_id)
        .bind(advanced)
        .bind(&key.symbol)
        .bind(key.exchange_order_id.to_string())
        .bind(true)
        .bind(Some("leg"))
        .bind(Some(intent))
        .bind(Some(f.position))
        .bind(Some("sell"))
        .bind(Some("live"))
        .bind(Some(prior.status.as_str()))
        .fetch_one(&mut *tx)
        .await?;
    let state: (i64, i64, i64, bool, bool) = sqlx::query_as(
        "SELECT g.accounting_version, c.pending_count, l.active_count, c.terminal_mask, s.potential
         FROM public.spot_order_set_generation g
         JOIN public.spot_order_set_cancel_groups c USING (tenant_id,venue_id,account_scope,generation_id)
         JOIN public.spot_order_set_cancel_legs l USING (tenant_id,venue_id,account_scope,generation_id,symbol,position_id)
         JOIN public.spot_order_set_sell_orders s USING (tenant_id,venue_id,account_scope,generation_id,symbol)
         WHERE (g.tenant_id,g.venue_id,g.account_scope,g.generation_id,c.symbol,c.position_id,c.target_id,s.exchange_order_id)
             = ($1,'binance:spot',$2,$3,$4,$5,'1','1')",
    )
    .bind(b.tenant_id).bind(&b.account_scope).bind(b.generation_id).bind(SYMBOL).bind(f.position)
    .fetch_one(&mut *tx).await?;
    ensure!(state == (advanced, i64::from(TARGET_MEMBERS), i64::from(NOISE), true, false));
    tx.rollback().await?;
    ensure!(version(&pool, &b).await? == before);
    ensure!(group(false).await? == (i64::from(TARGET_MEMBERS), 0, false));
    ensure!(projection_snapshot(&pool, &b).await? == before_projection);
    let source_phase: String = sqlx::query_scalar(
        "SELECT phase FROM public.venue_orders
         WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)
             = ($1,'binance:spot',$2,$3,'1')",
    )
    .bind(b.tenant_id)
    .bind(&b.account_scope)
    .bind(SYMBOL)
    .fetch_one(&pool)
    .await?;
    ensure!(source_phase == "live");

    // Traverse every executed physical access, including CTE init plans and DML
    // input scans. Read and UPDATE are each allowed one access to a touched key.
    let mut nodes = vec![&plan[0]["Plan"]];
    let mut probes = BTreeMap::<&str, u64>::new();
    let mut actual_updates = BTreeSet::new();
    while let Some(node) = nodes.pop() {
        if let Some(children) = node["Plans"].as_array() {
            nodes.extend(children);
        }
        let loops = node["Actual Loops"].as_u64().unwrap_or(0);
        if loops == 0 {
            continue;
        }
        let rows = node["Actual Rows"].as_u64().context("EXPLAIN actual rows")?;
        let operation = node["Node Type"].as_str().context("EXPLAIN node type")?;
        if operation == "ModifyTable" {
            if let Some(name) = node["Subplan Name"].as_str() {
                if rows > 0 {
                    ensure!(loops == 1 && rows == 1, "unbounded writes: {node:#}");
                    actual_updates.insert(name);
                }
            }
            continue;
        }
        let Some(relation) = node["Relation Name"].as_str() else {
            continue;
        };
        let owner = &["tenant_id", "venue_id", "account_scope"][..];
        let generation = &["tenant_id", "venue_id", "account_scope", "generation_id"][..];
        let (index, key_fields, limit) = match relation {
            "process_epochs" => ("idx_process_epochs_tenant_seq", &["tenant_id"][..], 1),
            "venue_orders" => (
                "venue_orders_pkey",
                &["venue_id", "account_scope", "symbol", "exchange_order_id"][..],
                1,
            ),
            "order_intents" => ("order_intents_pkey", &["intent_id"][..], 1),
            "balance_legs" => ("balance_legs_pkey", &["position_id"][..], 1),
            "spot_order_set_generation" => ("spot_order_set_generation_key", owner, 2),
            "spot_order_set_sell_orders" => (
                "spot_order_set_sell_orders_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                    "exchange_order_id",
                ][..],
                2,
            ),
            "spot_order_set_sell_intents" => (
                "spot_order_set_sell_intents_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "intent_id",
                ][..],
                1,
            ),
            "spot_order_set_sell_legs" => (
                "spot_order_set_sell_legs_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                    "position_id",
                ][..],
                2,
            ),
            "spot_order_set_cancel_groups" => (
                "spot_order_set_cancel_groups_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                    "position_id",
                    "target_id",
                ][..],
                2,
            ),
            "spot_order_set_cancel_legs" => (
                "spot_order_set_cancel_legs_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                    "position_id",
                ][..],
                2,
            ),
            "spot_order_set_sell_symbols" => (
                "spot_order_set_sell_symbols_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                ][..],
                1,
            ),
            "spot_order_set_cancel_ungroupable" => (
                "spot_order_set_cancel_ungroupable_key",
                &[
                    "tenant_id",
                    "venue_id",
                    "account_scope",
                    "generation_id",
                    "symbol",
                    "position_id",
                ][..],
                1,
            ),
            "spot_order_set_sell_unbound" => ("spot_order_set_sell_unbound_key", generation, 1),
            "spot_order_set_cancel_unbound" => ("spot_order_set_cancel_unbound_key", generation, 1),
            _ => anyhow::bail!(
                "unexpected physical relation access in terminal SELL branch: {node:#}"
            ),
        };
        let condition = node["Index Cond"]
            .as_str()
            .context("physical node lacked Index Cond")
            .with_context(|| format!("{relation}: {node:#}"))?;
        ensure!(
            matches!(operation, "Index Scan" | "Index Only Scan")
                && node["Index Name"].as_str() == Some(index)
                && key_fields.iter().all(|field| condition
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|token| token == *field))
                && loops == 1
                && rows <= 1
                && node["Rows Removed by Filter"].as_u64().unwrap_or(0) == 0
                && node["Rows Removed by Index Recheck"].as_u64().unwrap_or(0) == 0,
            "non-keyed work in {relation}: {node:#}",
        );
        let total = probes.entry(relation).or_default();
        *total += loops;
        ensure!(*total <= limit, "too many probes on {relation}: {total} > {limit}");
    }
    for relation in [
        "process_epochs",
        "venue_orders",
        "order_intents",
        "balance_legs",
        "spot_order_set_generation",
        "spot_order_set_sell_orders",
        "spot_order_set_sell_intents",
        "spot_order_set_sell_legs",
        "spot_order_set_cancel_groups",
        "spot_order_set_cancel_legs",
    ] {
        ensure!(probes.contains_key(relation), "vacant plan witness for {relation}");
    }
    ensure!(
        actual_updates
            == BTreeSet::from([
                "CTE sell_order_update",
                "CTE sell_leg_update",
                "CTE cancel_group_update",
                "CTE cancel_leg_update",
            ]),
        "unexpected real mutations: {actual_updates:?}"
    );
    eprintln!("PG16 terminal record_order plan probes={probes:?} updates={actual_updates:?}");
    Ok(())
}
