//! Native M38 global venue contention through two independent J2b writers.
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use sqlx::postgres::PgPoolOptions;

use super::*;

const VENUE: &str = "binance:spot";

async fn seed_with_one_unit_left(pool: &PgPool, tenant: Uuid) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO order_intents
          (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
         SELECT md5('j2b-capacity-intent-'||n)::uuid,$1,$2,'j2b-capacity-seed',
                'capacity-seed-'||n,'entry','{}'::jsonb,CURRENT_TIMESTAMP
         FROM generate_series(1,9999) AS series(n)",
    )
    .bind(tenant)
    .bind(VENUE)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO submit_attempts
          (attempt_id,venue_id,account_scope,client_order_id,state,created_at)
         SELECT md5('j2b-capacity-attempt-'||n)::uuid,$1,'j2b-capacity-seed',
                'capacity-seed-'||n,'started',CURRENT_TIMESTAMP
         FROM generate_series(1,9999) AS series(n)",
    )
    .bind(VENUE)
    .execute(&mut *tx)
    .await?;
    for first in (1_i32..=9999).step_by(1000) {
        sqlx::query(
            "INSERT INTO spot_intent_clock(attempt_id,intent_id,tenant_id,venue_id,attempt_created_at)
             SELECT md5('j2b-capacity-attempt-'||n)::uuid,md5('j2b-capacity-intent-'||n)::uuid,
                    $1,$2,CURRENT_TIMESTAMP
             FROM generate_series($3::int,$4::int) AS series(n)",
        )
        .bind(tenant)
        .bind(VENUE)
        .bind(first)
        .bind((first + 999).min(9999))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    ensure!(
        active_units(pool).await? == 9999,
        "actual trigger projected 9999 active charges"
    );
    Ok(())
}

async fn active_units(pool: &PgPool) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM budget_daily_charges
         WHERE venue_id=$1 AND daily_units=1 AND expiry_proof_id IS NULL",
    )
    .bind(VENUE)
    .fetch_one(pool)
    .await?)
}

async fn stored(pool: &PgPool, batch: &JournalBatch) -> anyhow::Result<(i64, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM order_intents WHERE intent_id=$1),
                (SELECT count(*) FROM submit_attempts WHERE attempt_id=$2),
                (SELECT count(*) FROM spot_intent_clock WHERE attempt_id=$2),
                (SELECT count(*) FROM budget_daily_charges WHERE attempt_id=$2)",
    )
    .bind(batch.intent.intent_id)
    .bind(batch.attempts[0].attempt_id)
    .fetch_one(pool)
    .await?)
}

async fn versions(pool: &PgPool, a: Uuid, b: Uuid) -> anyhow::Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$3),
                (SELECT version FROM accounting_state WHERE tenant_id=$2 AND venue_id=$3)",
    )
    .bind(a)
    .bind(b)
    .bind(VENUE)
    .fetch_one(pool)
    .await?)
}

async fn writer(pool: &PgPool, tenant: Uuid) -> anyhow::Result<(PgJournalBackend, i32)> {
    let dedicated_pool = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with((*pool.connect_options()).clone())
        .await?;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&dedicated_pool).await?;
    Ok((
        PgJournalBackend {
            pool: dedicated_pool,
            ..PgJournalBackend::new(pool, tenant)
        },
        pid,
    ))
}

async fn wait_on(
    pool: &PgPool,
    writer: i32,
    blocker: i32,
    peer: Option<i32>,
    deadline: Instant,
) -> anyhow::Result<()> {
    loop {
        let blockers: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
            .bind(writer)
            .fetch_one(pool)
            .await?;
        if blockers.contains(&blocker) || peer.is_some_and(|pid| blockers.contains(&pid)) {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "writer {writer} never queued on the held venue row");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn run_schedule(
    pool: &PgPool,
    first_writer: usize,
    same_account: bool,
    seed: u64,
) -> anyhow::Result<()> {
    let mut generator = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    });
    let (before_peer, before_release) = (0_u64..10, 0_u64..10)
        .new_tree(&mut generator)
        .map_err(|error| anyhow!("capacity schedule: {error:?}"))?
        .current();
    let a = scope(pool).await?;
    let b = tenant_scope(pool, NativeClock::initialize()?).await?;
    let before = versions(pool, a.tenant, b.tenant).await?;
    let tenants = [&a, &b];
    seed_with_one_unit_left(pool, a.tenant).await?;

    let mut hold = pool.begin().await?;
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *hold).await?;
    let venue: String =
        sqlx::query_scalar("SELECT venue_id FROM budget_venue_state WHERE venue_id=$1 FOR UPDATE")
            .bind(VENUE)
            .fetch_one(&mut *hold)
            .await?;
    ensure!(venue == VENUE, "global venue row locked by observer");

    let (backend0, pid0) = writer(pool, tenants[first_writer].tenant).await?;
    let (backend1, pid1) = writer(pool, tenants[1 - first_writer].tenant).await?;
    ensure!(pid0 != pid1 && pid0 != holder && pid1 != holder, "three separate connections");
    let (batch0, input0) = one(tenants[first_writer], "j2b-race-first-arrival")?;
    let (mut batch1, mut input1) = one(tenants[1 - first_writer], "j2b-race-second-arrival")?;
    if !same_account {
        batch1.intent.account_scope = "j2b-capacity-other-account".to_owned();
        input1.attempts[0].account_scope = batch1.intent.account_scope.clone();
    }
    let mut entered0 = Box::pin(backend0.commit_clocked(&batch0, &input0));
    let mut entered1 = Box::pin(backend1.commit_clocked(&batch1, &input1));

    // The witness only observes each writer queued behind the held row; a
    // loaded runner can take longer than 200 ms to show it (CI run
    // 44921877471), so it allows 5 s. The order is fixed by the polls.
    let witness_deadline = Instant::now() + Duration::from_secs(5);
    tokio::select! {
        early = &mut entered0 => anyhow::bail!("first writer finished behind held venue row: {early:?}"),
        observed = wait_on(pool, pid0, holder, None, witness_deadline) => observed?,
    }
    tokio::time::sleep(Duration::from_millis(before_peer)).await;
    tokio::select! {
        early = &mut entered0 => anyhow::bail!("first writer did not remain queued: {early:?}"),
        early = &mut entered1 => anyhow::bail!("second writer finished behind held venue row: {early:?}"),
        observed = wait_on(pool, pid1, holder, Some(pid0), witness_deadline) => observed?,
    }
    tokio::time::sleep(Duration::from_millis(before_release)).await;
    hold.rollback().await?;
    let (result0, result1) = tokio::join!(entered0, entered1);
    let (winning_scope, winner, winner_input, receipt, loser, failure) = match (result0, result1) {
        (Ok(receipt), Err(failure)) => {
            (tenants[first_writer], &batch0, &input0, receipt, &batch1, failure)
        },
        (Err(failure), Ok(receipt)) => {
            (tenants[1 - first_writer], &batch1, &input1, receipt, &batch0, failure)
        },
        others => anyhow::bail!("exactly one competing writer must commit: {others:?}"),
    };
    ensure!(
        matches!(failure, CommitFailure::Known(ref message)
            if message.contains("clock source binding or global daily budget is incomplete")),
        "loser failed on real M38 capacity: {failure:?}"
    );
    let expected = vec![ClockReceipt {
        attempt_id: winner.attempts[0].attempt_id,
        daily_units: 1,
        retired: false,
    }];
    ensure!(receipt == expected, "winner received one unit");
    ensure!(stored(pool, winner).await? == (1, 1, 1, 1), "winner landed atomically");
    ensure!(stored(pool, loser).await? == (0, 0, 0, 0), "all loser writes rolled back");
    ensure!(active_units(pool).await? == 10000, "venue reached exactly 10000");

    let after_commit = if winning_scope.tenant == a.tenant {
        (before.0 + 1, before.1)
    } else {
        (before.0, before.1 + 1)
    };
    ensure!(
        versions(pool, a.tenant, b.tenant).await? == after_commit,
        "the loser also rolled back the accounting version"
    );

    let backend = PgJournalBackend::new(pool, winning_scope.tenant);
    let (mut cancel, mut cancel_input) = one(winning_scope, "j2b-race-cancel")?;
    cancel.intent.kind = IntentKind::Cancel;
    cancel.intent.cancels_order_id = Some("4100019".to_owned());
    cancel_input.attempts[0].kind = IntentKind::Cancel;
    let cancel_receipt = vec![ClockReceipt {
        attempt_id: cancel.attempts[0].attempt_id,
        daily_units: 0,
        retired: false,
    }];
    ensure!(
        backend.commit_clocked(&cancel, &cancel_input).await == Ok(cancel_receipt.clone()),
        "cancel consumes zero units at full capacity"
    );
    ensure!(
        backend.commit_clocked(&cancel, &cancel_input).await == Ok(cancel_receipt)
            && backend.commit_clocked(winner, winner_input).await == Ok(expected),
        "exact replay of the cancel and placement at full capacity"
    );
    ensure!(stored(pool, &cancel).await? == (1, 1, 1, 1), "one durable cancel");
    ensure!(active_units(pool).await? == 10000, "cancel/replay add no active unit");
    eprintln!(
        "J2B_RACE seed={seed} first_writer={first_writer} same_account={same_account} peer_delay_ms={before_peer} release_delay_ms={before_release} active=10000 loser_rows=0"
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn first_tenant_reaches_the_shared_venue_lock_first(pool: PgPool) -> anyhow::Result<()> {
    run_schedule(&pool, 0, true, 2026111906).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn second_tenant_reaches_the_shared_venue_lock_first(pool: PgPool) -> anyhow::Result<()> {
    run_schedule(&pool, 1, true, 2026111907).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn venue_capacity_is_shared_across_distinct_account_scopes(
    pool: PgPool,
) -> anyhow::Result<()> {
    run_schedule(&pool, 0, false, 2026111908).await
}
