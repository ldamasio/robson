//! Native PG16 projection witnesses on the existing first-cancel transaction.
use std::time::Duration;

use anyhow::{ensure, Result};
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_testkit::db_proxy::ProxyMode;
use sqlx::PgPool;

use super::{
    faults::{blackhole_commit, durable, Wire},
    positive::{migrate_fixture, PositiveLeg, SYMBOL},
    *,
};
use crate::spot_order_set_store::JournalGeneration;

fn request(id: Uuid, version: i64) -> TransitionWrite<'static> {
    TransitionWrite {
        symbol: SYMBOL,
        kind: TransitionKind::Replace,
        target_trigger: Decimal::new(95, 0),
        reason: "local-projected-first-cancel",
        expected_accounting_version: version,
        expected_transition: ExpectedTransition::New(id),
    }
}

async fn bind_generation(
    pool: &PgPool,
    tenant: Uuid,
    scope: &str,
    version: i64,
) -> Result<JournalGeneration> {
    let start = crate::process_epoch::start_epoch(pool, tenant, "first-cancel-projection").await?;
    let binding = JournalGeneration {
        tenant_id: tenant,
        account_scope: scope.to_owned(),
        process_epoch: start.epoch.epoch_id,
        generation_id: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO spot_order_set_generation (
             tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,
             accounting_version) VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)",
    )
    .bind(tenant)
    .bind(scope)
    .bind(binding.generation_id)
    .bind(binding.process_epoch)
    .bind(binding.source_audit_id)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(binding)
}

async fn assert_projected(
    pool: &PgPool,
    binding: &JournalGeneration,
    cancel: &IntentRow,
    transition: Uuid,
    expected_version: i64,
    terminal_mask: bool,
    pending: i64,
) -> Result<()> {
    let position = cancel.position_id.unwrap();
    let target = cancel.cancels_order_id.as_deref().unwrap();
    let facts: (i64, i64, i64, i64, bool, bool, i64, i64) = sqlx::query_as(
        "SELECT s.accounting_version,a.version,g.pending_count,g.conflict_count,g.terminal_mask,
                m.pending AND NOT m.conflict,coalesce(l.active_count,0),coalesce(l.conflict_count,0)
         FROM spot_order_set_generation s
         JOIN accounting_state a USING (tenant_id,venue_id)
         JOIN spot_order_set_cancel_groups g USING (tenant_id,venue_id,account_scope,generation_id)
         JOIN spot_order_set_cancel_members m USING (
             tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id)
         LEFT JOIN spot_order_set_cancel_legs l USING (
             tenant_id,venue_id,account_scope,generation_id,symbol,position_id)
         JOIN balance_transitions t ON t.first_cancel_intent_id=m.intent_id
              AND t.position_id=m.position_id AND t.original_target_order_id=m.target_id
         JOIN order_intents j ON j.intent_id=m.intent_id
              AND j.position_id=m.position_id AND j.cancels_order_id=m.target_id AND j.kind='cancel'
         WHERE s.tenant_id=$1 AND s.venue_id='binance:spot' AND s.account_scope=$2
           AND s.generation_id=$3 AND m.intent_id=$4 AND m.position_id=$5
           AND m.target_id=$6 AND t.transition_id=$7",
    )
    .bind(binding.tenant_id)
    .bind(&binding.account_scope)
    .bind(binding.generation_id)
    .bind(cancel.intent_id)
    .bind(position)
    .bind(target)
    .bind(transition)
    .fetch_one(pool)
    .await?;
    ensure!(
        facts
            == (
                expected_version,
                expected_version,
                pending,
                0,
                terminal_mask,
                true,
                if terminal_mask { 0 } else { pending },
                0
            )
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_lost_ack_is_one_atomic_observed_write(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    let mut wire = Wire::new(&pool, leg.tenant).await?;
    wire.store.order_set_projection = Some(binding.clone());
    let transition = Uuid::now_v7();
    wire.proxy
        .set_mode(ProxyMode::DropCommitAckOf("INSERT INTO balance_transitions"));
    let result = wire.store.commit_first_cancel(&cancel, request(transition, version)).await;
    ensure!(matches!(result, Err(CommitFailure::Uncertain(_))));
    ensure!(wire.proxy.stats().commit_acks_dropped == 1);
    let trace = wire.proxy.query_trace();
    ensure!(trace.len() == 8 && trace[6].contains("spot_order_set_cancel_members"));
    wire.retires(&pool).await?;
    leg.assert_bare_cancel(&pool, &cancel).await?;
    assert_projected(&pool, &binding, &cancel, transition, version + 1, false, 1).await?;
    eprintln!(
        "Projected first cancel: 8 physical commands, native COMMIT landed without acknowledgement"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_stale_binding_rolls_back_under_native_accounting_lock(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let mut binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    binding.generation_id = Uuid::now_v7();
    let before = durable(&pool, leg.tenant).await?;
    let store = PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding);
    ensure!(matches!(
        store.commit_first_cancel(&cancel, request(Uuid::now_v7(), version)).await,
        Err(CommitFailure::Known(_))
    ));
    ensure!(durable(&pool, leg.tenant).await? == before);
    let members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM spot_order_set_cancel_members WHERE intent_id=$1")
            .bind(cancel.intent_id)
            .fetch_one(&pool)
            .await?;
    ensure!(members == 0);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_preserves_qualified_terminal_mask(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let _alias = leg.alias_on_another_symbol(&pool, "ETHUSDT").await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    let order = cancel.cancels_order_id.as_deref().unwrap();
    sqlx::query(
        "WITH target AS (
            UPDATE venue_orders SET status='CANCELED',phase='terminal',terminal_seen_at=now()
            WHERE tenant_id=$1 AND venue_id='binance:spot' AND account_scope=$2
              AND exchange_order_id=$4
            RETURNING intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,symbol,exchange_order_id
        ), journal AS (
            INSERT INTO order_intents
                (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
            SELECT intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,
                   'protection','{}',now() FROM target RETURNING intent_id
        )
        INSERT INTO spot_order_set_cancel_groups
            (tenant_id,venue_id,account_scope,generation_id,symbol,position_id,target_id,
             pending_count,conflict_count,terminal_mask)
        SELECT t.tenant_id,t.venue_id,t.account_scope,$5,t.symbol,t.position_id,t.exchange_order_id,0,0,true
        FROM target t JOIN journal j USING (intent_id) WHERE t.symbol=$3",
    )
    .bind(leg.tenant)
    .bind(&cancel.account_scope)
    .bind(SYMBOL)
    .bind(order)
    .bind(binding.generation_id)
    .execute(&pool)
    .await?;
    let store = PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding.clone());
    let before = durable(&pool, leg.tenant).await?;
    sqlx::query(
        "UPDATE spot_order_set_cancel_groups SET terminal_mask=false
         WHERE tenant_id=$1 AND generation_id=$2",
    )
    .bind(leg.tenant)
    .bind(binding.generation_id)
    .execute(&pool)
    .await?;
    ensure!(matches!(
        store.commit_first_cancel(&cancel, request(Uuid::now_v7(), version)).await,
        Err(CommitFailure::Known(_))
    ));
    ensure!(durable(&pool, leg.tenant).await? == before);
    sqlx::query(
        "UPDATE spot_order_set_cancel_groups SET terminal_mask=true
         WHERE tenant_id=$1 AND generation_id=$2",
    )
    .bind(leg.tenant)
    .bind(binding.generation_id)
    .execute(&pool)
    .await?;
    let transition = Uuid::now_v7();
    store
        .commit_first_cancel(&cancel, request(transition, version))
        .await
        .map_err(|err| anyhow::anyhow!("projected first cancel: {err:?}"))?;
    assert_projected(&pool, &binding, &cancel, transition, version + 1, true, 1).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_generated_same_version_competitors_publish_once(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x75b3e912),
        failure_persistence: None,
        ..Config::default()
    });
    let delays = (0_u64..=1_800, 0_u64..=1_800);
    for case in 0..24 {
        let (a_delay, b_delay) = delays.new_tree(&mut runner).expect("bounded schedule").current();
        let leg = PositiveLeg::admit(&pool).await?;
        let a = leg.cancel_intent();
        let b = leg.cancel_intent();
        let version = leg.accounting_version(&pool).await?;
        let binding = bind_generation(&pool, leg.tenant, &a.account_scope, version).await?;
        let left =
            PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding.clone());
        let right =
            PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding.clone());
        let t1 = Uuid::now_v7();
        let t2 = Uuid::now_v7();
        let submit = |store: PgJournalBackend, intent: IntentRow, transition, delay| async move {
            if delay != 0 {
                tokio::time::sleep(Duration::from_micros(delay)).await;
            }
            store.commit_first_cancel(&intent, request(transition, version)).await
        };
        let (one, two) = tokio::join!(
            submit(left, a.clone(), t1, a_delay),
            submit(right, b.clone(), t2, b_delay),
        );
        let (winner, transition, loser) = match (one, two) {
            (Ok(_), Err(CommitFailure::Known(_))) => (&a, t1, &b),
            (Err(CommitFailure::Known(_)), Ok(_)) => (&b, t2, &a),
            result => anyhow::bail!(
                "case {case} ({a_delay},{b_delay}): expected precisely one acknowledgement, observed {result:?}"
            ),
        };
        assert_projected(&pool, &binding, winner, transition, version + 1, false, 1).await?;
        let loser_writes: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM order_intents WHERE intent_id=$1)
                    +(SELECT count(*) FROM spot_order_set_cancel_members WHERE intent_id=$1)",
        )
        .bind(loser.intent_id)
        .fetch_one(&pool)
        .await?;
        ensure!(loser_writes == 0);
    }
    eprintln!(
        "Projected first cancel: 24 proptest-generated same-version native schedules, one winner each"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_rejects_wrong_scope_and_obsolete_epochs(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    let transition = Uuid::now_v7();
    let mut wire = Wire::new(&pool, leg.tenant).await?;
    let before = durable(&pool, leg.tenant).await?;
    for invalid in [
        JournalGeneration {
            tenant_id: Uuid::now_v7(),
            ..binding.clone()
        },
        JournalGeneration {
            account_scope: "foreign-scope".to_owned(),
            ..binding.clone()
        },
    ] {
        wire.store.order_set_projection = Some(invalid);
        ensure!(matches!(
            wire.store.commit_first_cancel(&cancel, request(transition, version)).await,
            Err(CommitFailure::Known(reason)) if reason.contains("invalid binding")
        ));
        ensure!(wire.proxy.query_trace().is_empty());
    }
    ensure!(durable(&pool, leg.tenant).await? == before);
    let mismatched_epoch =
        PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(JournalGeneration {
            process_epoch: Uuid::now_v7(),
            ..binding.clone()
        });
    ensure!(matches!(
        mismatched_epoch.commit_first_cancel(&cancel, request(transition, version)).await,
        Err(CommitFailure::Known(reason)) if reason.contains("stale generation")
    ));
    ensure!(durable(&pool, leg.tenant).await? == before);

    sqlx::query(
        "UPDATE process_epochs SET clean_stop_at=now(),marker_version=$2 WHERE epoch_id=$1",
    )
    .bind(binding.process_epoch)
    .bind(crate::process_epoch::MARKER_VERSION)
    .execute(&pool)
    .await?;
    let ended = PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding);
    ensure!(matches!(
        ended.commit_first_cancel(&cancel, request(transition, version)).await,
        Err(CommitFailure::Known(reason)) if reason.contains("stale generation")
    ));
    ensure!(durable(&pool, leg.tenant).await? == before);
    assert_no_projection(&pool, leg.tenant, cancel.intent_id).await?;

    // Use a separate tenant so the original writer in this case is still
    // open. An implementation checking only clean_stop_at must fail here.
    let old_leg = PositiveLeg::admit(&pool).await?;
    let old_cancel = old_leg.cancel_intent();
    let old_version = old_leg.accounting_version(&pool).await?;
    let old =
        bind_generation(&pool, old_leg.tenant, &old_cancel.account_scope, old_version).await?;
    let next =
        crate::process_epoch::start_epoch(&pool, old_leg.tenant, "first-cancel-successor").await?;
    ensure!(next.predecessor_epoch_id == Some(old.process_epoch));
    let open: bool =
        sqlx::query_scalar("SELECT clean_stop_at IS NULL FROM process_epochs WHERE epoch_id=$1")
            .bind(old.process_epoch)
            .fetch_one(&pool)
            .await?;
    ensure!(open && old_leg.accounting_version(&pool).await? == old_version);
    let stable = durable(&pool, old_leg.tenant).await?;
    let superseded = PgJournalBackend::new(&pool, old_leg.tenant).with_order_set_projection(old);
    ensure!(matches!(
        superseded.commit_first_cancel(&old_cancel, request(Uuid::now_v7(), old_version)).await,
        Err(CommitFailure::Known(reason)) if reason.contains("stale generation")
    ));
    ensure!(durable(&pool, old_leg.tenant).await? == stable);
    assert_no_projection(&pool, old_leg.tenant, old_cancel.intent_id).await
}

async fn assert_no_projection(pool: &PgPool, tenant: Uuid, cancel: Uuid) -> Result<()> {
    let actual: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM spot_order_set_cancel_members WHERE intent_id=$2),
                (SELECT count(*) FROM spot_order_set_cancel_groups WHERE tenant_id=$1),
                (SELECT count(*) FROM spot_order_set_cancel_legs WHERE tenant_id=$1)",
    )
    .bind(tenant)
    .bind(cancel)
    .fetch_one(pool)
    .await?;
    ensure!(actual == (0, 0, 0));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_updates_populated_group_on_existing_open_transition(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let first = leg.cancel_intent();
    let second = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &first.account_scope, version).await?;
    let store = PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding.clone());
    let initial = Uuid::now_v7();
    store
        .commit_first_cancel(&first, request(initial, version))
        .await
        .map_err(|error| anyhow::anyhow!("original projected first cancel: {error:?}"))?;
    assert_projected(&pool, &binding, &first, initial, version + 1, false, 1).await?;
    sqlx::query("UPDATE balance_transitions SET completed_at=now(),outcome='fixture-closed' WHERE transition_id=$1")
        .bind(initial).execute(&pool).await?;
    let transition = Uuid::now_v7();
    let mut command = request(transition, version + 1);
    sqlx::query(
        "INSERT INTO balance_transitions (
             transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,
             target_trigger,reason,started_at)
         VALUES ($1,$2,$3,'binance:spot',$4,$5,'replace',$6,$7,now())",
    )
    .bind(transition)
    .bind(second.position_id)
    .bind(leg.tenant)
    .bind(&second.account_scope)
    .bind(SYMBOL)
    .bind(command.target_trigger)
    .bind(command.reason)
    .execute(&pool)
    .await?;
    command.expected_transition = ExpectedTransition::ExistingOpen(transition);
    store
        .commit_first_cancel(&second, command)
        .await
        .map_err(|error| anyhow::anyhow!("projected attach to existing transition: {error:?}"))?;
    assert_projected(&pool, &binding, &first, initial, version + 2, false, 2).await?;
    assert_projected(&pool, &binding, &second, transition, version + 2, false, 2).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_duplicate_member_refuses_without_new_source_or_counters(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    sqlx::query(
        "INSERT INTO spot_order_set_cancel_members (
             tenant_id,venue_id,account_scope,generation_id,intent_id,symbol,position_id,
             target_id,pending,conflict)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6,$7,true,false)",
    )
    .bind(leg.tenant)
    .bind(&binding.account_scope)
    .bind(binding.generation_id)
    .bind(cancel.intent_id)
    .bind(SYMBOL)
    .bind(cancel.position_id)
    .bind(&cancel.cancels_order_id)
    .execute(&pool)
    .await?;
    let before = durable(&pool, leg.tenant).await?;
    let store = PgJournalBackend::new(&pool, leg.tenant).with_order_set_projection(binding.clone());
    ensure!(matches!(
        store.commit_first_cancel(&cancel, request(Uuid::now_v7(), version)).await,
        Err(CommitFailure::Known(reason)) if reason.contains("stale generation")
    ));
    ensure!(durable(&pool, leg.tenant).await? == before);
    let actual: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT accounting_version,
                (SELECT count(*) FROM spot_order_set_cancel_members WHERE intent_id=$2 AND pending AND NOT conflict),
                (SELECT count(*) FROM spot_order_set_cancel_groups WHERE generation_id=$1),
                (SELECT count(*) FROM spot_order_set_cancel_legs WHERE generation_id=$1)
         FROM spot_order_set_generation WHERE generation_id=$1",
    )
    .bind(binding.generation_id).bind(cancel.intent_id).fetch_one(&pool).await?;
    ensure!(actual == (version, 1, 0, 0));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_first_cancel_retires_a_blocked_commit_with_all_writes_absent(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let cancel = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let binding = bind_generation(&pool, leg.tenant, &cancel.account_scope, version).await?;
    let before = durable(&pool, leg.tenant).await?;
    let mut wire = Wire::new(&pool, leg.tenant).await?;
    wire.store.order_set_projection = Some(binding);
    let result = blackhole_commit(
        wire.store.commit_first_cancel(&cancel, request(Uuid::now_v7(), version)),
        &wire.proxy,
    )
    .await?;
    ensure!(matches!(result, Err(CommitFailure::Uncertain(_))));
    let trace = wire.proxy.query_trace();
    ensure!(
        trace.len() == 7 && trace[6].contains("spot_order_set_cancel_members"),
        "{trace:?}"
    );
    wire.retires(&pool).await?;
    ensure!(durable(&pool, leg.tenant).await? == before);
    assert_no_projection(&pool, leg.tenant, cancel.intent_id).await
}
