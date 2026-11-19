use std::time::Duration;

use super::*;

async fn shadow_equivalence(pool: &PgPool, b: &JournalGeneration) -> Result<()> {
    for table in ["members", "groups", "legs"] {
        let forecast = format!(
            "SELECT row_to_json(s)::jsonb::text FROM cancel_shadow_{table} s
           WHERE s.tenant_id=$1 AND s.account_scope=$2 ORDER BY row_to_json(s)::jsonb::text"
        );
        let projected = format!(
            "SELECT (to_jsonb(p)-'generation_id')::text FROM spot_order_set_cancel_{table} p
           WHERE p.tenant_id=$1 AND p.account_scope=$2 AND p.generation_id=$3
           ORDER BY (to_jsonb(p)-'generation_id')::text"
        );
        let expected: Vec<String> = sqlx::query_scalar(&forecast)
            .bind(b.tenant_id)
            .bind(&b.account_scope)
            .fetch_all(pool)
            .await?;
        let actual: Vec<String> = sqlx::query_scalar(&projected)
            .bind(b.tenant_id)
            .bind(&b.account_scope)
            .bind(b.generation_id)
            .fetch_all(pool)
            .await?;
        ensure!(actual == expected, "cancel {table}: actual/expected {actual:?} {expected:?}");
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_matches_shadow_siblings_and_physical_replay(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    sqlx::raw_sql(include_str!("../tests/spot_order_set_reference_pg/cancel_shadow.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(
        "CREATE TABLE cancel_projection_writes(name text);
         CREATE FUNCTION watch_projection_result() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO cancel_projection_writes VALUES(TG_TABLE_NAME); RETURN NEW; END $$;
         CREATE TRIGGER cancel_projection_member AFTER UPDATE OR INSERT ON spot_order_set_cancel_members
           FOR EACH ROW EXECUTE FUNCTION watch_projection_result();
         CREATE TRIGGER cancel_projection_group AFTER UPDATE OR INSERT ON spot_order_set_cancel_groups
           FOR EACH ROW EXECUTE FUNCTION watch_projection_result();
         CREATE TRIGGER cancel_projection_leg AFTER UPDATE OR INSERT ON spot_order_set_cancel_legs
           FOR EACH ROW EXECUTE FUNCTION watch_projection_result()",
    ).execute(&pool).await?;
    for masked in [false, true] {
        let (_, b, mut batch, _) = prepared(&pool, masked).await?;
        let id = batch.intent.intent_id;
        if masked {
            sqlx::query(
                "INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,
                     cancels_order_id,params,created_at)
                 SELECT md5(intent_id::text||i)::uuid,tenant_id,venue_id,account_scope,'cancel-sibling-'||i,position_id,kind,
                        cancels_order_id,params,created_at
                 FROM order_intents CROSS JOIN generate_series(1,20)i WHERE intent_id=$1",
            ).bind(id).execute(&pool).await?;
            sqlx::query(
                "INSERT INTO spot_order_set_cancel_members
                 (tenant_id,venue_id,account_scope,generation_id,intent_id,symbol,position_id,target_id,pending,conflict)
                 SELECT tenant_id,venue_id,account_scope,$3,intent_id,symbol,position_id,target_id,pending,conflict
                 FROM cancel_shadow_members WHERE tenant_id=$1 AND intent_id<>$2",
            ).bind(b.tenant_id).bind(id).bind(b.generation_id).execute(&pool).await?;
            sqlx::query(
                "UPDATE spot_order_set_cancel_groups SET pending_count=21 WHERE generation_id=$1",
            )
            .bind(b.generation_id)
            .execute(&pool)
            .await?;
        }
        shadow_equivalence(&pool, &b).await?;
        let ledger = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
        batch.outcome = outcome(true, None);
        ledger.commit(&batch).await.map_err(|err| anyhow::anyhow!("{err:?}"))?;
        shadow_equivalence(&pool, &b).await?;
        let leg_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM spot_order_set_cancel_legs WHERE generation_id=$1",
        )
        .bind(b.generation_id)
        .fetch_one(&pool)
        .await?;
        ensure!(leg_rows == i64::from(!masked));

        batch.outcome = Some(OutcomeRow {
            venue_evidence: Some(serde_json::json!({"fixture":"seen"})),
            ..OutcomeRow::default()
        });
        ledger.commit(&batch).await.map_err(|err| anyhow::anyhow!("{err:?}"))?;
        shadow_equivalence(&pool, &b).await?;
        let state = read_facts(&pool, &b, id).await?;
        ensure!(
            state.pending
                && state.conflict
                && state.native_outcome.is_none()
                && state.group_conflict == 1
        );
        ensure!(state.pending_count == if masked { 21 } else { 1 });
        sqlx::query("TRUNCATE cancel_projection_writes").execute(&pool).await?;
        ledger.commit(&batch).await.map_err(|err| anyhow::anyhow!("{err:?}"))?;
        let physical: i64 = sqlx::query_scalar("SELECT count(*) FROM cancel_projection_writes")
            .fetch_one(&pool)
            .await?;
        ensure!(physical == 0);
        shadow_equivalence(&pool, &b).await?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_refuses_wrong_aggregate_or_required_projection_write(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, v) = prepared(&pool, false).await?;
    let id = batch.intent.intent_id;
    let before = durable(&pool, b.tenant_id).await?;
    let original = read_facts(&pool, &b, id).await?;
    batch.outcome = outcome(true, None);
    let ledger = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    let wrong =
        PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(JournalGeneration {
            source_audit_id: Uuid::now_v7(),
            ..b.clone()
        });
    ensure!(matches!(wrong.commit(&batch).await, Err(CommitFailure::Known(_))));
    let fresh = JournalBatch {
        intent: IntentRow {
            intent_id: Uuid::now_v7(),
            client_order_id: "fresh-cancel".into(),
            ..batch.intent.clone()
        },
        ..batch.clone()
    };
    ensure!(matches!(ledger.commit(&fresh).await, Err(CommitFailure::Known(_))));
    let unbound = JournalBatch {
        intent: IntentRow {
            position_id: None,
            ..batch.intent.clone()
        },
        ..batch.clone()
    };
    ensure!(matches!(ledger.commit(&unbound).await, Err(CommitFailure::Known(_))));
    let mut attempted = batch.clone();
    attempted.attempts.push(robson_exec::order_journal::AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: batch.intent.created_at,
        rejected_at: None,
    });
    ensure!(matches!(ledger.commit(&attempted).await, Err(CommitFailure::Known(_))));
    for (fault,repair,key) in [
        ("UPDATE spot_order_set_cancel_members SET target_id='999' WHERE intent_id=$1",
         "UPDATE spot_order_set_cancel_members SET target_id='4100001' WHERE intent_id=$1",id),
        ("DELETE FROM spot_order_set_cancel_groups WHERE generation_id=$1",
         "INSERT INTO spot_order_set_cancel_groups
          (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,pending_count,conflict_count,terminal_mask)
          SELECT tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,1,0,false
          FROM spot_order_set_cancel_members WHERE generation_id=$1",b.generation_id),
        ("UPDATE spot_order_set_generation SET accounting_version=accounting_version-1 WHERE generation_id=$1",
         "UPDATE spot_order_set_generation SET accounting_version=accounting_version+1 WHERE generation_id=$1",b.generation_id),
    ] {
        sqlx::query(fault).bind(key).execute(&pool).await?;
        ensure!(matches!(ledger.commit(&batch).await,Err(CommitFailure::Known(_))));
        ensure!(durable(&pool,b.tenant_id).await?==before);
        sqlx::query(repair).bind(key).execute(&pool).await?;
        ensure!(read_facts(&pool,&b,id).await?==original);
    }

    sqlx::raw_sql(
        "CREATE FUNCTION suppress_cancel_group_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RETURN NULL; END $$;
         CREATE TRIGGER suppress_cancel_group_fixture BEFORE UPDATE ON spot_order_set_cancel_groups
         FOR EACH ROW EXECUTE FUNCTION suppress_cancel_group_fixture()",
    )
    .execute(&pool)
    .await?;
    ensure!(matches!(ledger.commit(&batch).await, Err(CommitFailure::Known(_))));
    ensure!(
        durable(&pool, b.tenant_id).await? == before
            && read_facts(&pool, &b, id).await? == original
    );
    sqlx::query("DROP TRIGGER suppress_cancel_group_fixture ON spot_order_set_cancel_groups")
        .execute(&pool)
        .await?;
    ledger.commit(&batch).await.map_err(|err| anyhow::anyhow!("{err:?}"))?;
    ensure!(read_facts(&pool, &b, id).await?.accounting == v + 1);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_clocked_result_has_no_clock_and_respects_physical_deadline(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, v) = prepared(&pool, false).await?;
    let id = batch.intent.intent_id;
    let mut wire = Wire::new(&pool, b.tenant_id).await?;
    wire.store.order_set_projection = Some(b.clone());
    let input = clocked::ClockedInput {
        process_epoch: b.process_epoch,
        attempts: Vec::new(),
    };
    batch.outcome = outcome(false, Some(VenueOutcome::Success));
    let acknowledgement = wire
        .store
        .commit_clocked(&batch, &input)
        .await
        .map_err(|err| anyhow::anyhow!("clocked ordinary observation: {err:?}"))?;
    ensure!(acknowledgement.is_empty());
    let trace = wire.proxy.query_trace();
    ensure!(trace.len() == 7 && trace.last().is_some_and(|sql| sql == "COMMIT"), "{trace:?}");
    ensure!(!trace
        .iter()
        .any(|sql| sql.contains("budget_insert_clocks")
            || sql.contains("INSERT INTO spot_intent_clock")));
    let committed = read_facts(&pool, &b, id).await?;
    ensure!(committed.accounting == v + 1 && committed.projected == v + 1 && committed.pending);
    let baseline = durable(&pool, b.tenant_id).await?;

    sqlx::raw_sql(
        "CREATE FUNCTION hold_grouped_clocked_projection() RETURNS trigger LANGUAGE plpgsql AS $$
           BEGIN PERFORM pg_advisory_xact_lock(719372); RETURN NEW; END $$;
         CREATE TRIGGER hold_grouped_clocked_projection AFTER UPDATE OF not_sent ON order_intents
         FOR EACH ROW EXECUTE FUNCTION hold_grouped_clocked_projection()",
    )
    .execute(&pool)
    .await?;
    let mut barrier = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(719372)").execute(&mut *barrier).await?;
    batch.outcome = outcome(true, None);
    wire.proxy.enable_exact_query_trace(16);
    let watcher =
        async {
            tokio::time::timeout(Duration::from_millis(300), async {
                loop {
                    let waiting:bool=sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                        AND pid<>pg_backend_pid() AND query LIKE '%UPDATE order_intents SET%'
                        AND wait_event='advisory')",
                ).fetch_one(&pool).await?;
                    if waiting {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Ok::<_, sqlx::Error>(())
            })
            .await??;
            ensure!(read_facts(&pool, &b, id).await? == committed);
            wire.proxy.set_mode(ProxyMode::Blackhole);
            Ok::<_, anyhow::Error>(())
        };
    let (blocked, observed) = tokio::join!(wire.store.commit_clocked(&batch, &input), watcher);
    observed?;
    ensure!(
        matches!(blocked,Err(CommitFailure::Known(reason)) if reason.contains("deadline before COMMIT"))
    );
    wire.proxy.set_mode(ProxyMode::Pass);
    wire.retires(&pool).await?;
    ensure!(wire
        .proxy
        .query_trace()
        .iter()
        .all(|q| q != "COMMIT" && !q.contains("budget_insert_clocks")));
    ensure!(
        durable(&pool, b.tenant_id).await? == baseline
            && read_facts(&pool, &b, id).await? == committed
    );
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(719372)")
        .fetch_one(&mut *barrier)
        .await?;
    ensure!(released);
    Ok(())
}
