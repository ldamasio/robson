//! Native plan instrumentation for actual projection writers. The normal wire
//! and latency controls run separately from these auto_explain samples.

use super::{
    batch, empty_budget, hot_query, install, linked_sell, scope, seen, through_proxy_limit,
    unowned, zero_execution_order, AttemptRow, Fixture, IntentKind, JournalBackend, OrderPhase,
    OutcomeRow, PgJournalBackend, PgVenueLedger, Scope, Uuid, VenueOutcome,
};
use robson_testkit::db_proxy::DbProxy;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;

const PROJECTION_HEAD: &str = "3aab1ef77d6315a35c6489f6d5293c3e23fb3811";
const TARGET: &str = "800001";
const TRACE_CAPACITY: usize = 64;

struct Sample {
    name: String,
    actor: PgPool,
    proxy: DbProxy,
    baseline: u64,
}

async fn marker(pool: &PgPool, boundary: &str, application: &str) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('robson_test.nested_marker', $1, true),
                set_config('robson_test.nested_projection_head', $2, true)",
    )
    .bind(format!("{boundary} phase={application}"))
    .bind(PROJECTION_HEAD)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::raw_sql(
        "DO $marker$
         BEGIN
             RAISE LOG 'ROBSON_ORDER_SET_NESTED % db=% projection_head=%',
                 current_setting('robson_test.nested_marker'), current_database(),
                 current_setting('robson_test.nested_projection_head');
         END
         $marker$",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

impl Sample {
    async fn begin(control: &PgPool, history: i64, phase: &str) -> Self {
        let name = format!("robson-nested-{history}-{phase}");
        let (proxy, passthrough) = through_proxy_limit(control, 1).await;
        // The actual journal creates its own pool with these same connection
        // options. A hook run only on the daemon pool would miss its writes.
        let options = (*passthrough.connect_options()).clone().application_name(&name).options([
            ("session_preload_libraries", "auto_explain"),
            ("auto_explain.log_min_duration", "0"),
            ("auto_explain.log_analyze", "on"),
            ("auto_explain.log_timing", "on"),
            ("auto_explain.log_buffers", "on"),
            ("auto_explain.log_triggers", "on"),
            ("auto_explain.log_nested_statements", "on"),
            ("auto_explain.log_format", "json"),
            ("robson_test.sell_shadow_trace", "on"),
            ("robson_test.sell_shadow_label", &name),
        ]);
        passthrough.close().await;
        let actor = PgPoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        marker(control, "BEGIN", &name).await;
        let baseline = proxy.stats().statements;
        proxy.enable_exact_query_trace(TRACE_CAPACITY);
        Self { name, actor, proxy, baseline }
    }

    async fn finish(self, control: &PgPool, dml_prefix: &str, branch: &str) {
        self.actor.close().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                 WHERE datname = current_database() AND application_name = $1)",
            )
            .bind(&self.name)
            .fetch_one(control)
            .await
            .unwrap()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("measurement actor backends retired");
        let trace = self.proxy.query_trace();
        let count = self.proxy.stats().statements - self.baseline;
        assert_eq!(count, trace.len() as u64);
        assert_eq!(self.proxy.query_trace_dropped(), 0);
        assert!(
            trace.iter().any(|sql| sql.trim_start().starts_with(dml_prefix)),
            "{} actual source DML: {trace:?}",
            self.name
        );
        let database = control.connect_options();
        println!(
            "order-set nested phase={} db={} wire_count={count} wire={}",
            self.name,
            database.get_database().unwrap(),
            serde_json::to_string(&trace).unwrap()
        );
        marker(control, "END", &self.name).await;
        let rows: Vec<(String, i32, i32, i32)> = sqlx::query_as(
            "SELECT branch, probes, member_rows, aggregate_rows FROM sell_shadow_work
             WHERE label = $1 ORDER BY work_id",
        )
        .bind(&self.name)
        .fetch_all(control)
        .await
        .unwrap();
        assert!(rows.iter().any(|row| row.0 == branch), "{} {rows:?}", self.name);
        assert!(rows.iter().all(|row| row.2 <= 2 && row.3 <= 4), "{} {rows:?}", self.name);
        println!("order-set nested phase={} sell_trigger_work={rows:?}", self.name);
    }
}

async fn seed_history(f: &Fixture, journal: &PgJournalBackend, s: Scope, history: i64) {
    for offset in 0..history {
        let mut exit = batch(s, IntentKind::Exit, None);
        exit.attempts.push(AttemptRow {
            attempt_id: Uuid::now_v7(),
            created_at: seen(2 + offset).at,
            rejected_at: None,
        });
        journal.commit(&exit).await.unwrap();
        let link = linked_sell(f, &exit.intent);
        let id = (1000 + offset).to_string();
        let mut terminal = zero_execution_order(&id, "CANCELED", OrderPhase::Terminal);
        terminal.client_order_id = link.client_order_id.clone();
        assert!(f.ledger.record_order(&terminal, &link, seen(3 + offset)).await.unwrap().changed);
    }
}

async fn verify_size(pool: &PgPool, history: i64) {
    // Stats are collected outside the samples, with default plan selection.
    sqlx::raw_sql(
        "ANALYZE order_intents, venue_orders, submit_attempts, sell_shadow_intents,
         sell_shadow_orders, sell_shadow_legs, sell_shadow_symbols, cancel_shadow_legs,
         cancel_shadow_groups, cancel_shadow_members",
    )
    .execute(pool)
    .await
    .unwrap();
    let (closed, journals, attempts): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM venue_orders WHERE phase = 'terminal'),
                (SELECT count(*) FROM order_intents), (SELECT count(*) FROM submit_attempts)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!((closed, journals, attempts), (history, history + 1, history));
    let database = pool.connect_options();
    println!(
        "order-set nested baseline db={} closed_pairs={closed} journals={journals} attempts={attempts}",
        database.get_database().unwrap()
    );
}

async fn measure(pool: PgPool, history: i64, cancel_members: i64) {
    empty_budget::initialize(&pool, seen(0).at).await.unwrap();
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let seed = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;
    seed_history(&f, &seed, s, history).await;
    verify_size(&pool, history).await;

    let mut rejected = batch(s, IntentKind::Exit, None);
    let sample = Sample::begin(&pool, history, "journal-insert").await;
    PgJournalBackend::new(&sample.actor, s.tenant).commit(&rejected).await.unwrap();
    sample.finish(&pool, "INSERT INTO order_intents", "journal_unresolved").await;
    assert_eq!(hot_query(s, None).fetch_one(&pool).await.unwrap().leg_sell, 1);

    rejected.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    rejected.attempts.push(AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: seen(600).at,
        rejected_at: None,
    });
    seed.commit(&rejected).await.unwrap();
    rejected.attempts[0].rejected_at = Some(seen(601).at);
    let sample = Sample::begin(&pool, history, "attempt-reject").await;
    PgJournalBackend::new(&sample.actor, s.tenant).commit(&rejected).await.unwrap();
    sample.finish(&pool, "INSERT INTO submit_attempts", "journal_rejected").await;
    assert_eq!(hot_query(s, None).fetch_one(&pool).await.unwrap().leg_sell, 0);

    let exit = batch(s, IntentKind::Exit, None);
    let link = linked_sell(&f, &exit.intent);
    let mut discovered = unowned();
    discovered.client_order_id = link.client_order_id.clone();
    let mut order = zero_execution_order(TARGET, "NEW", OrderPhase::Live);
    order.client_order_id = link.client_order_id.clone();
    assert!(f.ledger.record_order(&order, &discovered, seen(602)).await.unwrap().changed);
    seed.commit(&exit).await.unwrap();
    let sample = Sample::begin(&pool, history, "venue-adopt").await;
    assert!(
        PgVenueLedger::new(&sample.actor, s.tenant)
            .record_order(&order, &link, seen(603))
            .await
            .unwrap()
            .changed
    );
    sample.finish(&pool, "UPDATE venue_orders", "native_open").await;
    let adopted = hot_query(s, None).fetch_one(&pool).await.unwrap();
    assert_eq!((adopted.leg_sell, adopted.symbol_sell), (1, 0));

    for _ in 1..cancel_members {
        seed.commit(&batch(s, IntentKind::Cancel, Some(TARGET))).await.unwrap();
    }
    let cancel = batch(s, IntentKind::Cancel, Some(TARGET));
    let sample = Sample::begin(&pool, history, "journal-cancel").await;
    PgJournalBackend::new(&sample.actor, s.tenant).commit(&cancel).await.unwrap();
    sample.finish(&pool, "INSERT INTO order_intents", "journal_not_sell").await;
    assert_eq!(
        hot_query(s, None).fetch_one(&pool).await.unwrap().grouped_cancel,
        cancel_members
    );

    order.status = "CANCELED".to_owned();
    order.phase = OrderPhase::Terminal;
    let sample = Sample::begin(&pool, history, "venue-terminal").await;
    assert!(
        PgVenueLedger::new(&sample.actor, s.tenant)
            .record_order(&order, &link, seen(604))
            .await
            .unwrap()
            .changed
    );
    sample.finish(&pool, "UPDATE venue_orders", "native_discharged").await;
    let terminal = hot_query(s, None).fetch_one(&pool).await.unwrap();
    assert_eq!((terminal.leg_sell, terminal.grouped_cancel), (0, 0));
    let changed: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT group_rows, leg_rows, member_rows
         FROM cancel_shadow_terminal_work WHERE target_id=$1",
    )
    .bind(TARGET)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(changed, [(1, 1, 0)]);
    println!(
        "order-set nested cardinality closed_pairs={history} cancel_members={cancel_members} terminal_work={changed:?}"
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated superuser PostgreSQL with native auto_explain"]
async fn native_shadow_write_work_small(pool: PgPool) {
    measure(pool, 2, 2).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated superuser PostgreSQL with native auto_explain"]
async fn native_shadow_write_work_large(pool: PgPool) {
    measure(pool, 512, 48).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn member_arbiter_preserves_exact_noop_and_identity_refusal(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    install(&pool).await;
    let journal = PgJournalBackend::new(&pool, s.tenant);
    let cancel = batch(s, IntentKind::Cancel, Some(TARGET));
    journal.commit(&cancel).await.unwrap();
    let other_target = "800002";
    for parameter in [TARGET, other_target] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2 FOR UPDATE")
            .bind(s.tenant)
            .bind(s.venue)
            .execute(&mut *tx)
            .await
            .unwrap();
        let observe = sqlx::query(
            "SELECT cancel_shadow_sync_member(jsonb_populate_record(j,
               jsonb_build_object('cancels_order_id', $2::text)))
             FROM order_intents j WHERE j.intent_id=$1",
        )
        .bind(cancel.intent.intent_id)
        .bind(parameter)
        .execute(&mut *tx)
        .await;

        if parameter == TARGET {
            observe.unwrap();
            let mutations: i64 = sqlx::query_scalar("SELECT cancel_shadow_member_writes()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(mutations, 0, "equal replay must not physically update a member");
        } else {
            let error = observe.unwrap_err();
            let database = error.as_database_error().unwrap();
            assert_eq!(database.code().as_deref(), Some("P0001"));
            assert_eq!(database.message(), "test shadow requires a stable cancel identity");
        }
        tx.rollback().await.unwrap();
    }
    let target: String =
        sqlx::query_scalar("SELECT target_id FROM cancel_shadow_members WHERE intent_id=$1")
            .bind(cancel.intent.intent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(target, TARGET);
    assert_eq!(hot_query(s, None).fetch_one(&pool).await.unwrap().grouped_cancel, 1);
}
