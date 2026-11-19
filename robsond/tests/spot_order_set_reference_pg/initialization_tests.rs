//! Native fixture initialization at a fenced source/observer cutpoint.
use std::{collections::BTreeSet, time::Duration};

use anyhow::{anyhow, ensure, Context};
use proptest::{
    collection::vec as events,
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_exec::order_journal::JournalBatch;
use robson_exec::{
    venue::ProtectionKind,
    venue_ledger::{LegOrderKind, OrderOwner},
};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use rust_decimal::Decimal;
use tokio::time::timeout;

use self::ready::{
    initialize, initialize_at, probe, statement, Keys, ProjectionReady, ReplayOrder, Stage,
    ORDER_SET_COLUMNS,
};
use super::sell_nested_work_tests::seed_history;
use super::{
    audit, batch, cancel_conflict, cold_cancels, cold_sells, empty_budget, linked_sell, scope,
    seen, terminal, through_proxy_limit, unowned, zero_execution_order, AttemptRow, Fixture, Hot,
    IntentKind, JournalBackend, OrderPhase, OutcomeRow, PgJournalBackend, PgPool, Scope, Uuid,
    VenueOutcome,
};
use crate::fixture::retired;

#[path = "initialized_fixture.rs"]
mod ready;

#[derive(Debug, sqlx::FromRow)]
struct InitializedOrderSet {
    #[sqlx(flatten)]
    facts: Hot,
    unbound_sell: i64,
    unbound_sell_conflicts: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct CandidateOrderSet {
    #[sqlx(flatten)]
    set: InitializedOrderSet,
    base_commission_final: bool,
    candidate_scope: Option<String>,
    candidate_position: Option<Uuid>,
    candidate_potential: Option<bool>,
    candidate_conflict: Option<bool>,
    target_cancel_pending: Option<i64>,
    target_cancel_conflicts: Option<i64>,
    target_cancel_terminal: Option<bool>,
}

fn candidate_statement() -> String {
    statement(
        &format!("{ORDER_SET_COLUMNS}, {}", ready::CANDIDATE_COLUMNS),
        ready::CANDIDATE_JOINS,
    )
}

#[derive(sqlx::FromRow)]
struct Explain {
    #[sqlx(rename = "QUERY PLAN")]
    plan: serde_json::Value,
}

fn relation_nodes<'a>(node: &'a serde_json::Value, name: &str, out: &mut Vec<&'a serde_json::Value>) {
    if node["Relation Name"] == name {
        out.push(node);
    }
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            relation_nodes(child, name, out);
        }
    }
}

async fn measure_candidate_read(
    pool: &PgPool,
    actor: &PgPool,
    proxy: &DbProxy,
    ready: &ProjectionReady,
    scope: Scope,
    history: i64,
) -> anyhow::Result<()> {
    // Default PostgreSQL plans, measured after collecting real fixture statistics.
    sqlx::raw_sql(
        "ANALYZE venue_orders, order_intents, sell_shadow_orders, cancel_shadow_groups",
    )
    .execute(pool)
    .await?;
    let sql = candidate_statement();
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
    for (candidate, present) in [("1000", true), ("99000001", false)] {
        let before = proxy.stats().statements;
        let read: CandidateOrderSet = ready
            .query_as(&sql, keys(scope), Some(candidate))
            .fetch_one(actor)
            .await?;
        ensure!(proxy.stats().statements - before == 1);
        ensure!(proxy.query_trace().last() == Some(&sql));
        ensure!(read.set.facts.stop_id.as_deref() == present.then_some(candidate));
        ensure!(read.target_cancel_terminal == present.then_some(true));

        // EXPLAIN executes independently of the application wire measurement.
        let physical: Explain = ready
            .query_as(&explain, keys(scope), Some(candidate))
            .fetch_one(pool)
            .await?;
        let plan = &physical.plan[0]["Plan"];
        ensure!(plan["Actual Rows"].as_u64() == Some(1));
        for relation in ["venue_orders", "order_intents", "sell_shadow_orders", "cancel_shadow_groups"] {
            let mut nodes = Vec::new();
            relation_nodes(plan, relation, &mut nodes);
            ensure!(nodes.len() == 1, "expected one {relation} probe: {plan}");
            let node = nodes[0];
            let loops = node["Actual Loops"].as_u64().context("native probe loops")?;
            let rows = node["Actual Rows"].as_u64().context("native probe rows")?;
            ensure!(loops <= 1 && rows <= 1, "{relation}: {node}");
            if history >= 512 && loops != 0 {
                ensure!(node["Index Cond"].is_string(), "expected indexed {relation}: {node}");
                ensure!(node["Rows Removed by Filter"].as_u64().unwrap_or(0) == 0, "{node}");
                ensure!(node["Rows Removed by Index Recheck"].as_u64().unwrap_or(0) == 0, "{node}");
            }
        }
        for relation in ["cancel_shadow_members", "sell_shadow_intents", "submit_attempts"] {
            let mut nodes = Vec::new();
            relation_nodes(plan, relation, &mut nodes);
            ensure!(nodes.is_empty(), "unexpected historical {relation} read: {plan}");
        }
        eprintln!("SPOT_INIT_CANDIDATE_WORK history={history} present={present} wire=1 plan={}", physical.plan);
    }
    Ok(())
}

fn keys(s: Scope) -> Keys<'static> {
    Keys {
        tenant: s.tenant,
        venue: s.venue,
        account: s.account,
        position: s.position,
        symbol: s.symbol,
    }
}

impl ProjectionReady {
    async fn read(
        &self,
        pool: &PgPool,
        s: Scope,
        stop: Option<&str>,
    ) -> anyhow::Result<Option<InitializedOrderSet>> {
        let sql = statement(ORDER_SET_COLUMNS, "");
        Ok(self.query_as(&sql, keys(s), stop).fetch_optional(pool).await?)
    }

    async fn foreign_read(
        &self,
        pool: &PgPool,
        s: Scope,
    ) -> anyhow::Result<Option<InitializedOrderSet>> {
        let sql = statement(ORDER_SET_COLUMNS, "");
        Ok(self.foreign_query_as(&sql, keys(s), None).fetch_optional(pool).await?)
    }
}

async fn check(
    pool: &PgPool,
    ready: &ProjectionReady,
    s: Scope,
) -> anyhow::Result<InitializedOrderSet> {
    let hot = ready.read(pool, s, None).await?.context("exact initialized order set")?;
    // This is the existing separate ceiling for the exhaustive scale witness.
    let cold = timeout(Duration::from_secs(30), async {
        crate::reader::Snapshot::open(pool, s).await?.finish().await
    })
    .await
    .context("exhaustive initialization oracle deadline")?
    .map_err(|error| anyhow!("exhaustive reference: {error:?}"))?;
    ensure!(hot.facts.position_id == s.position && hot.facts.accounting_version == cold.version);
    ensure!(hot.facts.sells() == cold_sells(&cold));
    ensure!(hot.facts.cancels() == cold_cancels(&cold));
    let amounts: (Option<Decimal>, Option<Decimal>, Option<Decimal>, Option<Decimal>, Decimal, Decimal) =
        sqlx::query_as("SELECT gross_qty,acquisition_cost,base_commission,executable_qty,consumed_cum,target_trigger
                        FROM balance_legs WHERE
                        (tenant_id,venue_id,account_scope,symbol,position_id)=($1,$2,$3,$4,$5)")
            .bind(s.tenant).bind(s.venue).bind(s.account).bind(s.symbol).bind(s.position)
            .fetch_one(pool).await?;
    ensure!(
        amounts
            == (
                hot.facts.gross_qty,
                hot.facts.acquisition_cost,
                hot.facts.base_commission,
                hot.facts.executable_qty,
                hot.facts.consumed_cum,
                hot.facts.target_trigger,
            )
    );
    verify_member_identities(pool, s, &cold).await?;
    Ok(hot)
}

async fn verify_member_identities(
    pool: &PgPool,
    s: Scope,
    cold: &super::Report,
) -> anyhow::Result<()> {
    let actual_sells: Vec<(String, String, String, bool)> = sqlx::query_as(
        "SELECT origin, id, scope_kind, conflict FROM (
            SELECT 'journal' AS origin,intent_id::text AS id, scope_kind,conflict,potential,
                   scope_tenant,scope_venue,scope_account,scope_symbol,scope_position
            FROM sell_shadow_intents
            UNION ALL
            SELECT 'native',exchange_order_id,scope_kind,conflict,potential,
                   scope_tenant,scope_venue,scope_account,scope_symbol,scope_position
            FROM sell_shadow_orders
        ) members WHERE potential AND (scope_tenant,scope_venue,scope_account,scope_symbol)=($1,$2,$3,$4)
          AND (scope_position=$5 OR scope_kind='symbol')",
    )
    .bind(s.tenant).bind(s.venue).bind(s.account).bind(s.symbol).bind(s.position)
    .fetch_all(pool).await?;
    let mut expected = BTreeSet::new();
    let mut exact_native = BTreeSet::new();
    for row in cold
        .rows
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "exit" | "protection"))
    {
        let exact = row
            .named_id
            .as_deref()
            .filter(|_| row.named_identity_matches && row.named_side.as_deref() == Some("SELL"));
        exact_native.extend(exact);
        let contradicted = row.not_sent || row.venue_outcome.as_deref() == Some("rejected");
        let closed = terminal(row.named_phase.as_deref(), row.named_status.as_deref());
        if row.no_send
            || row.has_durable_rejection()
            || (exact.is_some() && !contradicted && closed)
        {
            continue;
        }
        let (origin, id) = match exact {
            Some(id) => ("native", id.to_owned()),
            None => ("journal", row.intent_id.to_string()),
        };
        expected.insert((
            origin.to_owned(),
            id,
            "leg".to_owned(),
            contradicted || closed || (row.named_id.is_some() && exact.is_none()),
        ));
    }
    for native in &cold.venue_complement {
        if exact_native.contains(native.exchange_order_id.as_str()) {
            continue;
        }
        if native.side != "SELL"
            && !cold.rows.iter().any(|journal| {
                Some(journal.intent_id) == native.intent_id
                    && matches!(journal.kind.as_str(), "exit" | "protection")
            })
        {
            continue;
        }
        let bound = native.position_id.is_some();
        expected.insert((
            "native".to_owned(),
            native.exchange_order_id.clone(),
            if bound { "leg" } else { "symbol" }.to_owned(),
            bound || native.phase == "terminal",
        ));
    }
    ensure!(
        actual_sells.len() == expected.len()
            && actual_sells.into_iter().collect::<BTreeSet<_>>() == expected,
        "exact source keys of projected SELL liabilities"
    );

    let actual_cancels: Vec<(Uuid, Option<String>, bool, bool)> = sqlx::query_as(
        "SELECT intent_id,target_id,pending,conflict FROM cancel_shadow_members
         WHERE (tenant_id,venue_id,account_scope)=($1,$2,$3)
           AND ((symbol,position_id)=($4,$5) OR symbol IS NULL) ORDER BY intent_id",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(s.position)
    .fetch_all(pool)
    .await?;
    let mut expected_cancels: Vec<_> = cold
        .rows
        .iter()
        .filter(|row| row.kind == "cancel")
        .map(|row| {
            (
                row.intent_id,
                row.original_target.clone(),
                !row.no_send,
                cancel_conflict(
                    row.named_id.is_some(),
                    row.not_sent,
                    row.venue_outcome.is_some(),
                    row.venue_evidence.is_some(),
                ),
            )
        })
        .chain(cold.unbound_cancels.iter().map(|row| {
            let target = row
                .cancels_order_id
                .as_deref()
                .filter(|id| id.parse::<u64>().is_ok_and(|value| value.to_string() == *id))
                .map(str::to_owned);
            (
                row.intent_id,
                target,
                !row.no_send,
                cancel_conflict(
                    row.named_id.is_some(),
                    row.not_sent,
                    row.venue_outcome.is_some(),
                    row.venue_evidence.is_some(),
                ),
            )
        }))
        .collect();
    expected_cancels.sort_unstable_by_key(|row| row.0);
    ensure!(
        actual_cancels == expected_cancels,
        "exact identity and evidence of each cancel member"
    );
    Ok(())
}

async fn journal_commit(journal: &PgJournalBackend, batch: &JournalBatch) -> anyhow::Result<()> {
    journal
        .commit(batch)
        .await
        .map_err(|failure| anyhow!("fixture journal commit: {failure:?}"))?;
    Ok(())
}

fn initializer_backend(proxy: &DbProxy) -> anyhow::Result<i32> {
    let identities = proxy.diagnostic_identities().context("native initializer trace")?;
    let [session] = identities.as_slice() else {
        return Err(anyhow!("expected precisely one raw initializer backend"));
    };
    session.pid.context("native initializer PID")
}

async fn unbound_sell_identities(pool: &PgPool, s: Scope) -> anyhow::Result<Vec<(Uuid, bool)>> {
    Ok(sqlx::query_as(
        "SELECT intent_id, conflict FROM sell_shadow_intents
         WHERE (scope_tenant,scope_venue,scope_account)=($1,$2,$3)
           AND scope_kind='unbound' AND scope_symbol IS NULL AND potential ORDER BY intent_id",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .fetch_all(pool)
    .await?)
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
    journal_commit(&journal, &terminal).await?;
    journal_commit(&journal, &batch(s, IntentKind::Cancel, Some("100"))).await?;
    let mut final_order = zero_execution_order("100", "CANCELED", OrderPhase::Terminal);
    final_order.client_order_id = terminal_link.client_order_id.clone();
    f.ledger.record_order(&final_order, &terminal_link, seen(2)).await?;
    journal_commit(&journal, &batch(s, IntentKind::Cancel, Some("100"))).await?;

    let pending = batch(s, IntentKind::Exit, None);
    let pending_link = linked_sell(&f, &pending.intent);
    journal_commit(&journal, &pending).await?;
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
    journal_commit(&journal, &rejected).await?;
    f.ledger
        .record_order(&zero_execution_order("102", "NEW", OrderPhase::Live), &unowned(), seen(4))
        .await?;

    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    let existing = check(&pool, &ready, s).await?;
    ensure!((existing.facts.leg_sell, existing.facts.symbol_sell) == (1, 1));
    ensure!(existing.facts.grouped_cancel == 0);
    ensure!(existing.unbound_sell == 0 && existing.unbound_sell_conflicts == 0);
    ensure!(
        ready.foreign_read(&pool, s).await?.is_none(),
        "wrong generation must withhold the initialized order set"
    );
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

    let protection = batch(s, IntentKind::Protection, None);
    journal_commit(&journal, &protection).await?;
    let mut stop_link = linked_sell(&f, &protection.intent);
    stop_link.owner = OrderOwner::Leg {
        position_id: s.position,
        intent_id: protection.intent.intent_id,
        kind: LegOrderKind::Stop,
    };
    let mut stop = zero_execution_order("103", "NEW", OrderPhase::Live);
    stop.client_order_id = stop_link.client_order_id.clone();
    stop.order_type = "STOP_LOSS".to_owned();
    stop.protection_kind = Some(ProtectionKind::MarketOnTrigger);
    stop.stop_price = Some(Decimal::from(90));
    ensure!(f.ledger.record_order(&stop, &stop_link, seen(6)).await?.changed);
    check(&pool, &ready, s).await?;
    let target = ready.read(&pool, s, Some("103")).await?.context("exact candidate stop")?;
    ensure!(
        target.facts.stop_id.as_deref() == Some("103")
            && target.facts.stop_phase.as_deref() == Some("live")
            && target.facts.stop_status.as_deref() == Some("NEW")
            && target.facts.exact_stop_identity
    );
    let decoy = ready.read(&pool, s, Some("101")).await?.context("non-stop candidate")?;
    ensure!(decoy.facts.stop_id.as_deref() == Some("101") && !decoy.facts.exact_stop_identity);

    let sql = candidate_statement();
    let (proxy, measured) = through_proxy_limit(&pool, 1).await;
    let before = proxy.stats().statements;
    let own: CandidateOrderSet = ready
        .query_as(&sql, keys(s), Some("103"))
        .fetch_optional(&measured)
        .await?
        .context("exact candidate membership")?;
    ensure!(proxy.stats().statements - before == 1, "one keyed candidate read");
    measured.close().await;
    let recorded_fee_final: bool =
        sqlx::query_scalar("SELECT base_commission_final FROM balance_legs WHERE position_id=$1")
            .bind(s.position)
            .fetch_one(&pool)
            .await?;
    ensure!(
        own.set.facts.exact_stop_identity
            && own.base_commission_final == recorded_fee_final
            && own.candidate_scope.as_deref() == Some("leg")
            && own.candidate_position == Some(s.position)
            && own.candidate_potential == Some(true)
            && own.candidate_conflict == Some(false)
            && own.target_cancel_pending.is_none()
            && own.target_cancel_conflicts.is_none()
            && own.target_cancel_terminal.is_none(),
        "own live stop is exactly one nonconflicting leg member with no target cancel"
    );
    journal_commit(&journal, &batch(s, IntentKind::Cancel, Some("103"))).await?;
    let cancelled: CandidateOrderSet = ready
        .query_as(&sql, keys(s), Some("103"))
        .fetch_optional(&pool)
        .await?
        .context("candidate under a target cancel")?;
    ensure!(
        cancelled.target_cancel_pending.is_some_and(|pending| pending > 0)
            && cancelled.target_cancel_conflicts == Some(0)
            && cancelled.target_cancel_terminal == Some(false),
        "an outstanding target cancel is visible on the candidate's own key"
    );
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
    journal_commit(&journal, &pending).await?;

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
    let (proxy, actor) = through_proxy_limit(&pool, 1).await;
    proxy.enable_diagnostics();
    ensure!(initialize(&actor, ReplayOrder::TargetsFirst, Some(&injected)).await.is_err());
    let pid = initializer_backend(&proxy)?;
    retired(&pool, pid).await;
    actor.close().await;
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

async fn committed_event(
    fixture: &Fixture,
    journal: &PgJournalBackend,
    action: u8,
    index: usize,
) -> anyhow::Result<()> {
    let s = scope(fixture);
    match action {
        0 => journal_commit(journal, &batch(s, IntentKind::Exit, None)).await?,
        1 => journal_commit(journal, &batch(s, IntentKind::Cancel, Some("1000"))).await?,
        2 => {
            let mut event = batch(s, IntentKind::Exit, None);
            event.attempts.push(AttemptRow {
                attempt_id: Uuid::now_v7(),
                created_at: seen(601).at,
                rejected_at: (index % 2 == 0).then_some(seen(602).at),
            });
            event.outcome = Some(OutcomeRow {
                venue_outcome: Some(VenueOutcome::Rejected),
                ..OutcomeRow::default()
            });
            journal_commit(journal, &event).await?;
        },
        3 => {
            let id = (10_000 + index).to_string();
            fixture
                .ledger
                .record_order(
                    &zero_execution_order(&id, "NEW", OrderPhase::Live),
                    &unowned(),
                    seen(603),
                )
                .await?;
        },
        _ => unreachable!(),
    }
    Ok(())
}

async fn generated_history(
    pool: &PgPool,
    history: i64,
    cancels: usize,
    seed: u64,
    order: ReplayOrder,
) -> anyhow::Result<()> {
    empty_budget::initialize(pool, seen(0).at).await?;
    let fixtures = [Fixture::new(pool).await, Fixture::new(pool).await];
    ensure!(fixtures[0].tenant != fixtures[1].tenant);
    let [first, second] = fixtures.each_ref().map(scope);
    let journals = [
        PgJournalBackend::new(pool, first.tenant),
        PgJournalBackend::new(pool, second.tenant),
    ];
    seed_history(&fixtures[0], &journals[0], first, history).await;
    for target in 1001..1000 + history {
        journal_commit(&journals[0], &batch(first, IntentKind::Cancel, Some(&target.to_string())))
            .await?;
    }
    for _ in 0..cancels {
        journal_commit(&journals[0], &batch(first, IntentKind::Cancel, Some("1000"))).await?;
    }
    let mut pre_leg = batch(first, IntentKind::Exit, None);
    pre_leg.intent.position_id = Some(Uuid::now_v7());
    journal_commit(&journals[0], &pre_leg).await?;
    let config = Config {
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);
    let schedule = (events(0_u8..8, 8), 2_usize..7)
        .new_tree(&mut runner)
        .map_err(|error| anyhow!("event generation: {error:?}"))?
        .current();
    let (events, cutpoint) = schedule;
    for (index, &event) in events[..cutpoint].iter().enumerate() {
        let owner = usize::from(event / 4);
        committed_event(&fixtures[owner], &journals[owner], event % 4, index).await?;
    }

    // Witness the stale-view failure of the earlier proposed cutpoint.
    // Owner B commits while A's earlier REPEATABLE READ view is open.
    let mut obsolete = pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
    let _: i64 = sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2",
    )
    .bind(first.tenant)
    .bind(first.venue)
    .fetch_one(&mut *obsolete)
    .await?;
    let (proxy, actor) = through_proxy_limit(pool, 1).await;
    proxy.enable_query_trace(32);
    let baseline = proxy.stats().statements;
    let (gate, reached, release) = probe(Stage::BeforeFence);
    let late = batch(second, IntentKind::Exit, None);
    let (ready, ()) = tokio::try_join!(
        initialize_at(&actor, order, None, Duration::from_secs(5), Some(gate)),
        async {
            reached.await?;
            // Both transactions started. B commits while the initializer
            // has yet to acquire any of its source table locks.
            journal_commit(&journals[1], &late).await?;
            let stale_visible: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT FROM order_intents WHERE intent_id=$1)")
                    .bind(late.intent.intent_id)
                    .fetch_one(&mut *obsolete)
                    .await?;
            ensure!(!stale_visible, "the obsolete early transaction misses the second owner");
            obsolete.rollback().await?;
            sqlx::raw_sql("ANALYZE order_intents; ANALYZE venue_orders; ANALYZE submit_attempts;")
                .execute(pool)
                .await?;
            release.send(()).map_err(|_| anyhow!("initializer fence wait was retired"))
        },
    )?;
    let init_frames = proxy.stats().statements - baseline;
    ensure!(init_frames == 10 && proxy.query_trace_dropped() == 0);
    let a = check(pool, &ready, first).await?;
    let b = check(pool, &ready, second).await?;
    ensure!(b.facts.leg_sell >= 1 && a.facts.grouped_cancel == 0);
    ensure!(
        (a.unbound_sell, a.unbound_sell_conflicts, b.unbound_sell) == (1, 0, 0),
        "pre-leg SELL remains a blocker only for its actual accounting owner"
    );
    let pre_leg_member = [(pre_leg.intent.intent_id, false)];
    ensure!(unbound_sell_identities(pool, first).await? == pre_leg_member);
    ensure!(unbound_sell_identities(pool, second).await?.is_empty());
    let coverage: serde_json::Value =
        sqlx::query_scalar("SELECT coverage FROM spot_shadow_initialization WHERE generation=$1")
            .bind(ready.generation())
            .fetch_one(pool)
            .await?;
    let cancel_count = coverage["cancel_members"].as_u64().context("native cancel coverage")?;
    ensure!(cancel_count >= cancels as u64);
    ensure!(coverage["sell_natives"].as_i64().context("native SELL coverage")? >= history);
    let late_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT FROM sell_shadow_intents
            WHERE intent_id=$1 AND scope_tenant=$2 AND potential)",
    )
    .bind(late.intent.intent_id)
    .bind(second.tenant)
    .fetch_one(pool)
    .await?;
    ensure!(late_member, "actual pre-fence source commit is missing from projection");
    let read_start = proxy.stats().statements;
    ensure!(ready.read(&actor, second, None).await?.is_some());
    ensure!(proxy.stats().statements - read_start == 1);
    let groups: i64 = sqlx::query_scalar("SELECT count(*) FROM cancel_shadow_groups WHERE tenant_id=$1")
        .bind(first.tenant)
        .fetch_one(pool)
        .await?;
    ensure!(groups >= history, "fixture must seed distinct historical cancel groups");
    measure_candidate_read(pool, &actor, &proxy, &ready, first, history).await?;
    actor.close().await;
    ensure!(
        sqlx::query_scalar::<_, bool>(
            "SELECT
        NOT EXISTS(SELECT FROM pg_locks WHERE relation='order_intents'::regclass
                   AND mode='AccessExclusiveLock' AND granted)"
        )
        .fetch_one(pool)
        .await?
    );
    for (index, &event) in events.iter().enumerate().skip(cutpoint) {
        let owner = usize::from(event / 4);
        committed_event(&fixtures[owner], &journals[owner], event % 4, index).await?;
        check(pool, &ready, first).await?;
        check(pool, &ready, second).await?;
        ensure!(unbound_sell_identities(pool, first).await? == pre_leg_member);
        ensure!(unbound_sell_identities(pool, second).await?.is_empty());
    }
    eprintln!("SPOT_FENCED_INIT histories={history} base_cancels={cancels} owners=2 seed={seed} cutpoint={cutpoint} init_wire_frames={init_frames} hot_sql=1 old_snapshot_missed_late=true all_events_match=true coverage={coverage}");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_generated_small_history(pool: PgPool) -> anyhow::Result<()> {
    generated_history(&pool, 2, 2, 2026111901, ReplayOrder::TargetsFirst).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_generated_large_history(pool: PgPool) -> anyhow::Result<()> {
    generated_history(&pool, 512, 48, 2026111902, ReplayOrder::MembersFirst).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_never_adopts_a_different_accounting_owner(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let original = Fixture::new(&pool).await;
    let foreign = Fixture::new(&pool).await;
    ensure!(original.tenant != foreign.tenant);
    let original_scope = scope(&original);
    let exit = batch(original_scope, IntentKind::Exit, None);
    journal_commit(&PgJournalBackend::new(&pool, original.tenant), &exit).await?;
    let link = linked_sell(&foreign, &exit.intent);
    let mut claimed = zero_execution_order("100", "NEW", OrderPhase::Live);
    claimed.client_order_id = link.client_order_id.clone();
    match foreign.ledger.record_order(&claimed, &link, seen(2)).await {
        Err(error) => {
            let leaked: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT FROM venue_orders WHERE intent_id=$1)")
                    .bind(exit.intent.intent_id)
                    .fetch_one(&pool)
                    .await?;
            ensure!(!leaked);
            let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
            ensure!(check(&pool, &ready, original_scope).await?.facts.leg_sell == 1);
            ensure!(check(&pool, &ready, scope(&foreign)).await?.facts.leg_sell == 0);
            eprintln!("SPOT_FENCED_INIT cross_owner=native_refused disposition={error:?}");
        },
        Ok(record) => {
            ensure!(record.changed);
            let failure = initialize(&pool, ReplayOrder::MembersFirst, None).await.unwrap_err();
            ensure!(failure
                .to_string()
                .contains("replay cannot reconcile across accounting owners"));
            let generation: Option<String> =
                sqlx::query_scalar("SELECT to_regclass('spot_shadow_initialization')::text")
                    .fetch_one(&pool)
                    .await?;
            ensure!(generation.is_none());
            eprintln!("SPOT_FENCED_INIT cross_owner=historical_conflict_refused");
        },
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_retires_cancelled_and_expired_native_owners(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let f = Fixture::new(&pool).await;
    for expire in [false, true] {
        let (proxy, actor) = through_proxy_limit(&pool, 1).await;
        proxy.enable_diagnostics();
        let (gate, reached, _release) = probe(Stage::AfterFence);
        let mut testing = Box::pin(initialize_at(
            &actor,
            ReplayOrder::MembersFirst,
            None,
            Duration::from_secs(1),
            Some(gate),
        ));
        tokio::select! {
            status = reached => status?,
            _ = &mut testing => return Err(anyhow!("native initialization never held its fence")),
        }
        let pid = initializer_backend(&proxy)?;
        let locked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT FROM pg_locks WHERE pid=$1 AND relation='order_intents'::regclass
                          AND granted AND mode='AccessExclusiveLock')",
        )
        .bind(pid)
        .fetch_one(&pool)
        .await?;
        ensure!(locked);
        if expire {
            let result = testing.as_mut().await.unwrap_err();
            ensure!(result.to_string() == "Spot initialization deadline");
        }
        drop(testing);
        retired(&pool, pid).await;
        actor.close().await;
    }
    let recovered = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    ensure!(check(&pool, &recovered, scope(&f)).await?.facts.leg_sell == 0);
    eprintln!(
        "SPOT_FENCED_INIT cancellation_retired=true expiry_retired=true exact_fence_observed=true"
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fenced_initialization_withholds_generation_when_confirmation_is_lost(
    pool: PgPool,
) -> anyhow::Result<()> {
    empty_budget::initialize(&pool, seen(0).at).await?;
    let _fixture = Fixture::new(&pool).await;
    let (proxy, actor) = through_proxy_limit(&pool, 1).await;
    proxy.enable_diagnostics();
    proxy.enable_query_trace(32);
    proxy.set_mode(ProxyMode::DropCommitAck);
    let (gate, reached, release) = probe(Stage::BeforeConfirmation);
    let mut testing = Box::pin(initialize_at(
        &actor,
        ReplayOrder::MembersFirst,
        None,
        Duration::from_secs(2),
        Some(gate),
    ));
    tokio::select! {
        status = reached => status?,
        _ = &mut testing => return Err(anyhow!("the COMMIT path never needed confirmation")),
    }
    ensure!(proxy.stats().commit_acks_dropped == 1);
    let pid = initializer_backend(&proxy)?;
    retired(&pool, pid).await;
    proxy.set_mode(ProxyMode::Blackhole);
    release.send(()).map_err(|_| anyhow!("closed confirmation staging"))?;
    let refusal = testing.await.unwrap_err();
    ensure!(refusal.to_string() == "Spot initialization commit unresolved");
    ensure!(proxy.stats().commits == 1, "no replay on uncertain acknowledgement");
    let marker_count: i64 = sqlx::query_scalar("SELECT count(*) FROM spot_shadow_initialization")
        .fetch_one(&pool)
        .await?;
    ensure!(marker_count == 1, "actual commit and unusable caller proof are distinct");
    actor.close().await;
    eprintln!("SPOT_FENCED_INIT lost_ack_and_confirmation=closed original_backend_retired=true replayed=false");
    Ok(())
}
