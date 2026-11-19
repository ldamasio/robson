//! Native atomic journal-to-projection evidence, in an isolated test database.
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use robson_exec::{
    order_journal::{AttemptRow, BalanceBatch, IntentKind, IntentRow, OutcomeRow, VenueOutcome},
    venue_clock::NativeClock,
};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use sqlx::{postgres::PgSslMode, ConnectOptions};

use super::*;
use crate::{
    spot_order_set_store::JournalGeneration,
    venue_budget::{fixture, Trace},
};

const ACCOUNT: &str = "projected-journal";
const SCOPED: &str = "SELECT a.version, g.accounting_version,
    coalesce(t.potential_count,0),coalesce(t.conflict_count,0),
    (SELECT count(*) FROM public.spot_order_set_sell_intents m
     WHERE (m.tenant_id,m.venue_id,m.account_scope,m.generation_id)
         = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id) AND m.potential),
    (SELECT count(*) FROM public.spot_order_set_sell_intents m
     WHERE (m.tenant_id,m.venue_id,m.account_scope,m.generation_id)
         = (g.tenant_id,g.venue_id,g.account_scope,g.generation_id) AND m.conflict)
    FROM public.accounting_state a
    JOIN public.spot_order_set_generation g USING (tenant_id,venue_id)
    LEFT JOIN public.spot_order_set_sell_unbound t
      USING (tenant_id,venue_id,account_scope,generation_id)
    WHERE (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)
        = ($1,'binance:spot',$2,$3)";

async fn seed_generation(pool: &PgPool) -> Result<JournalGeneration> {
    let tenant_id = Uuid::now_v7();
    let generation = JournalGeneration {
        tenant_id,
        account_scope: ACCOUNT.to_owned(),
        generation_id: Uuid::now_v7(),
        process_epoch: crate::process_epoch::start_epoch(pool, tenant_id, "projected-journal")
            .await?
            .epoch
            .epoch_id,
        source_audit_id: Uuid::now_v7(),
    };
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO accounting_state
         (tenant_id,venue_id,budget_model)
         VALUES ($1,'binance:spot','net_from_start_non_expanding_v1') RETURNING version",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO spot_order_set_generation
         (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,accounting_version)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)",
    ).bind(tenant_id).bind(ACCOUNT).bind(generation.generation_id)
        .bind(generation.process_epoch).bind(generation.source_audit_id).bind(version)
        .execute(pool).await?;
    Ok(generation)
}

fn sell() -> JournalBatch {
    let intent_id = Uuid::now_v7();
    JournalBatch {
        intent: IntentRow {
            intent_id,
            venue_id: VenueId::BINANCE_SPOT.to_owned(),
            account_scope: ACCOUNT.to_owned(),
            client_order_id: format!("p-{}", intent_id.simple()),
            position_id: Some(Uuid::now_v7()),
            kind: IntentKind::Exit,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at: Utc::now(),
        },
        attempts: Vec::new(),
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    }
}

async fn fields(pool: &PgPool, g: &JournalGeneration) -> Result<(i64, i64, i64, i64, i64, i64)> {
    Ok(sqlx::query_as(SCOPED)
        .bind(g.tenant_id)
        .bind(&g.account_scope)
        .bind(g.generation_id)
        .fetch_one(pool)
        .await?)
}

async fn j_present(pool: &PgPool, id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM order_intents WHERE intent_id=$1)")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

fn insist(result: std::result::Result<(), CommitFailure>) -> Result<()> {
    result.map_err(|e| anyhow::anyhow!("journal persistence: {e:?}"))
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_commits_replays_and_clears_exact_unbound_member(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let (v, _, _, _, _, _) = fields(&pool, &g).await?;
    let mut j = sell();
    insist(stored.commit(&j).await)?;
    ensure!(j_present(&pool, j.intent.intent_id).await?);
    ensure!(fields(&pool, &g).await? == (v + 1, v + 1, 1, 0, 1, 0));
    insist(stored.commit(&j).await)?;
    ensure!(fields(&pool, &g).await? == (v + 2, v + 2, 1, 0, 1, 0));
    let hint: Option<Uuid> = sqlx::query_scalar("SELECT source_position
        FROM spot_order_set_sell_intents WHERE intent_id=$1 AND scope_kind='unbound' AND scope_position IS NULL")
        .bind(j.intent.intent_id).fetch_one(&pool).await?;
    ensure!(
        hint == j.intent.position_id,
        "source hint must preserve only raw intent position"
    );
    j.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    insist(stored.commit(&j).await)?;
    ensure!(fields(&pool, &g).await? == (v + 3, v + 3, 0, 0, 0, 0));
    let still_member: String =
        sqlx::query_scalar("SELECT scope_kind FROM spot_order_set_sell_intents WHERE intent_id=$1")
            .bind(j.intent.intent_id)
            .fetch_one(&pool)
            .await?;
    ensure!(still_member == "none");
    let mut never_active = sell();
    never_active.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    insist(stored.commit(&never_active).await)?;
    insist(stored.commit(&never_active).await)?;
    ensure!(fields(&pool, &g).await? == (v + 5, v + 5, 0, 0, 0, 0));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_foreign_attempt_cannot_change_original_owner(
    pool: PgPool,
) -> Result<()> {
    let clock = native_clock(&pool).await?;
    let g = seed_generation(&pool).await?;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let mut original = sell();
    let input = clocked_attempt(&clock, &g, &mut original)?;
    let AttemptRow { attempt_id, created_at, .. } = original.attempts[0];
    original.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    insist(stored.commit_clocked(&original, &input).await.map(drop))?;
    let before = fields(&pool, &g).await?;
    ensure!((before.2, before.3, before.4, before.5) == (1, 1, 1, 1));
    let mut foreign = sell();
    foreign.attempts.push(AttemptRow {
        attempt_id,
        created_at,
        rejected_at: Some(Utc::now()),
    });
    ensure!(matches!(stored.commit(&foreign).await, Err(CommitFailure::Known(message))
        if message.contains("another intent")));
    let attempt: (String, String, bool) = sqlx::query_as(
        "SELECT client_order_id,state,rejected_at IS NULL FROM submit_attempts WHERE attempt_id=$1",
    )
    .bind(attempt_id)
    .fetch_one(&pool)
    .await?;
    ensure!(attempt == (original.intent.client_order_id, "started".to_owned(), true));
    ensure!(fields(&pool, &g).await? == before);
    ensure!(!j_present(&pool, foreign.intent.intent_id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_missing_member_or_generation_refuses_without_source_change(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let mut original = sell();
    insist(stored.commit(&original).await)?;
    sqlx::query("DELETE FROM public.spot_order_set_sell_intents WHERE intent_id=$1")
        .bind(original.intent.intent_id)
        .execute(&pool)
        .await?;
    let before = fields(&pool, &g).await?;
    original.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    ensure!(matches!(stored.commit(&original).await, Err(CommitFailure::Known(_))));
    let unchanged: bool =
        sqlx::query_scalar("SELECT NOT not_sent FROM order_intents WHERE intent_id=$1")
            .bind(original.intent.intent_id)
            .fetch_one(&pool)
            .await?;
    ensure!(unchanged && fields(&pool, &g).await? == before);
    let missing =
        PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(JournalGeneration {
            generation_id: Uuid::now_v7(),
            ..g.clone()
        });
    let j = sell();
    ensure!(matches!(missing.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(fields(&pool, &g).await? == before && !j_present(&pool, j.intent.intent_id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_refusal_or_native_projection_failure_rolls_back(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let before = fields(&pool, &g).await?;
    let mut foreign = sell();
    foreign.intent.account_scope = "other-account".to_owned();
    ensure!(matches!(stored.commit(&foreign).await, Err(CommitFailure::Known(_))));
    ensure!(
        fields(&pool, &g).await? == before && !j_present(&pool, foreign.intent.intent_id).await?
    );

    sqlx::raw_sql(
        "CREATE FUNCTION projected_member_failure() RETURNS trigger LANGUAGE plpgsql
        AS $$ BEGIN RAISE EXCEPTION 'native projection rejection'; END; $$;
        CREATE TRIGGER projected_member_failure AFTER INSERT ON public.spot_order_set_sell_intents
        FOR EACH ROW EXECUTE FUNCTION projected_member_failure()",
    )
    .execute(&pool)
    .await?;
    let j = sell();
    let failure = stored.commit(&j).await;
    ensure!(
        matches!(failure,Err(CommitFailure::Known(message)) if message.contains("native projection rejection"))
    );
    ensure!(fields(&pool, &g).await? == before && !j_present(&pool, j.intent.intent_id).await?);
    sqlx::raw_sql("DROP TRIGGER projected_member_failure ON public.spot_order_set_sell_intents")
        .execute(&pool)
        .await?;

    // A real older/nonprojecting writer remains available on the local legacy path.
    let legacy = PgJournalBackend::new(&pool, g.tenant_id);
    let old = sell();
    insist(legacy.commit(&old).await)?;
    let stale = fields(&pool, &g).await?;
    ensure!(stale == (before.0 + 1, before.1, 0, 0, 0, 0));
    ensure!(matches!(stored.commit(&j).await, Err(CommitFailure::Known(_))));
    ensure!(fields(&pool, &g).await? == stale && !j_present(&pool, j.intent.intent_id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_lost_commit_proves_original_atomic_state(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let (stored, proxy, pid, birth) = proxied_journal(&pool, &g).await?;
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE public.spot_order_set_generation g"));
    let before = fields(&pool, &g).await?;
    let j = sell();
    ensure!(matches!(stored.commit(&j).await, Err(CommitFailure::Uncertain(_))));
    ensure!(proxy.stats().commit_acks_dropped == 1);
    native_retired(&pool, pid, &birth).await?;
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 6 && proxy.query_trace_dropped() == 0,
        "two extra accounting executions, one projection and no queued rollback: {trace:?}"
    );
    ensure!(trace[4].contains("UPDATE public.spot_order_set_generation g") && trace[5] == "COMMIT");
    ensure!(j_present(&pool, j.intent.intent_id).await?);
    ensure!(fields(&pool, &g).await? == (before.0 + 1, before.1 + 1, 1, 0, 1, 0));
    eprintln!("Persisted journal native lost COMMIT ACK: 6 wire commands, original retired");
    Ok(())
}

async fn proxied_journal(
    pool: &PgPool,
    g: &JournalGeneration,
) -> Result<(PgJournalBackend, DbProxy, i32, String)> {
    let upstream = pool.connect_options();
    let server = tokio::net::lookup_host((upstream.get_host(), upstream.get_port()))
        .await?
        .next()
        .context("isolated native journal endpoint")?;
    ensure!(server.ip().is_loopback());
    let proxy = DbProxy::start(server).await?;
    let options = upstream
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(proxy.addr().port())
        .ssl_mode(PgSslMode::Disable);
    let own = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(options)
        .await?;
    let (pid, birth): (i32, String) = sqlx::query_as(
        "SELECT pid,backend_start::text
        FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    )
    .fetch_one(&own)
    .await?;
    proxy.enable_exact_query_trace(12);
    let stored = PgJournalBackend {
        pool: own,
        daemon_pool: pool.clone(),
        tenant_id: g.tenant_id,
        ready_wait: JOURNAL_READY_WAIT,
        order_set_projection: Some(g.clone()),
    };
    Ok((stored, proxy, pid, birth))
}

async fn native_retired(pool: &PgPool, pid: i32, birth: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_millis(350), async {
        loop {
            let alive: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE pid=$1 AND backend_start::text=$2)",
            )
            .bind(pid)
            .bind(birth)
            .fetch_one(pool)
            .await?;
            if !alive {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    })
    .await
    .context("original projected journal backend still present")??;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_caller_cancel_at_native_lock_retires_and_rolls_back(
    pool: PgPool,
) -> Result<()> {
    let g = seed_generation(&pool).await?;
    let (stored, proxy, pid, birth) = proxied_journal(&pool, &g).await?;
    sqlx::query(
        "INSERT INTO public.spot_order_set_sell_unbound
         (tenant_id,venue_id,account_scope,generation_id,potential_count,conflict_count)
         VALUES($1,'binance:spot',$2,$3,0,0)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .execute(&pool)
    .await?;
    let before = fields(&pool, &g).await?;
    let mut hold = pool.begin().await?;
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *hold).await?;
    // A row lock leaves PostgreSQL free to prepare and start the real
    // projection. The upsert then waits inside that statement.
    let locked: bool = sqlx::query_scalar(
        "SELECT true FROM public.spot_order_set_sell_unbound
         WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3)
         FOR UPDATE",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .fetch_one(&mut *hold)
    .await?;
    ensure!(locked);
    let j = sell();
    {
        let mut write = Box::pin(stored.commit(&j));
        let observe = tokio::time::timeout(Duration::from_millis(180), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                    SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2
                    AND wait_event_type='Lock' AND $3=ANY(pg_blocking_pids(pid))
                    AND query LIKE '%WITH journal AS MATERIALIZED%')",
                )
                .bind(pid)
                .bind(&birth)
                .bind(holder)
                .fetch_one(&pool)
                .await?;
                if waiting {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        tokio::select! {
            returned=&mut write => anyhow::bail!("original returned before caller cut: {returned:?}"),
            witnessed=observe => witnessed.context("exact native projection row lock")??,
        }
        let trace = proxy.query_trace();
        ensure!(trace.len()==5 && trace[4].contains("UPDATE public.spot_order_set_generation g"),
            "before the caller cut the original journal and projection attempt must reach PostgreSQL: {trace:?}");
        drop(write);
    }
    hold.rollback().await?;
    native_retired(&pool, pid, &birth).await?;
    ensure!(proxy.stats().commits == 0 && proxy.query_trace().len() == 5);
    ensure!(fields(&pool, &g).await? == before && !j_present(&pool, j.intent.intent_id).await?);
    eprintln!("Persisted journal native caller cancellation: actual aggregate row lock, 5 wire commands and retired rollback");
    Ok(())
}

async fn native_clock(pool: &PgPool) -> Result<NativeClock> {
    let clock = NativeClock::initialize()?;
    fixture::prepare(
        pool,
        &clock,
        tokio::time::Instant::now() + Duration::from_secs(2),
        &Trace::default(),
    )
    .await
    .map_err(|error| anyhow::anyhow!("budget fixture: {error:?}"))?;
    Ok(clock)
}

fn clocked_attempt(
    clock: &NativeClock,
    g: &JournalGeneration,
    j: &mut JournalBatch,
) -> Result<clocked::ClockedInput> {
    let capture = clock.wall_bracket(g.process_epoch)?;
    let wall = chrono::DateTime::from_timestamp(capture.wall_seconds, capture.wall_nanoseconds)
        .context("representable captured wall time")?;
    j.intent.created_at = wall;
    let attempt_id = Uuid::now_v7();
    j.attempts.push(AttemptRow {
        attempt_id,
        created_at: wall,
        rejected_at: None,
    });
    Ok(clocked::ClockedInput {
        process_epoch: g.process_epoch,
        attempts: vec![clocked::ClockedAttempt {
            attempt_id,
            intent_id: j.intent.intent_id,
            tenant_id: g.tenant_id,
            venue_id: j.intent.venue_id.clone(),
            account_scope: j.intent.account_scope.clone(),
            client_order_id: j.intent.client_order_id.clone(),
            kind: j.intent.kind,
            capture,
            sample: None,
            budget: clock.capture()?,
        }],
    })
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_clocked_projection_uses_real_attempt_evidence(
    pool: PgPool,
) -> Result<()> {
    let clock = native_clock(&pool).await?;
    let g = seed_generation(&pool).await?;
    let v = fields(&pool, &g).await?.0;
    let stored = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let mut j = sell();
    let input = clocked_attempt(&clock, &g, &mut j)?;
    let attempt_id = j.attempts[0].attempt_id;
    let receipts = stored
        .commit_clocked(&j, &input)
        .await
        .map_err(|e| anyhow::anyhow!("initial clocked projection: {e:?}"))?;
    ensure!(receipts.len() == 1 && fields(&pool, &g).await? == (v + 1, v + 1, 1, 0, 1, 0));
    j.attempts[0].rejected_at = Some(Utc::now());
    j.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    let receipts = stored
        .commit_clocked(&j, &input)
        .await
        .map_err(|e| anyhow::anyhow!("terminal clocked projection: {e:?}"))?;
    ensure!(receipts.len() == 1 && fields(&pool, &g).await? == (v + 2, v + 2, 0, 0, 0, 0));
    let real:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM submit_attempts WHERE attempt_id=$1 AND state='rejected' AND rejected_at IS NOT NULL)
        AND EXISTS(SELECT 1 FROM spot_intent_clock WHERE attempt_id=$1 AND intent_id=$2)")
        .bind(attempt_id).bind(j.intent.intent_id).fetch_one(&pool).await?;
    ensure!(real && j_present(&pool, j.intent.intent_id).await?);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn persisted_order_set_journal_two_writers_preserve_members_and_version(
    pool: PgPool,
) -> Result<()> {
    let clock = native_clock(&pool).await?;
    let g = seed_generation(&pool).await?;
    let left = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let right = PgJournalBackend::new(&pool, g.tenant_id).with_order_set_projection(g.clone());
    let input = clocked::ClockedInput {
        process_epoch: g.process_epoch,
        attempts: Vec::new(),
    };
    let initial = fields(&pool, &g).await?.0;
    for seed in 0_u8..16 {
        let mut a = sell();
        let mut b = sell();
        let witnessed = clocked_attempt(&clock, &g, &mut b)?;
        // Permute the complete 4-bit case space to vary competing writer
        // choices reproducibly without changing the set of checked branches.
        let mode = seed.wrapping_mul(13) & 15;
        let hints = [mode & 1 != 0, mode & 2 != 0, mode & 4 != 0, mode & 8 != 0];
        let (l, r) = tokio::join!(left.commit(&a), right.commit_clocked(&b, &witnessed));
        insist(l)?;
        ensure!(
            r.map_err(|e| anyhow::anyhow!("seed {seed} initial clock evidence: {e:?}"))?
                .len()
                == 1
        );
        a.outcome = Some(OutcomeRow {
            venue_outcome: Some(VenueOutcome::Rejected),
            not_sent: hints[0],
            ..OutcomeRow::default()
        });
        b.outcome = Some(OutcomeRow {
            not_sent: hints[1],
            venue_outcome: hints[2].then_some(VenueOutcome::Rejected),
            ..OutcomeRow::default()
        });
        if hints[2] {
            b.attempts[0].rejected_at = Some(Utc::now());
        }
        let (l, r) = if hints[2] {
            tokio::join!(left.commit(&b), right.commit_clocked(&b, &witnessed))
        } else {
            tokio::join!(left.commit(&a), right.commit_clocked(&b, &witnessed))
        };
        insist(l)?;
        ensure!(
            r.map_err(|e| anyhow::anyhow!("seed {seed} mixed transition evidence: {e:?}"))?
                .len()
                == 1
        );
        let (l, r) = if hints[3] {
            tokio::join!(left.commit(&b), right.commit_clocked(&a, &input))
        } else {
            tokio::join!(left.commit(&a), right.commit_clocked(&a, &input))
        };
        insist(l)?;
        insist(r.map(drop))?;
        let (av, gv, p, c, pm, cm) = fields(&pool, &g).await?;
        let (want_p, want_c, differences): (i64, i64, i64) = sqlx::query_as(
            "WITH expected AS (
                SELECT j.intent_id,j.position_id,
                    CASE WHEN j.not_sent THEN j.venue_outcome IS NOT NULL
                         WHEN j.venue_outcome='rejected' THEN s.rejected_at IS NULL
                         ELSE true END AS potential,
                    CASE WHEN j.venue_outcome='rejected'
                         THEN j.not_sent OR s.rejected_at IS NULL ELSE false END AS conflict
                FROM order_intents j LEFT JOIN submit_attempts s
                  USING (venue_id,account_scope,client_order_id)
                WHERE (j.tenant_id,j.venue_id,j.account_scope)=($1,'binance:spot',$2)
             ), actual AS (
                SELECT intent_id,source_position,scope_position,scope_kind,potential,conflict
                FROM public.spot_order_set_sell_intents
                WHERE (tenant_id,venue_id,account_scope,generation_id)=($1,'binance:spot',$2,$3)
             )
             SELECT count(*) FILTER (WHERE j.potential),count(*) FILTER (WHERE j.conflict),
               count(*) FILTER (WHERE j.intent_id IS NULL OR m.intent_id IS NULL OR m.scope_position IS NOT NULL
                OR m.source_position IS DISTINCT FROM j.position_id
                OR (m.potential,m.conflict) IS DISTINCT FROM (j.potential,j.conflict)
                OR m.scope_kind <> CASE WHEN j.potential OR j.conflict THEN 'unbound' ELSE 'none' END)
             FROM expected j FULL JOIN actual m USING (intent_id)",
        )
        .bind(g.tenant_id)
        .bind(ACCOUNT)
        .bind(g.generation_id)
        .fetch_one(&pool)
        .await?;
        ensure!(
            av == initial + 6 * ((seed as i64) + 1)
                && av == gv
                && (p, c, pm, cm) == (want_p, want_c, want_p, want_c),
            "seed {seed} disagrees with independent native J"
        );
        ensure!(differences == 0, "seed {seed} changed a member's identity or contribution");
        let native_attempt: Option<bool> = sqlx::query_scalar(
            "SELECT s.rejected_at IS NOT NULL FROM submit_attempts s
             JOIN spot_intent_clock c USING (attempt_id)
             WHERE (s.attempt_id,c.intent_id)=($1,$2)",
        )
        .bind(b.attempts[0].attempt_id)
        .bind(b.intent.intent_id)
        .fetch_optional(&pool)
        .await?;
        ensure!(
            native_attempt == Some(hints[2]),
            "seed {seed} native typed clock/attempt mismatch"
        );
    }
    eprintln!("Persisted journal native concurrent interleavings: 16 seeded cases, 96 writes, 32 verified clocked writes with attempts");
    Ok(())
}
