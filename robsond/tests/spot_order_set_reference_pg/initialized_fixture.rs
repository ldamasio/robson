//! Fenced initialization of the Spot order-set fixture and its one exact
//! initialized read. Mounted by path in the reference target and in the
//! private first-cancel writer tests. It owns no runtime authority: a ready
//! value is created only by a completed fenced initialization below.
use std::{collections::HashSet, sync::Mutex, time::Duration};

use anyhow::{anyhow, ensure, Context};
use robson_exec::order_journal::IntentRow;
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

/// The R1/R3 candidate's own SELL membership and target cancel group, each by
/// its full primary key. A null `$6` matches neither; aggregates stay as-is.
pub(super) const CANDIDATE_COLUMNS: &str = "m.scope_kind AS candidate_scope,
    m.scope_position AS candidate_position, m.potential AS candidate_potential,
    m.conflict AS candidate_conflict, cg.pending_count AS target_cancel_pending,
    cg.conflict_count AS target_cancel_conflicts, cg.terminal_mask AS target_cancel_terminal";
pub(super) const CANDIDATE_JOINS: &str = "LEFT JOIN sell_shadow_orders m
           ON (m.tenant_id,m.venue_id,m.account_scope,m.symbol,m.exchange_order_id)
            = ($1,$2,$3,$5,$6::text)
         LEFT JOIN cancel_shadow_groups cg
           ON (cg.tenant_id,cg.venue_id,cg.account_scope,cg.symbol,cg.position_id,cg.target_id)
            = ($1,$2,$3,$5,$4,$6::text)";

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

pub(super) fn relation_nodes<'a>(
    node: &'a serde_json::Value,
    name: &str,
    out: &mut Vec<&'a serde_json::Value>,
) {
    if node["Relation Name"] == name {
        out.push(node);
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            relation_nodes(child, name, out);
        }
    }
}

/// An initialized generation. Neither clonable nor constructible outside this
/// module; it is kept until its owner drops it and is never re-initialized.
#[derive(Debug)]
pub(super) struct ProjectionReady {
    generation: Uuid,
    database: String,
    consumed_cancels: Mutex<ConsumedCancels>,
}

#[derive(Debug, Default)]
struct ConsumedCancels {
    ids: HashSet<Uuid>,
    clients: HashSet<(String, String, String)>,
}

fn client_key(intent: &IntentRow) -> (String, String, String) {
    (
        intent.venue_id.clone(),
        intent.account_scope.clone(),
        intent.client_order_id.clone(),
    )
}

impl ProjectionReady {
    pub(super) fn generation(&self) -> Uuid {
        self.generation
    }

    pub(super) fn cancel_consumed(&self, intent: &IntentRow) -> bool {
        let registry = self.consumed_cancels.lock().expect("fixture cancel registry");
        registry.ids.contains(&intent.intent_id) || registry.clients.contains(&client_key(intent))
    }

    pub(super) fn consume_cancel(&self, intent: &IntentRow) -> bool {
        let client = client_key(intent);
        let mut registry = self.consumed_cancels.lock().expect("fixture cancel registry");
        if registry.ids.contains(&intent.intent_id) || registry.clients.contains(&client) {
            return false;
        }
        registry.ids.insert(intent.intent_id);
        registry.clients.insert(client);
        true
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

    pub(super) async fn explain(
        &self,
        pool: &PgPool,
        sql: &str,
        keys: Keys<'_>,
        candidate: &str,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(sqlx::FromRow)]
        struct Explain {
            #[sqlx(rename = "QUERY PLAN")]
            plan: serde_json::Value,
        }

        let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
        let physical: Explain =
            self.query_as(&explain, keys, Some(candidate)).fetch_one(pool).await?;
        Ok(physical.plan)
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
    let ready = ProjectionReady {
        generation,
        database,
        consumed_cancels: Mutex::default(),
    };
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
