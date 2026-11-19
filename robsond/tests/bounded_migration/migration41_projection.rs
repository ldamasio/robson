//! Verify the isolated expanded M41's physical owner keys and native hint
//! index.
use super::*;

const SCOPE: [&str; 4] = ["tenant_id", "venue_id", "account_scope", "generation_id"];
const LEAVES: [(&str, &[&str]); 10] = [
    ("sell_intents", &["intent_id"]),
    ("sell_orders", &["symbol", "exchange_order_id"]),
    ("sell_legs", &["symbol", "position_id"]),
    ("sell_symbols", &["symbol"]),
    ("sell_unbound", &[]),
    ("cancel_members", &["intent_id"]),
    ("cancel_groups", &["symbol", "position_id", "target_id"]),
    ("cancel_legs", &["symbol", "position_id"]),
    ("cancel_unbound", &[]),
    ("cancel_ungroupable", &["symbol", "position_id"]),
];
const PAGE: &str = "SELECT intent_id FROM public.spot_order_set_sell_intents
    WHERE (tenant_id,venue_id,account_scope,generation_id,source_position)=($1,$2,$3,$4,$5)
      AND scope_kind='unbound' AND source_position IS NOT NULL AND (potential OR conflict)
      AND intent_id > $6 ORDER BY intent_id LIMIT 8";

async fn assert_indexes(observer: &mut PgConnection) -> Result<String> {
    let mut names: Vec<_> =
        LEAVES.iter().map(|(suffix, _)| format!("spot_order_set_{suffix}")).collect();
    names.push("spot_order_set_generation".to_owned());
    let indexes: Vec<(String, String, bool, bool, String, Vec<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT t.relname::text,ix.relname::text,i.indisprimary,i.indisunique,
                    am.amname::text,array_agg(a.attname::text ORDER BY k.n),
                    pg_get_expr(i.indpred,i.indrelid)
             FROM pg_index i
             JOIN pg_class t ON t.oid=i.indrelid
             JOIN pg_namespace ns ON ns.oid=t.relnamespace
             JOIN pg_class ix ON ix.oid=i.indexrelid
             JOIN pg_am am ON am.oid=ix.relam
             CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum,n)
             JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=k.attnum
             WHERE ns.nspname='public' AND t.relkind='r' AND t.relname=ANY($1::text[])
               AND i.indisvalid AND i.indisready
             GROUP BY t.relname,ix.relname,i.indisprimary,i.indisunique,am.amname,
                      pg_get_expr(i.indpred,i.indrelid)",
        )
        .bind(&names)
        .fetch_all(&mut *observer)
        .await?;
    ensure!(indexes.len() == 13, "actual index inventory: {indexes:?}");
    let generation_key = SCOPE[..3].to_vec();
    for (table, tail) in LEAVES {
        let name = format!("spot_order_set_{table}");
        let expected: Vec<_> = SCOPE.into_iter().chain(tail.iter().copied()).collect();
        ensure!(
            indexes
                .iter()
                .filter(|(relation, _, primary, unique, method, cols, predicate)| {
                    relation == &name
                        && *primary
                        && *unique
                        && method == "btree"
                        && cols == &expected
                        && predicate.is_none()
                })
                .count()
                == 1,
            "wrong physical owner key for {name}: {indexes:?}",
        );
    }
    ensure!(indexes.iter().any(|(relation, _, primary, unique, method, cols, predicate)| {
        relation == "spot_order_set_generation"
            && *primary
            && *unique
            && method == "btree"
            && cols == &generation_key
            && predicate.is_none()
    }));
    let (relation, hint, _, unique, method, cols, predicate) = indexes
        .iter()
        .find(|entry| entry.1 == "spot_order_set_sell_intents_unbound_hint")
        .context("missing native SELL hint index")?;
    let expected: Vec<_> = SCOPE.into_iter().chain(["source_position", "intent_id"]).collect();
    ensure!(
        relation == "spot_order_set_sell_intents"
            && !unique
            && method == "btree"
            && cols == &expected
            && predicate.is_some()
    );
    ensure!(
        indexes
            .iter()
            .any(|(relation, name, primary, unique, method, cols, predicate)| {
                relation == "spot_order_set_cancel_members"
                    && name == "idx_spot_cancel_unbound_original_position"
                    && !primary
                    && !unique
                    && method == "btree"
                    && cols == &expected
                    && predicate.as_deref()
                        == Some("((position_id IS NULL) AND (source_position IS NOT NULL))")
            }),
        "missing exact native CANCEL hint index"
    );
    let flags: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid
                 WHERE t.relname=ANY($1::text[]) AND t.relnamespace='public'::regnamespace
                   AND c.contype='f'),
                (SELECT count(*) FROM pg_trigger x JOIN pg_class t ON t.oid=x.tgrelid
                 WHERE t.relname=ANY($1::text[]) AND t.relnamespace='public'::regnamespace),
                (SELECT count(*) FROM pg_class t
                 WHERE t.relname=ANY($1::text[]) AND t.relnamespace='public'::regnamespace
                   AND t.relkind='r' AND t.relpersistence='p')",
    )
    .bind(&names)
    .fetch_one(&mut *observer)
    .await?;
    ensure!(flags == (0, 0, 11));
    for name in names {
        // Names derive only from this test's fixed schema inventory.
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM public.{name}"))
            .fetch_one(&mut *observer)
            .await?;
        ensure!(rows == 0, "M41 populated {name}");
    }
    Ok(hint.clone())
}

fn node_uses_index(node: &serde_json::Value, index: &str) -> bool {
    (node["Index Name"] == index && node["Actual Loops"] == 1)
        || node["Plans"]
            .as_array()
            .is_some_and(|children| children.iter().any(|child| node_uses_index(child, index)))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration41_expanded_keys_and_late_hint_page_follow_native_index(
    pool: PgPool,
) -> Result<()> {
    let mut f = Fixture::new(&pool).await?;
    f.adapter.unlock().await?;
    let mut holder = pool.begin().await?;
    sqlx::raw_sql("LOCK TABLE public.accounting_state IN ROW EXCLUSIVE MODE")
        .execute(&mut *holder)
        .await?;
    const OLD: &str = "SELECT version,description,installed_on::text,success,checksum,
        execution_time FROM _sqlx_migrations WHERE version < $1 ORDER BY version";
    let history = sqlx::query_as::<_, (i64, String, String, bool, Vec<u8>, i64)>(OLD)
        .bind(VERSION41)
        .fetch_all(&mut f.observer)
        .await?;
    ensure!(!history.is_empty());
    let slot = f.adapter.gate.clone();
    let mut stage = Some(stages::arm(&slot, Stage::Restore, Edge::AfterReturn)?);
    f.proxy.enable_exact_query_trace(50);
    let before = f.proxy.stats().statements;
    let observer = &mut f.observer;
    let pid = f.pid;
    let assert_stage = async {
        let lock_and_history =
            "SELECT (SELECT ARRAY[l.classid::bigint,l.objid::bigint] FROM pg_locks l
                     WHERE l.pid=$1 AND l.locktype='advisory' AND l.granted AND l.objsubid=1),
                    ARRAY(SELECT version FROM _sqlx_migrations
                          WHERE version >= $2 ORDER BY version)";
        let versions = [VERSION41, VERSION42, VERSION43, VERSION44, VERSION45];
        let mut original_key = None;
        for completed in 1..=versions.len() {
            let (hit, release) = stage.take().context("official migrate restore stage")?;
            hit.await?;
            let (key, landed): (Option<Vec<i64>>, Vec<i64>) = sqlx::query_as(lock_and_history)
                .bind(pid)
                .bind(VERSION41)
                .fetch_one(&mut *observer)
                .await?;
            let key = key.context("original native SQLx advisory key")?;
            ensure!(original_key.get_or_insert_with(|| key.clone()).as_slice() == key.as_slice());
            ensure!(landed == versions[..completed], "unexpected durable stage {completed}");
            if completed < versions.len() {
                stage = Some(stages::arm(&slot, Stage::Restore, Edge::AfterReturn)?);
            }
            release.send(()).map_err(|_| anyhow::anyhow!("apply restore gate closed"))?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let (run, evidence) = tokio::join!(
        crate::MIGRATOR.run_direct(&mut f.adapter),
        timeout(Duration::from_millis(1150), assert_stage),
    );
    run?;
    evidence.context("five native schema/advisory witness stages")??;
    let trace = f.proxy.query_trace();
    let statements = f.proxy.stats().statements - before;
    ensure!(statements == 42 && trace.len() == 42 && f.proxy.query_trace_dropped() == 0);
    ensure!(trace[1] == "SELECT pg_advisory_lock($1)");
    ensure!(
        trace[7].trim() == migration().sql.trim()
            && trace[14].trim() == projection_migration().sql.trim()
            && trace[21].trim() == cancel_migration().sql.trim()
            && trace[28].trim() == cancel_unbound_migration().sql.trim()
            && trace[35].trim() == contract45().context("exact M45")?.sql.trim()
    );
    ensure!(trace[41] == "SELECT pg_advisory_unlock($1)");
    let after = sqlx::query_as::<_, (i64, String, String, bool, Vec<u8>, i64)>(OLD)
        .bind(VERSION41)
        .fetch_all(&mut f.observer)
        .await?;
    ensure!(history == after, "the complete previous history has changed");
    f.assert_landed(true).await?;
    f.migration = projection_migration();
    f.assert_landed(true).await?;
    f.migration = cancel_migration();
    f.assert_landed(true).await?;
    f.migration = cancel_unbound_migration();
    f.assert_landed(true).await?;
    f.migration = contract45().context("exact M45")?;
    f.assert_landed(true).await?;
    let index = assert_indexes(&mut f.observer).await?;
    holder.rollback().await?;
    eprintln!("M41/M42/M43/M44/M45 bounded native official runner physical_sql={statements} native_same_key=true old_relation_lock_held=true");
    let t = Uuid::now_v7();
    let generation = Uuid::now_v7();
    let position = Uuid::now_v7();
    let other = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO public.spot_order_set_sell_intents
             (tenant_id,venue_id,account_scope,generation_id,intent_id,scope_kind,
              scope_position,scope_symbol,source_position,potential,conflict)
         SELECT $1,'binance:spot','fixture-expanded',$2,md5(i::text)::uuid,'unbound',
                NULL,NULL,md5(('different' || i)::text)::uuid,true,false
         FROM generate_series(1,8192) i",
    )
    .bind(t)
    .bind(generation)
    .execute(&mut f.observer)
    .await?;

    let seed = "INSERT INTO public.spot_order_set_sell_intents
        (tenant_id,venue_id,account_scope,generation_id,intent_id,scope_kind,
         scope_position,scope_symbol,source_position,potential,conflict)
        VALUES($1,'binance:spot',$2,$3,$4,$5,$6,$7,$8,$9,$10)";
    for n in 1..=10 {
        sqlx::query(seed)
            .bind(t)
            .bind("fixture-expanded")
            .bind(generation)
            .bind(Uuid::from_u128(n))
            .bind("unbound")
            .bind(None::<Uuid>)
            .bind(None::<&str>)
            .bind(position)
            .bind(n % 2 == 0)
            .bind(n % 2 == 1)
            .execute(&mut f.observer)
            .await?;
    }
    for (tenant, account, g, n, hint, potential) in [
        (t, "fixture-expanded", generation, 11, Some(position), false),
        (other, "fixture-expanded", generation, 12, Some(position), true),
        (t, "other-account", generation, 13, Some(position), true),
        (t, "fixture-expanded", other, 14, Some(position), true),
        (t, "fixture-expanded", generation, 15, None, true),
    ] {
        sqlx::query(seed)
            .bind(tenant)
            .bind(account)
            .bind(g)
            .bind(Uuid::from_u128(n))
            .bind("unbound")
            .bind(None::<Uuid>)
            .bind(None::<&str>)
            .bind(hint)
            .bind(potential)
            .bind(false)
            .execute(&mut f.observer)
            .await?;
    }
    sqlx::raw_sql("ANALYZE public.spot_order_set_sell_intents")
        .execute(&mut f.observer)
        .await?;

    for (after, end) in [(0, 8), (8, 10), (10, 10)] {
        let ids: Vec<Uuid> = sqlx::query_scalar(PAGE)
            .bind(t)
            .bind("binance:spot")
            .bind("fixture-expanded")
            .bind(generation)
            .bind(position)
            .bind(Uuid::from_u128(after))
            .fetch_all(&mut f.observer)
            .await?;
        ensure!(ids == ((after + 1)..=end).map(Uuid::from_u128).collect::<Vec<_>>());
    }
    let plan: serde_json::Value =
        sqlx::query_scalar(&format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {PAGE}"))
            .bind(t)
            .bind("binance:spot")
            .bind("fixture-expanded")
            .bind(generation)
            .bind(position)
            .bind(Uuid::nil())
            .fetch_one(&mut f.observer)
            .await?;
    ensure!(
        plan[0]["Plan"]["Actual Rows"] == 8 && node_uses_index(&plan[0]["Plan"], &index),
        "default native planner must naturally select the actual hint btree: {plan}",
    );
    eprintln!(
        "M41 expanded native index={} scoped_prefix=true active_and_conflict=true page_rows=8,2,0 source_noise=8192",
        index,
    );
    Ok(())
}
