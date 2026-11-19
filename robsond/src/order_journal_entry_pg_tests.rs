//! Provisional native admission tests. The CANCEL hint schema is created only
//! in isolated PG16; these tests never establish reconstruction provenance.
use std::sync::Arc;

use robson_exec::order_journal::AdmissionRow;
use rust_decimal::Decimal;

use super::*;

const SYMBOL: &str = "BTCUSDT";

async fn arm(pool: &PgPool, g: &JournalGeneration) -> Result<JournalBatch> {
    let id = Uuid::now_v7();
    let position = Uuid::now_v7();
    let mut batch = sell();
    batch.intent.kind = IntentKind::Entry;
    batch.intent.intent_id = id;
    batch.intent.client_order_id = format!("entry-{}", id.simple());
    batch.intent.position_id = Some(position);
    let pre = crate::venue_ledger_store::PgVenueLedger::new(pool, g.tenant_id);
    pre.insert_route(position, &VenueId::new(VenueId::BINANCE_SPOT), &g.account_scope, Utc::now())
        .await?;
    batch.balance.as_mut().unwrap().admission = Some(Arc::new(AdmissionRow {
        position_id: position,
        symbol: SYMBOL.to_owned(),
        decided_month: chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        taker_bps: Decimal::new(10, 0),
        gap_bps: Decimal::new(10, 0),
        notional_margin_bps: Decimal::new(2, 0),
        min_notional: Decimal::new(10, 0),
        lot_step: Decimal::new(1, 3),
        fee_asset_policy: "base".into(),
        envelope_source: "isolated-entry-terms".into(),
        envelope_as_of: Utc::now(),
        envelope_hash: "fixture".into(),
        planned_qty: Decimal::ONE,
        requested_qty: Decimal::ONE,
        reference_price: Decimal::new(100, 0),
        remainder_reserve: Decimal::new(1, 1),
        admission_charge: Decimal::new(10, 0),
        initial_trigger: Decimal::new(90, 0),
    }));
    Ok(batch)
}

async fn hint_schema(pool: &PgPool) -> Result<()> {
    sqlx::raw_sql(include_str!("../tests/spot_entry_hint_fixture.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn assert_current(
    pool: &PgPool,
    b: &JournalGeneration,
    j: &JournalBatch,
    version: i64,
) -> Result<()> {
    let input = j.balance.as_ref().unwrap().admission.as_ref().unwrap();
    let actual: (i64, i64, Uuid, Uuid) = sqlx::query_as(
        "SELECT a.version,g.accounting_version,l.admission_intent_id,c.entry_intent_id
         FROM accounting_state a JOIN spot_order_set_generation g USING(tenant_id,venue_id)
         JOIN balance_legs l USING(tenant_id,venue_id,account_scope)
         JOIN balance_leg_admissions c ON c.position_id=l.position_id AND c.superseded_at IS NULL
              AND (c.tenant_id,c.venue_id,c.account_scope,c.symbol,c.decided_month)
                   =(l.tenant_id,l.venue_id,l.account_scope,l.symbol,l.decided_month)
         JOIN order_intents j ON j.intent_id=c.entry_intent_id AND j.kind='entry'
              AND (j.tenant_id,j.venue_id,j.account_scope,j.position_id)
                   =(l.tenant_id,l.venue_id,l.account_scope,l.position_id)
         WHERE g.generation_id=$1 AND l.position_id=$2 AND l.symbol=$3",
    )
    .bind(b.generation_id)
    .bind(input.position_id)
    .bind(&input.symbol)
    .fetch_one(pool)
    .await?;
    ensure!(actual == (version, version, j.intent.intent_id, j.intent.intent_id));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_real_birth_native_clock_and_exact_replay(pool: PgPool) -> Result<()> {
    hint_schema(&pool).await?;
    let clock = native_clock(&pool).await?;
    let b = seed_generation(&pool).await?;
    let start = fields(&pool, &b).await?.0;
    let mut j = arm(&pool, &b).await?;
    let stored = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    let input = clocked_attempt(&clock, &b, &mut j)?;
    let sent = stored
        .commit_clocked(&j, &input)
        .await
        .map_err(|failure| anyhow::anyhow!("fresh native Entry: {failure:?}"))?;
    ensure!(sent.len() == 1);
    assert_current(&pool, &b, &j, start + 1).await?;
    let replay = stored
        .commit_clocked(&j, &input)
        .await
        .map_err(|failure| anyhow::anyhow!("exact native Entry replay: {failure:?}"))?;
    ensure!(replay.len() == 1);
    assert_current(&pool, &b, &j, start + 2).await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_refuses_an_inactive_original_cancel_hint(pool: PgPool) -> Result<()> {
    hint_schema(&pool).await?;
    let b = seed_generation(&pool).await?;
    let j = arm(&pool, &b).await?;
    let position = j.intent.position_id.unwrap();
    let earlier = Uuid::now_v7();
    sqlx::query(
        "WITH original AS (
             INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,
                 kind,cancels_order_id,params,created_at,not_sent)
             VALUES($1,$2,'binance:spot',$3,$1::uuid::text,$4,'cancel','4100001','{}',now(),true)
             RETURNING intent_id,tenant_id,venue_id,account_scope,position_id,cancels_order_id
         )
         INSERT INTO spot_order_set_cancel_members
            (tenant_id,venue_id,account_scope,generation_id,intent_id,target_id,pending,conflict,source_position)
         SELECT tenant_id,venue_id,account_scope,$5,intent_id,cancels_order_id,false,false,position_id
         FROM original",
    ).bind(earlier).bind(b.tenant_id).bind(&b.account_scope).bind(position).bind(b.generation_id)
    .execute(&pool).await?;
    let before = fields(&pool, &b).await?;
    let stored = PgJournalBackend::new(&pool, b.tenant_id).with_order_set_projection(b.clone());
    ensure!(matches!(stored.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(!j_present(&pool, j.intent.intent_id).await? && fields(&pool, &b).await? == before);
    let has_leg: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM balance_legs WHERE position_id=$1)")
            .bind(position)
            .fetch_one(&pool)
            .await?;
    ensure!(!has_leg);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_original_ack_loss_and_linked_replay_preserve_native_identity(
    pool: PgPool,
) -> Result<()> {
    hint_schema(&pool).await?;
    let g = seed_generation(&pool).await?;
    let j = arm(&pool, &g).await?;
    let position = j.intent.position_id.unwrap();
    let before = fields(&pool, &g).await?.0;
    let (store, proxy, pid, birth) = proxied_journal(&pool, &g).await?;
    proxy.set_mode(ProxyMode::DropCommitAckOf("SELECT balance_leg_admit"));
    ensure!(matches!(store.commit(&j).await, Err(CommitFailure::Uncertain(_))));
    ensure!(proxy.stats().commit_acks_dropped == 1);
    native_retired(&pool, pid, &birth).await?;
    assert_current(&pool, &g, &j, before + 1).await?;

    // A same-version exact native reverse edge independently invalidates the
    // numeric-only replay specialization. No generation mismatch is seeded.
    sqlx::query(
        "INSERT INTO venue_orders(venue_id,account_scope,symbol,exchange_order_id,tenant_id,
            client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,
            orig_qty,status,phase,last_checked_at,live_seen_at)
         VALUES('binance:spot',$1,$2,'4100013',$3,$4,$4,'leg',$5,$6,'entry','BUY','MARKET',
                1,'NEW','live',now(),now())",
    )
    .bind(&g.account_scope)
    .bind(SYMBOL)
    .bind(g.tenant_id)
    .bind(&j.intent.client_order_id)
    .bind(j.intent.intent_id)
    .bind(position)
    .execute(&pool)
    .await?;
    let ordinary = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    ensure!(matches!(ordinary.commit(&j).await, Err(CommitFailure::Known(_))));
    assert_current(&pool, &g, &j, before + 1).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_retains_active_unbound_sell_until_exact_repair(
    pool: PgPool,
) -> Result<()> {
    hint_schema(&pool).await?;
    let g = seed_generation(&pool).await?;
    let j = arm(&pool, &g).await?;
    let writer = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let mut unresolved = sell();
    unresolved.intent.position_id = j.intent.position_id;
    insist(writer.commit(&unresolved).await)?;
    let observed = fields(&pool, &g).await?;
    ensure!(observed.2 == 1);
    ensure!(matches!(writer.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(fields(&pool, &g).await? == observed && !j_present(&pool, j.intent.intent_id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_competing_births_publish_one_current_admission(
    pool: PgPool,
) -> Result<()> {
    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::{Config, RngSeed, TestRunner},
    };
    hint_schema(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0xb5bcb71d),
        failure_persistence: None,
        ..Config::default()
    });
    let phases = (0_u64..=1800, 0_u64..=1800);
    for i in 0..16 {
        let (a_delay, b_delay) = phases.new_tree(&mut runner).expect("bounded schedule").current();
        let g = seed_generation(&pool).await?;
        let start = fields(&pool, &g).await?.0;
        let a = arm(&pool, &g).await?;
        let mut b = a.clone();
        b.intent.intent_id = Uuid::now_v7();
        b.intent.client_order_id = format!("other-{}", b.intent.intent_id.simple());
        let position = a.intent.position_id.unwrap();
        let a_store =
            PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
        let b_store =
            PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
        let observe = async {
            for _ in 0..3 {
                let values:(i64,i64,i64,i64)=sqlx::query_as(
                    "SELECT a.version,g.accounting_version,
                     (SELECT count(*) FROM balance_legs WHERE position_id=$2),
                     (SELECT count(*) FROM balance_leg_admissions WHERE position_id=$2 AND superseded_at IS NULL)
                     FROM accounting_state a JOIN spot_order_set_generation g USING(tenant_id,venue_id)
                     WHERE g.generation_id=$1",
                ).bind(g.generation_id).bind(position).fetch_one(&pool).await?;
                ensure!(
                    values.0 == values.1
                        && values.2 == values.3
                        && values.0 == start + values.2
                        && (0..=1).contains(&values.2)
                );
                tokio::task::yield_now().await;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (one, two, seen) = tokio::join!(
            async {
                tokio::time::sleep(Duration::from_micros(a_delay)).await;
                a_store.commit(&a).await
            },
            async {
                tokio::time::sleep(Duration::from_micros(b_delay)).await;
                b_store.commit(&b).await
            },
            observe,
        );
        seen?;
        let (landed, refused) = match (one, two) {
            (Ok(()), Err(CommitFailure::Known(_))) => (&a, &b),
            (Err(CommitFailure::Known(_)), Ok(())) => (&b, &a),
            other => anyhow::bail!("admission schedule {i}: {other:?}"),
        };
        assert_current(&pool, &g, landed, start + 1).await?;
        ensure!(!j_present(&pool, refused.intent.intent_id).await?);
    }
    eprintln!("Entry PG16: 16 generated races, each one exact current admission");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_local_hints_do_not_use_foreign_or_unknown_position(
    pool: PgPool,
) -> Result<()> {
    hint_schema(&pool).await?;
    let g = seed_generation(&pool).await?;
    let j = arm(&pool, &g).await?;
    let position = j.intent.position_id.unwrap();
    for (a_scope, tenant, generation, source) in [
        (g.account_scope.as_str(), Uuid::now_v7(), g.generation_id, Some(position)),
        ("foreign-account", g.tenant_id, g.generation_id, Some(position)),
        (g.account_scope.as_str(), g.tenant_id, Uuid::now_v7(), Some(position)),
        (g.account_scope.as_str(), g.tenant_id, g.generation_id, Some(Uuid::now_v7())),
        (g.account_scope.as_str(), g.tenant_id, g.generation_id, None),
    ] {
        sqlx::query(
            "INSERT INTO spot_order_set_cancel_members
             (tenant_id,venue_id,account_scope,generation_id,intent_id,target_id,pending,conflict,source_position)
             VALUES($1,'binance:spot',$2,$3,$4,'4100001',true,false,$5)",
        ).bind(tenant).bind(a_scope).bind(generation).bind(Uuid::now_v7()).bind(source)
        .execute(&pool).await?;
    }
    let initial = fields(&pool, &g).await?.0;
    let writer = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    insist(writer.commit(&j).await)?;
    assert_current(&pool, &g, &j, initial + 1).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_rejects_unsupported_payload_and_preserves_exact_current_terms(
    pool: PgPool,
) -> Result<()> {
    hint_schema(&pool).await?;
    let g = seed_generation(&pool).await?;
    let j = arm(&pool, &g).await?;
    let initial = fields(&pool, &g).await?;
    let writer = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let wrong =
        PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(JournalGeneration {
            source_audit_id: Uuid::now_v7(),
            ..g.clone()
        });
    ensure!(matches!(wrong.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(!j_present(&pool, j.intent.intent_id).await? && fields(&pool, &g).await? == initial);
    let (wire, proxy, _, _) = proxied_journal(&pool, &g).await?;
    let mut invalid = j.clone();
    Arc::make_mut(invalid.balance.as_mut().unwrap().admission.as_mut().unwrap()).position_id =
        Uuid::now_v7();
    ensure!(matches!(wire.commit(&invalid).await, Err(CommitFailure::Known(_))));
    invalid = j.clone();
    invalid.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    ensure!(matches!(wire.commit(&invalid).await, Err(CommitFailure::Known(_))));
    ensure!(proxy.query_trace().is_empty());

    insist(writer.commit(&j).await)?;
    let mut changed = j.clone();
    Arc::make_mut(changed.balance.as_mut().unwrap().admission.as_mut().unwrap()).planned_qty =
        Decimal::new(2, 0);
    ensure!(matches!(writer.commit(&changed).await, Err(CommitFailure::Known(_))));
    assert_current(&pool, &g, &j, initial.0 + 1).await?;
    let new =
        crate::process_epoch::start_epoch(&pool, g.tenant_id, "fresh-entry-successor").await?;
    ensure!(new.predecessor_epoch_id == Some(g.process_epoch));
    ensure!(matches!(writer.commit(&j).await, Err(CommitFailure::Known(_))));
    assert_current(&pool, &g, &j, initial.0 + 1).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn projected_entry_rolls_back_real_clocked_admission_when_projection_refuses(
    pool: PgPool,
) -> Result<()> {
    hint_schema(&pool).await?;
    let clock = native_clock(&pool).await?;
    let g = seed_generation(&pool).await?;
    let mut j = arm(&pool, &g).await?;
    let input = clocked_attempt(&clock, &g, &mut j)?;
    let id = j.intent.intent_id;
    let attempt = j.attempts[0].attempt_id;
    let position = j.intent.position_id.unwrap();
    let initial = fields(&pool, &g).await?;
    sqlx::raw_sql(
        "CREATE FUNCTION suppress_entry_version() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RETURN NULL; END $$;
         CREATE TRIGGER suppress_entry_version BEFORE UPDATE OF accounting_version ON spot_order_set_generation
           FOR EACH ROW EXECUTE FUNCTION suppress_entry_version()",
    ).execute(&pool).await?;
    let writer = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    ensure!(matches!(writer.commit_clocked(&j, &input).await, Err(CommitFailure::Known(_))));
    let pending: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM order_intents WHERE intent_id=$1),
                EXISTS(SELECT 1 FROM submit_attempts WHERE attempt_id=$2),
                EXISTS(SELECT 1 FROM spot_intent_clock WHERE attempt_id=$2),
                EXISTS(SELECT 1 FROM balance_legs WHERE position_id=$3)",
    )
    .bind(id)
    .bind(attempt)
    .bind(position)
    .fetch_one(&pool)
    .await?;
    ensure!(pending == (false, false, false, false) && fields(&pool, &g).await? == initial);
    sqlx::query("DROP TRIGGER suppress_entry_version ON spot_order_set_generation")
        .execute(&pool)
        .await?;
    ensure!(
        writer
            .commit_clocked(&j, &input)
            .await
            .map_err(|err| anyhow::anyhow!("{err:?}"))?
            .len()
            == 1
    );
    assert_current(&pool, &g, &j, initial.0 + 1).await
}
