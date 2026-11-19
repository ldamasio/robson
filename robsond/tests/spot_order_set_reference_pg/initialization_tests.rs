//! Native fixture initialization at a fenced source/observer cutpoint.
use std::time::Duration;

use anyhow::{anyhow, ensure, Context};
use robson_testkit::db_proxy::ProxyMode;
use sqlx::{ConnectOptions, Executor, PgConnection};
use tokio::time::{timeout_at, Instant};

use super::{
    audit, batch, cold_cancels, cold_sells, empty_budget, linked_sell, scope, seen,
    through_proxy_limit, unowned, zero_execution_order, AttemptRow, Fixture, Hot, IntentKind,
    JournalBackend, OrderPhase, OutcomeRow, PgJournalBackend, PgPool, Scope, Uuid, VenueOutcome,
    HOT,
};

const FIXTURE_REVISION: &str = "e3322d200cccf21eed13d6b3f4a3ebfbff1f835e";
const FENCE: &str = "LOCK TABLE accounting_state, leg_routes, balance_legs, order_intents,
                    submit_attempts, venue_orders IN ACCESS EXCLUSIVE MODE NOWAIT";
const CONFIGURE: &str = "SELECT set_config('lock_timeout','100ms',true),
                         set_config('statement_timeout','2s',true),
                         set_config('idle_in_transaction_session_timeout','2s',true)";
const GENERATION_DDL: &str = "CREATE TABLE spot_shadow_initialization (
        generation uuid PRIMARY KEY, fixture_database name NOT NULL, fixture_revision text NOT NULL,
        coverage jsonb NOT NULL)";
const INITIALIZATION_ACK: &str = "SELECT EXISTS (SELECT FROM spot_shadow_initialization
       WHERE generation=$1 AND fixture_database=current_database()
         AND fixture_database=$2::name AND fixture_revision=$3)";
const REPLAY: &str = include_str!("initialization_replay.sql");

#[derive(Debug)]
struct ProjectionReady {
    generation: Uuid,
    database: String,
}

#[derive(Clone, Copy)]
enum ReplayOrder {
    MembersFirst,
    TargetsFirst,
}

impl ReplayOrder {
    fn name(self) -> &'static str {
        match self {
            Self::MembersFirst => "members_first",
            Self::TargetsFirst => "targets_first",
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct InitializedOrderSet {
    #[sqlx(flatten)]
    facts: Hot,
    unbound_sell: i64,
    unbound_sell_conflicts: i64,
}

impl ProjectionReady {
    async fn read(
        &self,
        pool: &PgPool,
        s: Scope,
        stop: Option<&str>,
    ) -> anyhow::Result<Option<InitializedOrderSet>> {
        // The two aggregate namespaces and generation come from exact keys.
        // A missing generation suppresses the entire row, including zeroes.
        let sql = format!(
            "SELECT hot.*, coalesce(u.potential_sell, 0::bigint) AS unbound_sell,
                    coalesce(u.conflict_count, 0::bigint) AS unbound_sell_conflicts
             FROM spot_shadow_initialization g
             CROSS JOIN LATERAL ({HOT} AND g.generation=$7 OFFSET 0) hot
             LEFT JOIN sell_shadow_unbound u
               ON (u.tenant_id,u.venue_id,u.account_scope)=($1,$2,$3)
             WHERE g.generation=$7 AND g.fixture_database=current_database()
               AND g.fixture_database=$8::name AND g.fixture_revision=$9"
        );
        let facts = sqlx::query_as(&sql)
            .bind(s.tenant)
            .bind(s.venue)
            .bind(s.account)
            .bind(s.position)
            .bind(s.symbol)
            .bind(stop)
            .bind(self.generation)
            .bind(&self.database)
            .bind(FIXTURE_REVISION)
            .fetch_optional(pool)
            .await?;
        Ok(facts)
    }
}

async fn own_connection(pool: &PgPool) -> anyhow::Result<PgConnection> {
    Ok(pool.connect_options().connect().await?)
}

async fn initialize(
    pool: &PgPool,
    replay_order: ReplayOrder,
    fail_member: Option<&str>,
) -> anyhow::Result<ProjectionReady> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let generation = Uuid::now_v7();
    let database = pool.connect_options().get_database().context("fixture database")?.to_owned();
    let ready = ProjectionReady { generation, database };
    let mut commit_started = false;
    let initialize = async {
        let mut conn = own_connection(pool).await?;
        sqlx::raw_sql("BEGIN ISOLATION LEVEL READ COMMITTED").execute(&mut conn).await?;
        sqlx::query(CONFIGURE).execute(&mut conn).await?;
        // The first relation reference locks the full exact source set. All
        // data reads and observer DDL are below this cutpoint.
        sqlx::raw_sql(FENCE).execute(&mut conn).await?;
        sqlx::raw_sql(include_str!("cancel_shadow.sql")).execute(&mut conn).await?;
        sqlx::raw_sql(include_str!("sell_shadow.sql")).execute(&mut conn).await?;
        sqlx::raw_sql(GENERATION_DDL).execute(&mut conn).await?;
        if let Some(member) = fail_member {
            sqlx::query("INSERT INTO sell_shadow_fail_member (member_key) VALUES ($1)")
                .bind(member)
                .execute(&mut conn)
                .await?;
        }
        sqlx::query("SELECT set_config('robson_test.replay_cancel_order',$1,true)")
            .bind(replay_order.name())
            .execute(&mut conn)
            .await?;
        sqlx::raw_sql(REPLAY).execute(&mut conn).await?;
        sqlx::query(
            "INSERT INTO spot_shadow_initialization VALUES (
            $1,current_database(),$2,current_setting('robson_test.replay_coverage',true)::jsonb)",
        )
        .bind(ready.generation)
        .bind(FIXTURE_REVISION)
        .execute(&mut conn)
        .await?;
        commit_started = true;
        let completed = conn.execute(sqlx::raw_sql("COMMIT")).await?;
        ensure!(completed.rows_affected() == 0);
        anyhow::Ok(())
    };
    let failure = match timeout_at(deadline, initialize).await {
        Ok(Ok(())) => return Ok(ready),
        Ok(Err(error)) => error,
        Err(_) => anyhow!("Spot initialization deadline"),
    };
    if !commit_started {
        return Err(failure);
    }
    // The original raw connection was retired before any exact observation.
    // Absence here is unresolved, which never opens a generation or retries it.
    let confirmation = timeout_at(deadline, async {
        let mut conn = own_connection(pool).await?;
        let observed: bool = sqlx::query_scalar(INITIALIZATION_ACK)
            .bind(ready.generation)
            .bind(&ready.database)
            .bind(FIXTURE_REVISION)
            .fetch_one(&mut conn)
            .await?;
        anyhow::Ok(observed)
    })
    .await;
    match confirmation {
        Ok(Ok(true)) => Ok(ready),
        _ => Err(failure.context("Spot initialization commit unresolved")),
    }
}

async fn check(
    pool: &PgPool,
    ready: &ProjectionReady,
    s: Scope,
) -> anyhow::Result<InitializedOrderSet> {
    let hot = ready.read(pool, s, None).await?.context("exact initialized order set")?;
    let cold = audit(pool, s)
        .await
        .map_err(|error| anyhow!("exhaustive reference: {error:?}"))?;
    ensure!(hot.facts.position_id == s.position && hot.facts.accounting_version == cold.version);
    ensure!(hot.facts.sells() == cold_sells(&cold));
    ensure!(hot.facts.cancels() == cold_cancels(&cold));
    Ok(hot)
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_replays_existing_journal_and_venue_state(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);

    let terminal = batch(s, IntentKind::Exit, None);
    let terminal_link = linked_sell(&f, &terminal.intent);
    journal.commit(&terminal).await?;
    journal.commit(&batch(s, IntentKind::Cancel, Some("100"))).await?;
    let mut final_order = zero_execution_order("100", "CANCELED", OrderPhase::Terminal);
    final_order.client_order_id = terminal_link.client_order_id.clone();
    f.ledger.record_order(&final_order, &terminal_link, seen(2)).await?;
    journal.commit(&batch(s, IntentKind::Cancel, Some("100"))).await?;

    let pending = batch(s, IntentKind::Exit, None);
    let pending_link = linked_sell(&f, &pending.intent);
    journal.commit(&pending).await?;
    let mut open_order = zero_execution_order("101", "NEW", OrderPhase::Live);
    open_order.client_order_id = pending_link.client_order_id.clone();
    f.ledger.record_order(&open_order, &pending_link, seen(3)).await?;
    let mut rejected = batch(s, IntentKind::Exit, None);
    rejected.attempts.push(AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: seen(3).at,
        rejected_at: Some(seen(4).at),
    });
    rejected.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    journal.commit(&rejected).await?;
    f.ledger
        .record_order(&zero_execution_order("102", "NEW", OrderPhase::Live), &unowned(), seen(4))
        .await?;

    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    let existing = check(&pool, &ready, s).await?;
    ensure!((existing.facts.leg_sell, existing.facts.symbol_sell) == (1, 1));
    ensure!(existing.facts.grouped_cancel == 0);
    ensure!(existing.unbound_sell == 0 && existing.unbound_sell_conflicts == 0);
    let wrong = ProjectionReady {
        generation: Uuid::now_v7(),
        database: ready.database.clone(),
    };
    ensure!(wrong.read(&pool, s, None).await?.is_none());
    let (proxy, measured) = through_proxy_limit(&pool, 1).await;
    let before = proxy.stats().statements;
    ensure!(ready.read(&measured, s, None).await?.is_some());
    ensure!(proxy.stats().statements - before == 1, "one exact initialized hot read");
    measured.close().await;

    open_order.phase = OrderPhase::Terminal;
    open_order.status = "CANCELED".to_owned();
    f.ledger.record_order(&open_order, &pending_link, seen(5)).await?;
    let after = check(&pool, &ready, s).await?;
    ensure!(after.facts.leg_sell == 0 && after.facts.symbol_sell == 1);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_commit_ack_requires_exact_generation(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let (proxy, measured) = through_proxy_limit(&pool, 1).await;
    proxy.enable_query_trace(64);
    proxy.set_mode(ProxyMode::DropCommitAck);
    let ready = initialize(&measured, ReplayOrder::TargetsFirst, None).await?;
    ensure!(proxy.stats().commit_acks_dropped == 1);
    ensure!(proxy.query_trace().iter().any(|statement| statement.contains("generation=$1")));
    let empty = check(&pool, &ready, s).await?;
    ensure!(empty.facts.leg_sell == 0 && empty.facts.grouped_cancel == 0);
    measured.close().await;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_refuses_conflict_and_retires_failed_replay(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    let pending = batch(s, IntentKind::Exit, None);
    journal.commit(&pending).await?;

    let mut writer = pool.begin().await?;
    sqlx::query(
        "SELECT version FROM accounting_state
                 WHERE tenant_id=$1 AND venue_id=$2 FOR UPDATE",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .fetch_one(&mut *writer)
    .await?;
    let blocked = initialize(&pool, ReplayOrder::TargetsFirst, None).await.unwrap_err();
    ensure!(
        blocked
            .downcast_ref::<sqlx::Error>()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .as_deref()
            == Some("55P03")
    );
    writer.rollback().await?;

    let injected = pending.intent.intent_id.to_string();
    ensure!(initialize(&pool, ReplayOrder::TargetsFirst, Some(&injected)).await.is_err());
    let shadow_absent: bool = sqlx::query_scalar(
        "SELECT
        to_regclass('spot_shadow_initialization') IS NULL AND
        to_regclass('sell_shadow_intents') IS NULL AND
        to_regclass('cancel_shadow_members') IS NULL",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(shadow_absent, "a failed replay published no table or readiness");
    let still_canonical = audit(&pool, s).await.map_err(|error| anyhow!("{error:?}"))?;
    ensure!(cold_sells(&still_canonical).leg.active == 1);
    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    ensure!(check(&pool, &ready, s).await?.facts.leg_sell == 1);
    Ok(())
}
