//! Native first, replay, transfer and refusal controls for scoped SELL projection.
use super::*;

const SYMBOL: &str = "BTCUSDT";

/// Seed a scoped native leg through its admission function. This is a fixture
/// transition under the lock, not an operational generation/activation proof.
async fn seed_leg(pool: &PgPool, g: &JournalGeneration, position: Uuid) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT version FROM public.accounting_state
         WHERE tenant_id=$1 AND venue_id='binance:spot' FOR UPDATE",
    )
    .bind(g.tenant_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO public.leg_routes (position_id,tenant_id,venue_id,account_scope,routed_at)
         VALUES ($1,$2,'binance:spot',$3,now())",
    )
    .bind(position)
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "SELECT public.balance_leg_admit($1,'binance:spot',$2,$3,$4,$5,'2026-11-01',
            10,10,2,10,0.001,'base','fixture',now(),'bound-witness',1,1,100,0.1,10,90,now(),false)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(SYMBOL)
    .bind(position)
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

type Member = (String, Option<String>, Option<Uuid>, Option<Uuid>, bool, bool, String);
type LegCounts = (i64, i64, String);

async fn member(pool: &PgPool, g: &JournalGeneration, intent: Uuid) -> Result<Member> {
    Ok(sqlx::query_as(
        "SELECT scope_kind,scope_symbol,scope_position,source_position,potential,conflict,xmin::text
         FROM public.spot_order_set_sell_intents
         WHERE (tenant_id,venue_id,account_scope,generation_id,intent_id)
            = ($1,'binance:spot',$2,$3,$4)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .bind(intent)
    .fetch_one(pool)
    .await?)
}

async fn counts(pool: &PgPool, g: &JournalGeneration, position: Uuid) -> Result<LegCounts> {
    Ok(sqlx::query_as(
        "SELECT potential_count,conflict_count,xmin::text
         FROM public.spot_order_set_sell_legs
         WHERE (tenant_id,venue_id,account_scope,generation_id,symbol,position_id)
            = ($1,'binance:spot',$2,$3,$4,$5)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .bind(SYMBOL)
    .bind(position)
    .fetch_one(pool)
    .await?)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_bound_protection_replays_and_retains_inactive_zero(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let mut j = sell();
    j.intent.kind = IntentKind::Protection;
    let position = j.intent.position_id.context("test position")?;
    seed_leg(&pool, &g, position).await?;
    let (store, proxy, _, _) = proxied_journal(&pool, &g).await?;
    let version = fields(&pool, &g).await?.0;
    let id = j.intent.intent_id;
    insist(store.commit(&j).await)?;
    let trace = proxy.query_trace();
    ensure!(trace.len() == 6 && proxy.query_trace_dropped() == 0);
    ensure!(trace[4].contains("UPDATE public.spot_order_set_generation g") && trace[5] == "COMMIT");
    let active = member(&pool, &g, id).await?;
    ensure!(active.0 == "leg" && active.1.as_deref() == Some(SYMBOL));
    ensure!(active.2 == Some(position) && active.3 == Some(position) && active.4 && !active.5);
    let projected = counts(&pool, &g, position).await?;
    ensure!((projected.0, projected.1) == (1, 0));
    ensure!(fields(&pool, &g).await? == (version + 1, version + 1, 0, 0, 1, 0));
    insist(store.commit(&j).await)?;
    ensure!(member(&pool, &g, id).await? == active);
    ensure!(counts(&pool, &g, position).await? == projected);
    ensure!(fields(&pool, &g).await? == (version + 2, version + 2, 0, 0, 1, 0));

    j.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    insist(store.commit(&j).await)?;
    let inactive = member(&pool, &g, id).await?;
    ensure!(inactive.0 == "none" && inactive.1.is_none() && inactive.2.is_none());
    ensure!(inactive.3 == Some(position) && !inactive.4 && !inactive.5);
    let zero = counts(&pool, &g, position).await?;
    ensure!((zero.0, zero.1) == (0, 0));
    insist(store.commit(&j).await)?;
    ensure!(member(&pool, &g, id).await? == inactive && counts(&pool, &g, position).await? == zero);
    ensure!(fields(&pool, &g).await? == (version + 4, version + 4, 0, 0, 0, 0));
    let unbound: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.spot_order_set_sell_unbound WHERE tenant_id=$1",
    )
    .bind(g.tenant_id)
    .fetch_one(&pool)
    .await?;
    ensure!(unbound == 0 && j_present(&pool, id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_bound_exit_transfers_and_refuses_deficient_prior(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let mut j = sell();
    let position = j.intent.position_id.context("test position")?;
    let id = j.intent.intent_id;
    let store = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let version = fields(&pool, &g).await?.0;
    insist(store.commit(&j).await)?;
    ensure!(fields(&pool, &g).await? == (version + 1, version + 1, 1, 0, 1, 0));
    seed_leg(&pool, &g, position).await?;
    insist(store.commit(&j).await)?;
    let bound = member(&pool, &g, id).await?;
    ensure!(bound.0 == "leg" && bound.1.as_deref() == Some(SYMBOL));
    ensure!(bound.2 == Some(position) && bound.3 == Some(position) && bound.4 && !bound.5);
    let zero: (i64, i64) = sqlx::query_as(
        "SELECT potential_count,conflict_count FROM public.spot_order_set_sell_unbound
         WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .fetch_one(&pool)
    .await?;
    ensure!(zero == (0, 0));
    ensure!(fields(&pool, &g).await? == (version + 2, version + 2, 0, 0, 1, 0));

    // An authorized corruption control in this isolated database. Zero net
    // delta on exact replay cannot conceal the missing prior contribution.
    sqlx::query(
        "UPDATE public.spot_order_set_sell_legs SET potential_count=0
         WHERE (tenant_id,venue_id,account_scope,generation_id,symbol,position_id)
             = ($1,'binance:spot',$2,$3,$4,$5)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .bind(SYMBOL)
    .bind(position)
    .execute(&pool)
    .await?;
    let deficient = counts(&pool, &g, position).await?;
    ensure!((deficient.0, deficient.1) == (0, 0));
    ensure!(matches!(store.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(counts(&pool, &g, position).await? == deficient);
    j.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    ensure!(matches!(store.commit(&j).await, Err(CommitFailure::Known(_))));
    let unchanged: bool =
        sqlx::query_scalar("SELECT NOT not_sent FROM public.order_intents WHERE intent_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    ensure!(unchanged && member(&pool, &g, id).await? == bound);
    ensure!(fields(&pool, &g).await? == (version + 2, version + 2, 0, 0, 1, 0));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL and a native clock"]
async fn persisted_order_set_journal_bound_clocked_reverse_link_rolls_back(
    pool: PgPool,
) -> Result<()> {
    let clock = native_clock(&pool).await?;
    let g = seed_generation(&pool).await?;
    let mut j = sell();
    j.intent.kind = IntentKind::Protection;
    let position = j.intent.position_id.context("test position")?;
    let id = j.intent.intent_id;
    seed_leg(&pool, &g, position).await?;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let earlier = fields(&pool, &g).await?;
    let input = clocked_attempt(&clock, &g, &mut j)?;
    let attempt = j.attempts[0].attempt_id;
    let receipts = stored
        .commit_clocked(&j, &input)
        .await
        .map_err(|error| anyhow::anyhow!("first scoped clocked journal: {error:?}"))?;
    ensure!(receipts.len() == 1);
    let projected = counts(&pool, &g, position).await?;
    ensure!((projected.0, projected.1) == (1, 0));
    let bound = member(&pool, &g, id).await?;
    ensure!(bound.0 == "leg" && bound.2 == Some(position));
    let version = fields(&pool, &g).await?;
    ensure!(version == (earlier.0 + 1, earlier.1 + 1, 0, 0, 1, 0));

    // Install exact native reverse evidence in the fixture. No journal result
    // can use this narrower writer to infer linked SELL and CANCEL effects.
    sqlx::query(
        "INSERT INTO public.venue_orders
         (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,
          last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,
          stop_price,status,phase,last_checked_at,live_seen_at)
         VALUES ('binance:spot',$1,$2,'991105',$3,$4,$4,'leg',$5,$6,'stop','SELL','STOP_LOSS',
                 1,90,'NEW','live',now(),now())",
    )
    .bind(&g.account_scope)
    .bind(SYMBOL)
    .bind(g.tenant_id)
    .bind(&j.intent.client_order_id)
    .bind(id)
    .bind(position)
    .execute(&pool)
    .await?;
    j.attempts[0].rejected_at = Some(Utc::now());
    j.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    ensure!(matches!(stored.commit_clocked(&j, &input).await, Err(CommitFailure::Known(_))));
    let untouched: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.submit_attempts
                        WHERE attempt_id=$1 AND state='started' AND rejected_at IS NULL)
             AND EXISTS (SELECT 1 FROM public.order_intents WHERE intent_id=$2
                         AND venue_outcome IS NULL AND venue_evidence IS NULL)
             AND EXISTS (SELECT 1 FROM public.spot_intent_clock WHERE attempt_id=$1 AND intent_id=$2)",
    )
    .bind(attempt)
    .bind(id)
    .fetch_one(&pool)
    .await?;
    ensure!(untouched);
    ensure!(fields(&pool, &g).await? == version);
    ensure!(
        member(&pool, &g, id).await? == bound && counts(&pool, &g, position).await? == projected
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_foreign_scoped_leg_keeps_raw_hint_unbound(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let j = sell();
    let position = j.intent.position_id.context("test position")?;
    let foreign = JournalGeneration {
        account_scope: "other-account".to_owned(),
        ..g.clone()
    };
    seed_leg(&pool, &foreign, position).await?;
    let store = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let before = fields(&pool, &g).await?;
    insist(store.commit(&j).await)?;
    let projected = member(&pool, &g, j.intent.intent_id).await?;
    ensure!(projected.0 == "unbound" && projected.1.is_none() && projected.2.is_none());
    ensure!(projected.3 == Some(position) && projected.4 && !projected.5);
    ensure!(fields(&pool, &g).await? == (before.0 + 1, before.1 + 1, 1, 0, 1, 0));
    Ok(())
}
