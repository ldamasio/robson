//! Grouped outcome updates use the real first-cancel and native journal paths.
use anyhow::{ensure, Result};
use robson_exec::order_journal::{
    BalanceBatch, JournalBackend, JournalBatch, OutcomeRow, VenueOutcome,
};
use robson_testkit::db_proxy::ProxyMode;
use sqlx::PgPool;

use super::{
    faults::{durable, Wire},
    positive::{migrate_fixture, PositiveLeg, SYMBOL},
    *,
};
use crate::{order_journal_store::clocked, process_epoch, spot_order_set_store::JournalGeneration};

async fn prepared(
    pool: &PgPool,
    masked: bool,
) -> Result<(Uuid, JournalGeneration, JournalBatch, i64)> {
    let leg = PositiveLeg::admit(pool).await?;
    let intent = leg.cancel_intent();
    let before = leg.accounting_version(pool).await?;
    let start = process_epoch::start_epoch(pool, leg.tenant, "grouped-cancel-outcome").await?;
    let b = JournalGeneration {
        tenant_id: leg.tenant,
        account_scope: intent.account_scope.clone(),
        generation_id: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
        process_epoch: start.epoch.epoch_id,
    };
    sqlx::query(
        "INSERT INTO spot_order_set_generation(
             tenant_id,venue_id,account_scope,generation_id,source_audit_id,process_epoch,accounting_version)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)",
    )
    .bind(b.tenant_id).bind(&b.account_scope).bind(b.generation_id).bind(b.source_audit_id)
    .bind(b.process_epoch).bind(before).execute(pool).await?;
    if masked {
        sqlx::query(
            "WITH target AS (
                 UPDATE venue_orders SET status='CANCELED',phase='terminal',terminal_seen_at=now()
                 WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)
                       =($1,'binance:spot',$2,$3,$4)
                 RETURNING intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,symbol,exchange_order_id
             ), inserted AS (
                 INSERT INTO order_intents(
                     intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
                 SELECT intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,
                        'protection','{}',now() FROM target RETURNING intent_id
             )
             INSERT INTO spot_order_set_cancel_groups(
                 tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,
                 pending_count,conflict_count,terminal_mask)
             SELECT t.tenant_id,t.venue_id,t.account_scope,$5,t.symbol,t.position_id,t.exchange_order_id,0,0,true
             FROM target t JOIN inserted i USING(intent_id)",
        )
        .bind(leg.tenant).bind(&b.account_scope).bind(SYMBOL).bind(&intent.cancels_order_id)
        .bind(b.generation_id).execute(pool).await?;
    }
    let journal = PgJournalBackend::new(pool, leg.tenant).with_order_set_projection(b.clone());
    let transition = Uuid::now_v7();
    journal
        .commit_first_cancel(&intent, TransitionWrite {
            symbol: SYMBOL,
            kind: TransitionKind::Replace,
            target_trigger: Decimal::new(95, 0),
            reason: "grouped-cancel-outcome",
            expected_accounting_version: before,
            expected_transition: ExpectedTransition::New(transition),
        })
        .await
        .map_err(|error| anyhow::anyhow!("first cancel: {error:?}"))?;
    let batch = JournalBatch {
        intent,
        attempts: Vec::new(),
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: std::time::Duration::from_millis(500),
        }),
    };
    Ok((transition, b, batch, before + 1))
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
struct ResultFacts {
    accounting: i64,
    projected: i64,
    native_not_sent: bool,
    native_outcome: Option<String>,
    pending: bool,
    conflict: bool,
    pending_count: i64,
    group_conflict: i64,
    terminal_mask: bool,
    active_count: i64,
    leg_conflict: i64,
}

async fn read_facts(pool: &PgPool, b: &JournalGeneration, id: Uuid) -> Result<ResultFacts> {
    Ok(sqlx::query_as(
        "SELECT a.version accounting,g.accounting_version projected,j.not_sent native_not_sent,
                j.venue_outcome native_outcome,m.pending,m.conflict,grp.pending_count,
                grp.conflict_count group_conflict,grp.terminal_mask,coalesce(l.active_count,0) active_count,
                coalesce(l.conflict_count,0) leg_conflict
           FROM spot_order_set_generation g JOIN accounting_state a USING(tenant_id,venue_id)
           JOIN spot_order_set_cancel_members m USING(tenant_id,venue_id,account_scope,generation_id)
           JOIN spot_order_set_cancel_groups grp USING(tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id)
           LEFT JOIN spot_order_set_cancel_legs l USING(tenant_id,venue_id,account_scope,generation_id,symbol,position_id)
           JOIN order_intents j ON j.intent_id=m.intent_id
           WHERE g.generation_id=$1 AND m.intent_id=$2",
    ).bind(b.generation_id).bind(id).fetch_one(pool).await?)
}

fn outcome(not_sent: bool, outcome: Option<VenueOutcome>) -> Option<OutcomeRow> {
    Some(OutcomeRow {
        not_sent,
        venue_outcome: outcome,
        ..OutcomeRow::default()
    })
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_ordinary_and_clocked_outcomes_keep_exact_deltas(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, v) = prepared(&pool, false).await?;
    let store = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    let id = batch.intent.intent_id;
    batch.outcome = outcome(true, None);
    store
        .commit(&batch)
        .await
        .map_err(|error| anyhow::anyhow!("ordinary cancel result: {error:?}"))?;
    let unsent = read_facts(&pool, &b, id).await?;
    ensure!(
        unsent
            == ResultFacts {
                accounting: v + 1,
                projected: v + 1,
                native_not_sent: true,
                native_outcome: None,
                pending: false,
                conflict: false,
                pending_count: 0,
                group_conflict: 0,
                terminal_mask: false,
                active_count: 0,
                leg_conflict: 0
            }
    );
    batch.outcome = outcome(false, Some(VenueOutcome::Success));
    let input = clocked::ClockedInput {
        process_epoch: b.process_epoch,
        attempts: Vec::new(),
    };
    let receipts = store
        .commit_clocked(&batch, &input)
        .await
        .map_err(|error| anyhow::anyhow!("clocked result: {error:?}"))?;
    ensure!(receipts.is_empty());
    let actual = read_facts(&pool, &b, id).await?;
    ensure!(
        actual
            == ResultFacts {
                accounting: v + 2,
                projected: v + 2,
                native_not_sent: true,
                native_outcome: Some("success".into()),
                pending: true,
                conflict: true,
                pending_count: 1,
                group_conflict: 1,
                terminal_mask: false,
                active_count: 1,
                leg_conflict: 1
            }
    );
    let before = durable(&pool, b.tenant_id).await?;
    let wrong = clocked::ClockedInput {
        process_epoch: Uuid::now_v7(),
        attempts: Vec::new(),
    };
    ensure!(matches!(
        store.commit_clocked(&batch, &wrong).await,
        Err(CommitFailure::Known(_))
    ));
    ensure!(durable(&pool, b.tenant_id).await? == before);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_lost_ack_retires_then_observes_both_versions(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, v) = prepared(&pool, false).await?;
    let mut wire = Wire::new(&pool, b.tenant_id).await?;
    wire.store.order_set_projection = Some(b.clone());
    batch.outcome = outcome(true, None);
    wire.proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE order_intents SET"));
    ensure!(matches!(wire.store.commit(&batch).await, Err(CommitFailure::Uncertain(_))));
    let queries = wire.proxy.query_trace();
    ensure!(wire.proxy.stats().commit_acks_dropped == 1);
    ensure!(queries.last().is_some_and(|sql| sql == "COMMIT"));
    ensure!(
        queries
            .iter()
            .filter(|sql| sql.contains("UPDATE public.spot_order_set_generation"))
            .count()
            == 1
    );
    ensure!(!queries.iter().any(|sql| sql.contains("budget_insert_clocks")));
    wire.retires(&pool).await?;
    let actual = read_facts(&pool, &b, batch.intent.intent_id).await?;
    ensure!(
        actual.native_not_sent
            && !actual.pending
            && !actual.conflict
            && actual.accounting == v + 1
            && actual.projected == v + 1
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_requires_exact_prior_group_and_preserves_native_terminal_mask(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, v) = prepared(&pool, true).await?;
    let id = batch.intent.intent_id;
    let initial = read_facts(&pool, &b, id).await?;
    ensure!(
        initial.pending
            && initial.terminal_mask
            && initial.pending_count == 1
            && initial.active_count == 0
    );
    let plain = durable(&pool, b.tenant_id).await?;
    batch.outcome = outcome(true, Some(VenueOutcome::Success));
    let store = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    for (change, restore) in [
        (
            "UPDATE spot_order_set_cancel_members SET pending=false WHERE intent_id=$1",
            "UPDATE spot_order_set_cancel_members SET pending=true WHERE intent_id=$1",
        ),
        (
            "UPDATE spot_order_set_cancel_groups SET terminal_mask=false WHERE generation_id=$1",
            "UPDATE spot_order_set_cancel_groups SET terminal_mask=true WHERE generation_id=$1",
        ),
        (
            "UPDATE spot_order_set_cancel_groups SET pending_count=0 WHERE generation_id=$1",
            "UPDATE spot_order_set_cancel_groups SET pending_count=1 WHERE generation_id=$1",
        ),
    ] {
        let target = if change.contains("intent_id") {
            id
        } else {
            b.generation_id
        };
        sqlx::query(change).bind(target).execute(&pool).await?;
        let rejected = read_facts(&pool, &b, id).await?;
        ensure!(matches!(store.commit(&batch).await, Err(CommitFailure::Known(_))));
        ensure!(durable(&pool, b.tenant_id).await? == plain);
        ensure!(read_facts(&pool, &b, id).await? == rejected);
        sqlx::query(restore).bind(target).execute(&pool).await?;
        ensure!(read_facts(&pool, &b, id).await? == initial);
    }
    store
        .commit(&batch)
        .await
        .map_err(|error| anyhow::anyhow!("masked outcome: {error:?}"))?;
    let committed = read_facts(&pool, &b, id).await?;
    ensure!(
        committed
            == ResultFacts {
                accounting: v + 1,
                projected: v + 1,
                native_not_sent: true,
                native_outcome: Some("success".into()),
                pending: true,
                conflict: true,
                pending_count: 1,
                group_conflict: 1,
                terminal_mask: true,
                active_count: 0,
                leg_conflict: 1
            }
    );
    store
        .commit(&batch)
        .await
        .map_err(|error| anyhow::anyhow!("exact outcome replay: {error:?}"))?;
    let replay = read_facts(&pool, &b, id).await?;
    ensure!(
        replay
            == ResultFacts {
                accounting: v + 2,
                projected: v + 2,
                ..committed
            }
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_seeded_interleavings_converge_one_exact_member(
    pool: PgPool,
) -> Result<()> {
    use std::time::Duration;

    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::{Config, RngSeed, TestRunner},
    };
    migrate_fixture(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0xac825a23),
        failure_persistence: None,
        ..Config::default()
    });
    let strategy = (0_u64..=1700, 0_u64..=1700, proptest::bool::ANY);
    for _ in 0..16 {
        let (x, y, mask) = strategy.new_tree(&mut runner).expect("bounded schedule").current();
        let (_, b, mut first, v) = prepared(&pool, mask).await?;
        let id = first.intent.intent_id;
        let mut second = first.clone();
        first.outcome = outcome(true, None);
        second.outcome = outcome(false, Some(VenueOutcome::Success));
        let left = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
        let right = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
        let read = async {
            for _ in 0..3 {
                let f = read_facts(&pool, &b, id).await?;
                ensure!(
                    f.accounting == f.projected
                        && f.pending_count == i64::from(f.pending)
                        && f.group_conflict == i64::from(f.conflict)
                        && f.terminal_mask == mask
                        && f.active_count == i64::from(f.pending && !mask)
                        && f.leg_conflict == i64::from(f.conflict)
                );
                tokio::task::yield_now().await;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (a, c, seen) = tokio::join!(
            async {
                tokio::time::sleep(Duration::from_micros(x)).await;
                left.commit(&first).await
            },
            async {
                tokio::time::sleep(Duration::from_micros(y)).await;
                right.commit(&second).await
            },
            read,
        );
        seen?;
        ensure!(a.is_ok() && c.is_ok(), "grouped outcomes {a:?} {c:?}");
        let f = read_facts(&pool, &b, id).await?;
        ensure!(
            f == ResultFacts {
                accounting: v + 2,
                projected: v + 2,
                native_not_sent: true,
                native_outcome: Some("success".into()),
                pending: true,
                conflict: true,
                pending_count: 1,
                group_conflict: 1,
                terminal_mask: mask,
                active_count: i64::from(!mask),
                leg_conflict: 1
            }
        );
    }
    Ok(())
}

async fn named_order(pool: &PgPool, tenant: Uuid, cancel_id: Uuid) -> Result<()> {
    // A separate native (leg-owned) observation naming this J row. The test
    // initializes the same conservative prior projection before the write.
    sqlx::query(
        "INSERT INTO venue_orders (
             venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,
             last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,
             status,phase,last_checked_at,live_seen_at)
         SELECT venue_id,account_scope,symbol,'4100009',tenant_id,'named-cancel','named-cancel',
                'leg',$2,position_id,'sell','SELL','MARKET',1,'NEW','live',now(),now()
           FROM venue_orders WHERE tenant_id=$1 AND exchange_order_id='4100001'",
    )
    .bind(tenant)
    .bind(cancel_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn expected_named_projection(pool: &PgPool, b: &JournalGeneration, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE spot_order_set_cancel_members SET conflict=true WHERE intent_id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE spot_order_set_cancel_groups SET conflict_count=1 WHERE generation_id=$1")
        .bind(b.generation_id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE spot_order_set_cancel_legs SET conflict_count=1 WHERE generation_id=$1")
        .bind(b.generation_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_reads_named_conflict_only_from_its_native_scope(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    for is_foreign in [false, true] {
        let (_, b, mut batch, v) = prepared(&pool, false).await?;
        let id = batch.intent.intent_id;
        let name_owner = if is_foreign {
            PositiveLeg::admit(&pool).await?.tenant
        } else {
            b.tenant_id
        };
        named_order(&pool, name_owner, id).await?;
        expected_named_projection(&pool, &b, id).await?;
        let durable_before = durable(&pool, b.tenant_id).await?;
        let previous = read_facts(&pool, &b, id).await?;
        ensure!(previous.pending && previous.conflict && previous.leg_conflict == 1);
        let store = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
        batch.outcome = outcome(true, None);
        let result = store.commit(&batch).await;
        if is_foreign {
            ensure!(matches!(result, Err(CommitFailure::Known(_))));
            ensure!(durable(&pool, b.tenant_id).await? == durable_before);
            ensure!(read_facts(&pool, &b, id).await? == previous);
        } else {
            result.map_err(|error| anyhow::anyhow!("same-scope named conflict: {error:?}"))?;
            let actual = read_facts(&pool, &b, id).await?;
            ensure!(
                actual
                    == ResultFacts {
                        native_not_sent: true,
                        accounting: v + 1,
                        projected: v + 1,
                        ..previous
                    }
            );
        }
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_grouped_cancel_refuses_missing_native_write_and_obsolete_epoch(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let (_, b, mut batch, _) = prepared(&pool, false).await?;
    let id = batch.intent.intent_id;
    let previous = read_facts(&pool, &b, id).await?;
    let store = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    let stable = durable(&pool, b.tenant_id).await?;
    sqlx::raw_sql(
        "CREATE FUNCTION refuse_outcome_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RETURN NULL; END $$;
         CREATE TRIGGER refuse_outcome_fixture BEFORE UPDATE OF not_sent ON order_intents
         FOR EACH ROW EXECUTE FUNCTION refuse_outcome_fixture()",
    )
    .execute(&pool)
    .await?;
    batch.outcome = outcome(true, None);
    ensure!(matches!(store.commit(&batch).await,
        Err(CommitFailure::Known(reason)) if reason.contains("did not affect one row")));
    ensure!(
        durable(&pool, b.tenant_id).await? == stable
            && read_facts(&pool, &b, id).await? == previous
    );
    sqlx::query("DROP TRIGGER refuse_outcome_fixture ON order_intents")
        .execute(&pool)
        .await?;

    let newer = process_epoch::start_epoch(&pool, b.tenant_id, "grouped-outcome-successor").await?;
    ensure!(newer.predecessor_epoch_id == Some(b.process_epoch));
    let open: bool =
        sqlx::query_scalar("SELECT clean_stop_at IS NULL FROM process_epochs WHERE epoch_id=$1")
            .bind(b.process_epoch)
            .fetch_one(&pool)
            .await?;
    ensure!(open);
    let after_epoch = durable(&pool, b.tenant_id).await?;
    ensure!(matches!(store.commit(&batch).await, Err(CommitFailure::Known(_))));
    ensure!(
        durable(&pool, b.tenant_id).await? == after_epoch
            && read_facts(&pool, &b, id).await? == previous
    );
    Ok(())
}
