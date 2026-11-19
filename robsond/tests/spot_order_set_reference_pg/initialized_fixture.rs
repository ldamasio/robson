//! Fenced initialization of the Spot order-set fixture and its one exact
//! initialized read. Mounted by path in the reference target and in the
//! private first-cancel writer tests. It owns no runtime authority: a ready
//! value is created only by a completed fenced initialization below.
use std::time::Duration;

use anyhow::{anyhow, ensure, Context};
use sqlx::{
    postgres::{PgArguments, PgRow},
    query::QueryAs,
    ConnectOptions, Executor, FromRow, PgConnection, PgPool, Postgres,
};
use tokio::{
    sync::oneshot,
    time::{timeout_at, Instant},
};
use uuid::Uuid;

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
const HOT: &str = include_str!("bounded_order_set.sql");

/// The accepted initialized order-set columns, including the unbound SELL
/// namespace of the leg's accounting owner.
pub(super) const ORDER_SET_COLUMNS: &str = "hot.*,
    coalesce(u.potential_sell, 0::bigint) AS unbound_sell,
    coalesce(u.conflict_count, 0::bigint) AS unbound_sell_conflicts";

/// The one exact initialized read. `$1..$6` are the bounded order-set keys,
/// `$7..$9` the generation, database and revision. The two aggregate
/// namespaces and generation come from exact keys; a missing generation
/// suppresses the entire row, including zeroes. `joins` may only add keyed
/// columns to the same statement.
pub(super) fn statement(columns: &str, joins: &str) -> String {
    format!(
        "SELECT {columns}
         FROM spot_shadow_initialization g
         CROSS JOIN LATERAL ({HOT} AND g.generation=$7 OFFSET 0) hot
         LEFT JOIN sell_shadow_unbound u
           ON (u.tenant_id,u.venue_id,u.account_scope)=($1,$2,$3)
         {joins}
         WHERE g.generation=$7 AND g.fixture_database=current_database()
           AND g.fixture_database=$8::name AND g.fixture_revision=$9"
    )
}

#[derive(Clone, Copy)]
pub(super) struct Keys<'a> {
    pub(super) tenant: Uuid,
    pub(super) venue: &'a str,
    pub(super) account: &'a str,
    pub(super) position: Uuid,
    pub(super) symbol: &'a str,
}

/// An initialized generation. Neither clonable nor constructible outside this
/// module; it is kept until its owner drops it and is never re-initialized.
#[derive(Debug)]
pub(super) struct ProjectionReady {
    generation: Uuid,
    database: String,
}

impl ProjectionReady {
    pub(super) fn generation(&self) -> Uuid {
        self.generation
    }

    /// Binds `$1..$9` of a `statement` to this exact generation.
    pub(super) fn query_as<'q, T>(
        &'q self,
        sql: &'q str,
        keys: Keys<'q>,
        candidate: Option<&'q str>,
    ) -> QueryAs<'q, Postgres, T, PgArguments>
    where
        T: for<'r> FromRow<'r, PgRow>,
    {
        bound(sql, keys, candidate, self.generation, &self.database)
    }

    /// Negative control: the same statement under a generation that was
    /// never initialized.
    pub(super) fn foreign_query_as<'q, T>(
        &'q self,
        sql: &'q str,
        keys: Keys<'q>,
        candidate: Option<&'q str>,
    ) -> QueryAs<'q, Postgres, T, PgArguments>
    where
        T: for<'r> FromRow<'r, PgRow>,
    {
        bound(sql, keys, candidate, Uuid::now_v7(), &self.database)
    }
}

fn bound<'q, T>(
    sql: &'q str,
    keys: Keys<'q>,
    candidate: Option<&'q str>,
    generation: Uuid,
    database: &'q str,
) -> QueryAs<'q, Postgres, T, PgArguments>
where
    T: for<'r> FromRow<'r, PgRow>,
{
    sqlx::query_as(sql)
        .bind(keys.tenant)
        .bind(keys.venue)
        .bind(keys.account)
        .bind(keys.position)
        .bind(keys.symbol)
        .bind(candidate)
        .bind(generation)
        .bind(database)
        .bind(FIXTURE_REVISION)
}

#[derive(Clone, Copy)]
pub(super) enum ReplayOrder {
    MembersFirst,
    TargetsFirst,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    BeforeFence,
    AfterFence,
    BeforeConfirmation,
}

pub(super) struct InitProbe {
    stage: Stage,
    entered: oneshot::Sender<()>,
    proceed: oneshot::Receiver<()>,
}

pub(super) fn probe(stage: Stage) -> (InitProbe, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (enter, entered) = oneshot::channel();
    let (proceed, release) = oneshot::channel();
    (InitProbe { stage, entered: enter, proceed: release }, entered, proceed)
}

async fn pause(probe: &mut Option<InitProbe>, stage: Stage) -> anyhow::Result<()> {
    if probe.as_ref().is_some_and(|value| value.stage == stage) {
        let value = probe.take().context("initializer staging control")?;
        value
            .entered
            .send(())
            .map_err(|_| anyhow!("missing initializer stage observer"))?;
        value.proceed.await?;
    }
    Ok(())
}

impl ReplayOrder {
    fn name(self) -> &'static str {
        match self {
            Self::MembersFirst => "members_first",
            Self::TargetsFirst => "targets_first",
        }
    }
}

async fn own_connection(pool: &PgPool) -> anyhow::Result<PgConnection> {
    Ok(pool.connect_options().connect().await?)
}

pub(super) async fn initialize(
    pool: &PgPool,
    replay_order: ReplayOrder,
    fail_member: Option<&str>,
) -> anyhow::Result<ProjectionReady> {
    initialize_at(pool, replay_order, fail_member, Duration::from_secs(5), None).await
}

pub(super) async fn initialize_at(
    pool: &PgPool,
    replay_order: ReplayOrder,
    fail_member: Option<&str>,
    limit: Duration,
    mut probe: Option<InitProbe>,
) -> anyhow::Result<ProjectionReady> {
    let deadline = Instant::now() + limit;
    let generation = Uuid::now_v7();
    let database = pool.connect_options().get_database().context("fixture database")?.to_owned();
    let ready = ProjectionReady { generation, database };
    let mut commit_started = false;
    let initialize = async {
        let mut conn = own_connection(pool).await?;
        sqlx::raw_sql("BEGIN ISOLATION LEVEL READ COMMITTED").execute(&mut conn).await?;
        sqlx::query(CONFIGURE).execute(&mut conn).await?;
        pause(&mut probe, Stage::BeforeFence).await?;
        // The first relation reference locks the full exact source set. All
        // data reads and observer DDL are below this cutpoint.
        sqlx::raw_sql(FENCE).execute(&mut conn).await?;
        pause(&mut probe, Stage::AfterFence).await?;
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
        pause(&mut probe, Stage::BeforeConfirmation).await?;
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
