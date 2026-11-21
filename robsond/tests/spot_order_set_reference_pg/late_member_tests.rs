//! Real J orders and typed admission in the test-only initialized shadow.
use std::sync::Arc;

use robson_exec::{
    order_journal::{AdmissionRow, COMMIT_BOUND},
    venue::VenueId,
};
use rust_decimal_macros::dec;
use sqlx::{
    pool::PoolConnection,
    types::chrono::{DateTime, NaiveDate, Utc},
    PgConnection, Postgres,
};

use super::*;

const REPAIR: &str = "SELECT sell_shadow_repair_exact($1,$2,$3,$4)";

async fn bounded_write(journal: &PgJournalBackend, batch: &JournalBatch) -> anyhow::Result<()> {
    timeout(COMMIT_BOUND, journal_commit(journal, batch))
        .await
        .context("actual fixture journal reached its 500ms ceiling")?
}

async fn admit_late_leg(
    fixture: &Fixture,
    journal: &PgJournalBackend,
    leg: Scope,
) -> anyhow::Result<Uuid> {
    fixture
        .ledger
        .insert_route(leg.position, &VenueId::new(leg.venue), leg.account, seen(0).at)
        .await?;
    let mut entry = batch(leg, IntentKind::Entry, None);
    entry.balance.as_mut().context("ordinary balance entry batch")?.admission =
        Some(Arc::new(AdmissionRow {
            position_id: leg.position,
            symbol: leg.symbol.into(),
            decided_month: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            taker_bps: dec!(10),
            gap_bps: dec!(10),
            notional_margin_bps: dec!(200),
            min_notional: dec!(5),
            lot_step: dec!(0.01),
            fee_asset_policy: "base".into(),
            envelope_source: "test rules".into(),
            envelope_as_of: seen(0).at,
            envelope_hash: "late-member-admission-fixture".into(),
            planned_qty: dec!(1),
            requested_qty: dec!(1.01),
            reference_price: dec!(100),
            remainder_reserve: dec!(0.1),
            admission_charge: dec!(12),
            // Synthetic second 15-minute support from the existing fixture.
            initial_trigger: dec!(90),
        }));
    bounded_write(journal, &entry).await?;
    Ok(entry.intent.intent_id)
}

async fn repair(actor: &mut PgConnection, owner: Scope, intent: Uuid) -> anyhow::Result<bool> {
    Ok(timeout(
        COMMIT_BOUND,
        sqlx::query_scalar(REPAIR)
            .bind(owner.tenant)
            .bind(owner.venue)
            .bind(owner.account)
            .bind(intent)
            .fetch_one(actor),
    )
    .await
    .context("exact local repair reached the caller ceiling")??)
}

fn exact_unbound(mut ids: Vec<Uuid>) -> Vec<(Uuid, bool)> {
    ids.sort_unstable();
    ids.into_iter().map(|id| (id, false)).collect()
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn exact_late_sell_member_moves_only_the_identified_native_leg(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let fixture = Fixture::new(&pool).await;
    let owner = scope(&fixture);
    let journal = PgJournalBackend::new(&pool, owner.tenant);
    let late = Scope { position: Uuid::now_v7(), ..owner };
    let missing = Scope { position: Uuid::now_v7(), ..owner };
    let first = batch(late, IntentKind::Exit, None);
    let other = batch(missing, IntentKind::Exit, None);
    let first_id = first.intent.intent_id;
    let other_id = other.intent.intent_id;
    bounded_write(&journal, &first).await?;
    bounded_write(&journal, &other).await?;

    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    let generation = ready.generation();
    let original = check(&pool, &ready, owner).await?;
    let before = exact_unbound(vec![first_id, other_id]);
    ensure!(
        original.unbound_sell == 2 && unbound_sell_identities(&pool, owner).await? == before,
        "before the real leg both same-account SELLs must be held"
    );
    let no_leg: (bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM public.balance_legs WHERE position_id=$1),
                EXISTS(SELECT 1 FROM public.balance_legs WHERE position_id=$2)",
    )
    .bind(late.position)
    .bind(missing.position)
    .fetch_one(&pool)
    .await?;
    ensure!(no_leg == (false, false), "neither unresolved member initially has a leg");

    let admitted = admit_late_leg(&fixture, &journal, late).await?;
    let real: (Uuid, String) = sqlx::query_as(
        "SELECT admission_intent_id,symbol FROM public.balance_legs
         WHERE (position_id,tenant_id,venue_id,account_scope)=($1,$2,$3,$4)",
    )
    .bind(late.position)
    .bind(owner.tenant)
    .bind(owner.venue)
    .bind(owner.account)
    .fetch_one(&pool)
    .await?;
    ensure!(real == (admitted, late.symbol.into()), "typed leg source after real commit");

    let warm = ready
        .read(&pool, late, None)
        .await?
        .context("fenced warm late leg after admission")?;
    let cold = audit(&pool, late).await.map_err(|e| anyhow!("cold late ledger: {e:?}"))?;
    ensure!(
        cold.rows.iter().any(|row| row.intent_id == first_id)
            && cold.rows.iter().all(|row| row.intent_id != other_id)
            && cold.version == warm.facts.accounting_version
            && cold_sells(&cold).leg.active == 1
            && warm.facts.leg_sell == 0
            && warm.unbound_sell == 2
            && unbound_sell_identities(&pool, owner).await? == before,
        "after admission cold targets the exact member while warm remains conservative"
    );

    let (proxy, actor_pool) = through_proxy_limit(&pool, 1).await;
    let mut actor = actor_pool.acquire().await?;
    sqlx::query("SELECT set_config('robson_test.sell_shadow_trace','on',false)")
        .execute(&mut *actor)
        .await?;
    ensure!(
        !repair(&mut actor, Scope { account: "wrong-scope", ..late }, first_id).await?
            && !repair(&mut actor, Scope { tenant: Uuid::now_v7(), ..late }, first_id).await?
            && !repair(&mut actor, late, other_id).await?,
        "wrong owner or a different unresolved position cannot clear a hold"
    );
    ensure!(unbound_sell_identities(&pool, owner).await? == before);

    sqlx::query("INSERT INTO sell_shadow_fail_member(member_key) VALUES($1)")
        .bind(first_id.to_string())
        .execute(&pool)
        .await?;
    let failure = repair(&mut actor, late, first_id)
        .await
        .expect_err("injected exact member abort");
    let denied = failure.downcast_ref::<sqlx::Error>().and_then(sqlx::Error::as_database_error);
    ensure!(
        denied.is_some_and(|db| db.code().as_deref() == Some("P0001")
            && db.message() == "sell shadow member rollback witness"),
        "repair failed at the intended native rollback point"
    );
    let unchanged = ready.read(&pool, late, None).await?.context("warm after repair rollback")?;
    ensure!(
        unchanged.facts.leg_sell == 0
            && unchanged.unbound_sell == 2
            && unbound_sell_identities(&pool, owner).await? == before,
        "repair rollback must keep both original member and aggregate holds"
    );
    sqlx::query("DELETE FROM sell_shadow_fail_member WHERE member_key=$1")
        .bind(first_id.to_string())
        .execute(&pool)
        .await?;

    proxy.enable_exact_query_trace(4);
    let wire = proxy.stats();
    ensure!(repair(&mut actor, late, first_id).await?, "exact late leg must repair");
    ensure!(
        proxy.stats().statements - wire.statements == 1
            && proxy.stats().commits == wire.commits
            && proxy.query_trace() == [REPAIR]
            && proxy.query_trace_dropped() == 0,
        "the identified repair is one native autocommit statement"
    );
    let repaired = check(&pool, &ready, late).await?;
    let still_held = exact_unbound(vec![other_id]);
    ensure!(
        repaired.facts.leg_sell == 1
            && repaired.unbound_sell == 1
            && repaired.facts.accounting_version == cold.version
            && unbound_sell_identities(&pool, owner).await? == still_held,
        "only the target moves to its exact leg and the other hold survives"
    );
    ensure!(repair(&mut actor, late, first_id).await?, "exact repair replay");
    let replay = check(&pool, &ready, late).await?;
    ensure!(
        replay.facts.leg_sell == 1
            && replay.unbound_sell == 1
            && ready.generation() == generation
            && unbound_sell_identities(&pool, owner).await? == still_held,
        "idempotent repair preserves original generation and other holds"
    );

    let measured: Vec<(i32, i32, i32)> = sqlx::query_as(
        "SELECT probes,member_rows,aggregate_rows FROM sell_shadow_work
         WHERE source='late_exact_repair' AND intent_id=$1 ORDER BY work_id",
    )
    .bind(first_id)
    .fetch_all(&pool)
    .await?;
    ensure!(
        measured == [(7, 1, 2)],
        "one membership/aggregate move; exact replay is read-only: {measured:?}"
    );
    eprintln!("SPOT-LATE-MEMBER repair_one_wire journal_bound_ms=500 work={measured:?}");
    drop(actor);
    actor_pool.close().await;
    Ok(())
}

type Backend = (i32, DateTime<Utc>);

async fn backend(conn: &mut PgConnection) -> anyhow::Result<Backend> {
    Ok(sqlx::query_as(
        "SELECT pid,backend_start FROM pg_stat_activity
         WHERE pid=pg_backend_pid() AND datname=current_database()",
    )
    .fetch_one(conn)
    .await?)
}

/// Blockers of one exact backend while it lock-waits inside the repair
/// statement.
async fn repair_blockers(pool: &PgPool, who: Backend) -> anyhow::Result<Option<Vec<i32>>> {
    Ok(sqlx::query_scalar(
        "SELECT pg_blocking_pids(a.pid) FROM pg_stat_activity a
         WHERE a.pid=$1 AND a.backend_start=$2 AND a.datname=current_database()
           AND a.state='active' AND a.wait_event_type='Lock' AND a.query=$3",
    )
    .bind(who.0)
    .bind(who.1)
    .bind(REPAIR)
    .fetch_optional(pool)
    .await?)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn exact_late_sell_member_competing_repairs_serialize_once(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let fixture = Fixture::new(&pool).await;
    let owner = scope(&fixture);
    let journal = PgJournalBackend::new(&pool, owner.tenant);
    let late = Scope { position: Uuid::now_v7(), ..owner };
    let missing = Scope { position: Uuid::now_v7(), ..owner };
    let first = batch(late, IntentKind::Exit, None);
    let other = batch(missing, IntentKind::Exit, None);
    let first_id = first.intent.intent_id;
    let other_id = other.intent.intent_id;
    bounded_write(&journal, &first).await?;
    bounded_write(&journal, &other).await?;

    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    let generation = ready.generation();
    let before = exact_unbound(vec![first_id, other_id]);
    admit_late_leg(&fixture, &journal, late).await?;
    let warm = ready
        .read(&pool, late, None)
        .await?
        .context("fenced warm late leg after admission")?;
    let cold = audit(&pool, late).await.map_err(|e| anyhow!("cold late ledger: {e:?}"))?;
    ensure!(
        cold.version == warm.facts.accounting_version
            && warm.facts.leg_sell == 0
            && warm.unbound_sell == 2
            && unbound_sell_identities(&pool, owner).await? == before,
        "before the race both original holds remain"
    );

    let mut contenders: Vec<(PoolConnection<Postgres>, Backend)> = Vec::new();
    for _ in 0..2 {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT set_config('robson_test.sell_shadow_trace','on',false)")
            .execute(&mut *conn)
            .await?;
        let id = backend(&mut conn).await?;
        contenders.push((conn, id));
    }
    let (mut b, b_id) = contenders.pop().context("second contender")?;
    let (mut a, a_id) = contenders.pop().context("first contender")?;
    ensure!(a_id.0 != b_id.0, "contenders are distinct native sessions");

    let mut holder = pool.begin().await?;
    let holder_id = backend(&mut holder).await?;
    sqlx::query("SELECT 1 FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2 FOR UPDATE")
        .bind(owner.tenant)
        .bind(owner.venue)
        .fetch_one(&mut *holder)
        .await?;

    let observe_and_release = async {
        let queue = [holder_id.0, a_id.0, b_id.0];
        let observed = timeout(Duration::from_millis(350), async {
            loop {
                let a_waits = repair_blockers(&pool, a_id).await?;
                let b_waits = repair_blockers(&pool, b_id).await?;
                if let (Some(x), Some(y)) = (a_waits, b_waits) {
                    let queued = |own: i32, by: &[i32]| {
                        !by.is_empty() && by.iter().all(|pid| *pid != own && queue.contains(pid))
                    };
                    if queued(a_id.0, &x)
                        && queued(b_id.0, &y)
                        && (x.contains(&holder_id.0) || y.contains(&holder_id.0))
                    {
                        break anyhow::Ok((x, y));
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .context("both contenders never observed queued behind the holder");
        holder.rollback().await?;
        observed?
    };
    // Each repair has the original 500ms ceiling from its first poll,
    // including the witnessed lock wait. No detached repair survives the test.
    let (observed, a_done, b_done) = tokio::join!(
        observe_and_release,
        repair(&mut a, late, first_id),
        repair(&mut b, late, first_id)
    );
    let (a_waits, b_waits) = observed?;
    ensure!(a_done? && b_done?, "both competing repairs acknowledge idempotently");

    let repaired = check(&pool, &ready, late).await?;
    let after_cold = audit(&pool, late).await.map_err(|e| anyhow!("cold after race: {e:?}"))?;
    ensure!(
        repaired.facts.leg_sell == 1
            && repaired.unbound_sell == 1
            && repaired.facts.accounting_version == cold.version
            && after_cold.version == cold.version
            && cold_sells(&after_cold).leg.active == 1
            && ready.generation() == generation
            && unbound_sell_identities(&pool, owner).await? == exact_unbound(vec![other_id]),
        "the race moves only the target once and the other hold survives"
    );
    let measured: Vec<(i32, i32, i32)> = sqlx::query_as(
        "SELECT probes,member_rows,aggregate_rows FROM sell_shadow_work
         WHERE source='late_exact_repair' ORDER BY work_id",
    )
    .fetch_all(&pool)
    .await?;
    ensure!(
        measured == [(7, 1, 2)],
        "exactly one membership/aggregate move under race: {measured:?}"
    );
    eprintln!(
        "SPOT-LATE-MEMBER competing_repairs holder={} a={a_waits:?} b={b_waits:?} work={measured:?}",
        holder_id.0
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn exact_late_sell_member_native_arrival_owns_the_same_member(
    pool: PgPool,
) -> anyhow::Result<()> {
    use robson_exec::venue_ledger::OrderLink;

    empty_budget::initialize(&pool, seen(0).at).await?;
    let fixture = Fixture::new(&pool).await;
    let owner = scope(&fixture);
    let late = Scope { position: Uuid::now_v7(), ..owner };
    let journal = PgJournalBackend::new(&pool, owner.tenant);
    let sell = batch(late, IntentKind::Exit, None);
    let intent = sell.intent.intent_id;
    bounded_write(&journal, &sell).await?;

    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    let generation = ready.generation();
    let original = check(&pool, &ready, owner).await?;
    ensure!(original.unbound_sell == 1);
    admit_late_leg(&fixture, &journal, late).await?;
    let warm = ready.read(&pool, late, None).await?.context("warm late leg")?;
    ensure!(warm.unbound_sell == 1 && warm.facts.leg_sell == 0);

    let key = "992001";
    let mut observed = zero_execution_order(key, "NEW", OrderPhase::Live);
    observed.client_order_id.clone_from(&sell.intent.client_order_id);
    let link = OrderLink {
        owner: OrderOwner::Leg {
            position_id: late.position,
            intent_id: intent,
            kind: LegOrderKind::Sell,
        },
        client_order_id: sell.intent.client_order_id.clone(),
        account_scope: late.account.to_owned(),
        ..unowned()
    };
    ensure!(
        fixture.ledger.record_order(&observed, &link, seen(1)).await?.changed,
        "native ledger must commit exact SELL arrival"
    );
    let now = check(&pool, &ready, late).await?;
    let durable: (Uuid, Uuid, String, String) = sqlx::query_as(
        "SELECT intent_id,position_id,side,owner FROM venue_orders
         WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)=($1,$2,$3,$4,$5)",
    )
    .bind(owner.tenant)
    .bind(owner.venue)
    .bind(owner.account)
    .bind(late.symbol)
    .bind(key)
    .fetch_one(&pool)
    .await?;
    ensure!(durable == (intent, late.position, "SELL".into(), "leg".into()));
    ensure!(
        now.unbound_sell == 0
            && now.facts.leg_sell == 1
            && unbound_sell_identities(&pool, owner).await?.is_empty(),
        "exact native arrival moves the same liability without duplication"
    );
    let mut c = pool.acquire().await?;
    ensure!(
        !repair(&mut c, late, intent).await?,
        "an already native-owned order must have no remaining active journal member to repair"
    );
    let stayed = check(&pool, &ready, late).await?;
    ensure!(
        stayed.unbound_sell == 0
            && stayed.facts.leg_sell == 1
            && stayed.facts.accounting_version == now.facts.accounting_version
            && ready.generation() == generation
    );
    eprintln!(
        "SPOT-LATE-MEMBER exact_native_arrival intent={intent} native={key} duplicate_members=0"
    );
    Ok(())
}
