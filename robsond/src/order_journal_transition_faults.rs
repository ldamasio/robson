use std::{cell::RefCell, future::Future, time::Duration};

use anyhow::Result;
use robson_exec::order_journal::{JournalBackend, JournalBatch, OutcomeRow, VenueOutcome};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use serde_json::{json, Value};
use sqlx::{
    postgres::{PgPoolOptions, PgSslMode},
    PgPool,
};
use tokio::{sync::oneshot, time::timeout};

use super::{
    positive::{migrate_fixture, PositiveLeg, ORIGINAL, SYMBOL},
    *,
};

struct Pause {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

tokio::task_local! {
    static PAUSE: RefCell<Option<Pause>>;
}

pub(super) async fn before_commit() {
    let Ok(Some(pause)) = PAUSE.try_with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let _ = pause.reached.send(());
    let _ = pause.release.await;
}

fn pause() -> (RefCell<Option<Pause>>, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached, wait) = oneshot::channel();
    let (resume, release) = oneshot::channel();
    (RefCell::new(Some(Pause { reached, release })), wait, resume)
}

pub(super) async fn blackhole_commit<T>(
    apply: impl Future<Output = T>,
    proxy: &DbProxy,
) -> Result<T> {
    let (slot, reached, release) = pause();
    let apply = PAUSE.scope(slot, apply);
    let intercept = async {
        timeout(Duration::from_millis(400), reached).await??;
        proxy.set_mode(ProxyMode::Blackhole);
        release.send(()).map_err(|_| anyhow::anyhow!("commit paused writer retired"))?;
        Ok::<_, anyhow::Error>(())
    };
    let (outcome, interception) = tokio::join!(apply, intercept);
    interception?;
    proxy.set_mode(ProxyMode::Sever);
    Ok(outcome)
}

pub(super) struct Wire {
    pub(super) proxy: DbProxy,
    pub(super) store: PgJournalBackend,
    pid: i32,
    birth: String,
    baseline: u64,
}

impl Wire {
    pub(super) async fn new(pool: &PgPool, tenant: Uuid) -> Result<Self> {
        Self::with_idle_health_check(pool, tenant, true).await
    }

    pub(super) async fn with_idle_health_check(
        pool: &PgPool,
        tenant: Uuid,
        check_idle: bool,
    ) -> Result<Self> {
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no fixture PostgreSQL address"))?;
        anyhow::ensure!(upstream.ip().is_loopback(), "fixture must be loopback");
        let proxy = DbProxy::start(upstream).await?;
        let test_pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(super::super::JOURNAL_ACQUIRE_BOUND)
            .test_before_acquire(check_idle)
            .connect_with(
                options
                    .as_ref()
                    .clone()
                    .host("127.0.0.1")
                    .port(proxy.addr().port())
                    .ssl_mode(PgSslMode::Disable)
                    .application_name("atomic-cancel-local-wire"),
            )
            .await?;
        let mut connection = test_pool.acquire().await?;
        let (pid, birth): (i32, String) = sqlx::query_as(
            "SELECT pg_backend_pid(),backend_start::text
            FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&mut *connection)
        .await?;
        drop(connection);
        timeout(Duration::from_secs(1), async {
            while test_pool.num_idle() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let baseline = proxy.stats().statements;
        proxy.enable_exact_query_trace(16);
        let store = PgJournalBackend {
            pool: test_pool,
            daemon_pool: pool.clone(),
            tenant_id: tenant,
            ready_wait: super::super::JOURNAL_READY_WAIT,
            balance_account: false,
            order_set_projection: None,
            accounting_generation: Default::default(),
            non_trading: false,
        };
        Ok(Self { proxy, store, pid, birth, baseline })
    }

    fn assert_prefix(&self, count: usize) {
        let expected = [
            "BEGIN",
            "SELECT set_config('lock_timeout'",
            "UPDATE accounting_state",
            "INSERT INTO order_intents",
            "INSERT INTO balance_transitions",
            "COMMIT",
        ];
        let trace = self.proxy.query_trace();
        assert_eq!(trace.len(), count, "unexpected writer statements: {trace:?}");
        assert_eq!(self.proxy.stats().statements - self.baseline, count as u64);
        for (command, prefix) in trace.iter().zip(expected) {
            assert!(command.trim_start().starts_with(prefix), "expected {prefix}: {command:?}");
        }
    }

    pub(super) async fn retires(&self, observer: &PgPool) -> Result<()> {
        timeout(Duration::from_secs(2), async {
            loop {
                let present: bool = sqlx::query_scalar("SELECT EXISTS(
                    SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database())")
                    .bind(self.pid).bind(&self.birth).fetch_one(observer).await?;
                if !present {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok::<_, sqlx::Error>(())
        }).await??;
        Ok(())
    }
}

pub(super) async fn durable(pool: &PgPool, tenant: Uuid) -> Result<Value> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('version',(SELECT version FROM accounting_state
          WHERE tenant_id=$1 AND venue_id=$2),
          'cancels',(SELECT coalesce(jsonb_agg(to_jsonb(i) ORDER BY intent_id),'[]'::jsonb)
            FROM order_intents i WHERE i.tenant_id=$1 AND i.venue_id=$2 AND i.kind='cancel'),
          'transitions',(SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY transition_id),'[]'::jsonb)
            FROM balance_transitions t WHERE t.tenant_id=$1 AND t.venue_id=$2))",
    )
    .bind(tenant)
    .bind(VenueId::BINANCE_SPOT)
    .fetch_one(pool)
    .await?)
}

fn request(transition: Uuid, version: i64) -> TransitionWrite<'static> {
    TransitionWrite {
        symbol: SYMBOL,
        kind: TransitionKind::Replace,
        target_trigger: Decimal::new(95, 0),
        reason: "local-writer-control",
        expected_accounting_version: version,
        expected_transition: ExpectedTransition::New(transition),
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn spot_unflagged_backend_refuses_before_sql(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let saved = leg.cancel_intent();
    PgJournalBackend::new(&pool, leg.tenant)
        .commit_first_cancel(&saved, request(Uuid::now_v7(), leg.accounting_version(&pool).await?))
        .await
        .expect("flagged atomic fixture must persist");
    let before = durable(&pool, leg.tenant).await?;
    let wire = Wire::new(&pool, leg.tenant).await?;

    let linked = leg.cancel_intent();
    assert!(linked.position_id.is_some());
    let unlinked = IntentRow { position_id: None, ..leg.cancel_intent() };
    for (intent, outcome) in [
        (linked, None),
        (unlinked, None),
        (
            saved,
            Some(OutcomeRow {
                venue_outcome: Some(VenueOutcome::Success),
                ..OutcomeRow::default()
            }),
        ),
    ] {
        let result = wire
            .store
            .commit(&JournalBatch {
                intent,
                attempts: Vec::new(),
                outcome,
                balance: None,
            })
            .await;
        let after = durable(&pool, leg.tenant).await?;
        assert_eq!(after["version"], before["version"], "fixture retained accounting version");
        assert_eq!(after, before, "unflagged Spot mutation changed the journal at its old version");
        assert!(
            matches!(result, Err(CommitFailure::Known(ref reason))
            if reason == "Spot journal mutations require accounting-first storage"),
            "{result:?}"
        );
        wire.assert_prefix(0);
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_rejected_transition_leaves_no_fresh_journal(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let other = leg.another_leg(&pool, "4100002").await?;
    let mut intent = leg.cancel_intent();
    intent.cancels_order_id = other.cancel_intent().cancels_order_id;
    let version = leg.accounting_version(&pool).await?;
    let before = durable(&pool, leg.tenant).await?;
    let wire = Wire::new(&pool, leg.tenant).await?;
    let result = wire.store.commit_first_cancel(&intent, request(Uuid::now_v7(), version)).await;
    assert!(
        matches!(&result, Err(CommitFailure::Known(reason)) if reason.contains("first cancel must target the transition leg")),
        "{result:?}"
    );
    wire.retires(&pool).await?;
    assert_eq!(
        durable(&pool, leg.tenant).await?,
        before,
        "first_cancel_rejected_transition_leaves_no_fresh_journal"
    );
    wire.assert_prefix(5);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_refuses_missing_original_and_wrong_route(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let before = durable(&pool, leg.tenant).await?;
    for wrong_route in [false, true] {
        let wire = Wire::new(&pool, leg.tenant).await?;
        let mut intent = leg.cancel_intent();
        if wrong_route {
            intent.account_scope = "no-admitted-route".to_owned();
        } else {
            intent.cancels_order_id = Some("4100019".to_owned());
        }
        let result =
            wire.store.commit_first_cancel(&intent, request(Uuid::now_v7(), version)).await;
        assert!(
            matches!(&result, Err(CommitFailure::Known(reason)) if reason.contains("first cancel must target the transition leg")),
            "{result:?}"
        );
        wire.retires(&pool).await?;
        wire.assert_prefix(5);
        assert_eq!(durable(&pool, leg.tenant).await?, before);
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_lost_commit_ack_never_returns_receipt(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let intent = leg.cancel_intent();
    let transition = Uuid::now_v7();
    let wire = Wire::new(&pool, leg.tenant).await?;
    wire.proxy
        .set_mode(ProxyMode::DropCommitAckOf("INSERT INTO balance_transitions"));
    let result = wire.store.commit_first_cancel(&intent, request(transition, version)).await;
    wire.retires(&pool).await?;
    assert_eq!(wire.proxy.stats().commit_acks_dropped, 1);
    let actual = durable(&pool, leg.tenant).await?;
    assert_eq!(actual["version"], version + 1);
    assert_eq!(actual["cancels"].as_array().unwrap().len(), 1);
    assert_eq!(actual["transitions"].as_array().unwrap().len(), 1);
    assert_eq!(actual["transitions"][0]["transition_id"], json!(transition));
    assert_eq!(actual["transitions"][0]["first_cancel_intent_id"], json!(intent.intent_id));
    assert_eq!(actual["transitions"][0]["original_target_order_id"], ORIGINAL);
    leg.assert_bare_cancel(&pool, &intent).await?;
    assert!(
        matches!(&result, Err(CommitFailure::Uncertain(_))),
        "first_cancel_lost_commit_ack_never_returns_receipt: {result:?}"
    );
    wire.assert_prefix(6);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_native_commit_rejection_rolls_back_both_rows(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    sqlx::query("CREATE FUNCTION reject_first_cancel() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'native first cancel rejected at commit' USING ERRCODE='23514'; END $$")
        .execute(&pool).await?;
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER reject_first_cancel AFTER INSERT ON balance_transitions
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_first_cancel()",
    )
    .execute(&pool)
    .await?;
    let version = leg.accounting_version(&pool).await?;
    let before = durable(&pool, leg.tenant).await?;
    let intent = leg.cancel_intent();
    let wire = Wire::new(&pool, leg.tenant).await?;
    let result = wire.store.commit_first_cancel(&intent, request(Uuid::now_v7(), version)).await;
    assert!(
        matches!(&result, Err(CommitFailure::Uncertain(reason)) if reason.contains("native first cancel rejected at commit")),
        "{result:?}"
    );
    wire.retires(&pool).await?;
    wire.assert_prefix(6);
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_deadline_before_commit_discards_the_owned_connection(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let before = durable(&pool, leg.tenant).await?;
    let intent = leg.cancel_intent();
    let wire = Wire::new(&pool, leg.tenant).await?;
    let (slot, reached, release) = pause();
    let started = Instant::now();
    let apply = PAUSE
        .scope(slot, wire.store.commit_first_cancel(&intent, request(Uuid::now_v7(), version)));
    let verify = async {
        timeout(Duration::from_millis(400), reached).await??;
        wire.assert_prefix(5);
        let actual: String = sqlx::query_scalar(
            "SELECT state FROM pg_stat_activity
            WHERE pid=$1 AND backend_start::text=$2 AND datname=current_database()",
        )
        .bind(wire.pid)
        .bind(&wire.birth)
        .fetch_one(&pool)
        .await?;
        assert_eq!(actual, "idle in transaction");
        assert_eq!(durable(&pool, leg.tenant).await?, before);
        Ok::<_, anyhow::Error>(())
    };
    let (result, witness) = tokio::join!(apply, verify);
    witness?;
    let elapsed = started.elapsed();
    assert!(
        (COMMIT_BOUND..COMMIT_BOUND + Duration::from_millis(250)).contains(&elapsed),
        "{elapsed:?}"
    );
    assert!(
        matches!(result, Err(CommitFailure::Known(reason)) if reason.contains("deadline expired before COMMIT"))
    );
    assert!(release.send(()).is_err(), "expired F future remained runnable");
    wire.retires(&pool).await?;
    wire.assert_prefix(5);
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&wire.store.pool)
        .await?;
    assert_ne!(replacement, wire.pid);
    assert_eq!(wire.store.pool.size(), 1);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_barrier_and_expired_timer_ready_together_forbid_commit(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let before = durable(&pool, leg.tenant).await?;
    let intent = leg.cancel_intent();
    let wire = Wire::new(&pool, leg.tenant).await?;
    let (slot, reached, release) = pause();
    let apply = PAUSE
        .scope(slot, wire.store.commit_first_cancel(&intent, request(Uuid::now_v7(), version)));
    tokio::pin!(apply);
    tokio::select! {
        result = &mut apply => anyhow::bail!("unexpected completion before COMMIT barrier: {result:?}"),
        reached = timeout(Duration::from_millis(400), reached) => reached??,
    }
    wire.assert_prefix(5);
    // Keep the driver future unpolled while the real transaction stays open.
    // At the next poll both its barrier and outer timeout are ready.
    tokio::time::sleep(COMMIT_BOUND + Duration::from_millis(10)).await;
    release
        .send(())
        .map_err(|_| anyhow::anyhow!("writer stopped before resumption"))?;
    let result = apply.await;
    assert_eq!(
        result,
        Err(CommitFailure::Known(DEADLINE_BEFORE_COMMIT.into())),
        "first_cancel_barrier_and_expired_timer_ready_together_forbid_commit"
    );
    wire.retires(&pool).await?;
    wire.assert_prefix(5);
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_fresh_claim_refuses_spent_id_and_already_bound_open_row(
    pool: PgPool,
) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let intent = leg.cancel_intent();
    let version = leg.accounting_version(&pool).await?;
    let transition = Uuid::now_v7();
    let writer = Wire::new(&pool, leg.tenant).await?;
    let result = writer.store.commit_first_cancel(&intent, request(transition, version)).await;
    assert!(result.is_ok(), "{result:?}");
    writer.assert_prefix(6);
    let before = durable(&pool, leg.tenant).await?;
    let version = version + 1;

    let mut duplicate = leg.cancel_intent();
    duplicate.client_order_id.clone_from(&intent.client_order_id);
    let wire = Wire::new(&pool, leg.tenant).await?;
    let result = wire
        .store
        .commit_first_cancel(&duplicate, request(Uuid::now_v7(), version))
        .await;
    assert_eq!(result, Err(CommitFailure::IdSpent));
    wire.retires(&pool).await?;
    wire.assert_prefix(4);
    assert_eq!(durable(&pool, leg.tenant).await?, before);

    let fresh = leg.cancel_intent();
    let wire = Wire::new(&pool, leg.tenant).await?;
    let update = TransitionWrite {
        expected_transition: ExpectedTransition::ExistingOpen(transition),
        ..request(transition, version)
    };
    let result = wire.store.commit_first_cancel(&fresh, update).await;
    assert!(
        matches!(&result, Err(CommitFailure::Known(reason)) if reason.contains("no longer available")),
        "{result:?}"
    );
    wire.retires(&pool).await?;
    let trace = wire.proxy.query_trace();
    assert_eq!(trace.len(), 5);
    assert_eq!(wire.proxy.stats().statements - wire.baseline, 5);
    assert!(trace.last().unwrap().trim_start().starts_with("UPDATE balance_transitions"));
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    Ok(())
}

async fn compete(
    pool: &PgPool,
    leg: &PositiveLeg,
    same_id: bool,
    winner_kind: TransitionKind,
    price: i64,
) -> Result<()> {
    let version = leg.accounting_version(pool).await?;
    let winner_intent = leg.cancel_intent();
    let loser_intent = if same_id {
        winner_intent.clone()
    } else {
        leg.cancel_intent()
    };
    let winner = Wire::new(pool, leg.tenant).await?;
    let loser = Wire::new(pool, leg.tenant).await?;
    let transition = Uuid::now_v7();
    let winning = TransitionWrite {
        kind: winner_kind,
        target_trigger: Decimal::new(price, 0),
        ..request(transition, version)
    };
    let losing = TransitionWrite {
        kind: TransitionKind::Exit,
        ..request(Uuid::now_v7(), version)
    };
    let (slot, reached, release) = pause();
    let primary = PAUSE.scope(slot, winner.store.commit_first_cancel(&winner_intent, winning));
    let contender = async {
        timeout(Duration::from_millis(380), reached).await??;
        winner.assert_prefix(5);
        let secondary = loser.store.commit_first_cancel(&loser_intent, losing);
        tokio::pin!(secondary);
        let blocked = async {
            loop {
                let held: bool = sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))")
                    .bind(winner.pid)
                    .bind(loser.pid)
                    .fetch_one(pool)
                    .await?;
                if held {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(4)).await;
            }
            Ok::<_, sqlx::Error>(())
        };
        tokio::select! {
            early = &mut secondary => anyhow::bail!("contender finished before actual accounting block: {early:?}"),
            blocked = timeout(Duration::from_millis(220), blocked) => blocked??,
        }
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("primary commit barrier expired"))?;
        Ok::<_, anyhow::Error>(secondary.await)
    };
    let (result, contender) = tokio::join!(primary, contender);
    assert_eq!(
        result,
        Ok(StoredFirstCancel {
            transition_id: transition,
            intent_id: winner_intent.intent_id,
            accounting_version: version + 1,
        })
    );
    let refused = contender?;
    assert!(
        matches!(&refused, Err(CommitFailure::Known(reason)) if reason.contains("stale first-cancel accounting version")),
        "{refused:?}"
    );
    winner.assert_prefix(6);
    loser.retires(pool).await?;
    loser.assert_prefix(3);
    let final_state = durable(pool, leg.tenant).await?;
    let expected_kind = match winner_kind {
        TransitionKind::Exit => "exit",
        TransitionKind::Flatten => "flatten",
        TransitionKind::Replace => "replace",
        TransitionKind::Reprotect => "reprotect",
    };
    assert_eq!(final_state["version"], version + 1);
    assert_eq!(final_state["cancels"].as_array().unwrap().len(), 1);
    assert_eq!(final_state["transitions"].as_array().unwrap().len(), 1);
    assert_eq!(final_state["cancels"][0]["intent_id"], json!(winner_intent.intent_id));
    assert_eq!(final_state["transitions"][0]["transition_id"], json!(transition));
    assert_eq!(final_state["transitions"][0]["kind"], expected_kind);
    assert_eq!(final_state["transitions"][0]["target_trigger"], json!(price));
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_contender_cannot_pass_the_accounting_version_fence(
    pool: PgPool,
) -> Result<()> {
    use proptest::{
        prelude::*,
        test_runner::{Config, RngSeed, TestCaseResult, TestRunner},
    };
    migrate_fixture(&pool).await?;
    let mut runner = TestRunner::new(Config {
        cases: 10,
        rng_seed: RngSeed::Fixed(402018),
        failure_persistence: None,
        ..Config::default()
    });
    let generated = RefCell::new(Vec::new());
    runner
        .run(&(any::<bool>(), 0usize..4, 90i64..200), |case| {
            generated.borrow_mut().push(case);
            TestCaseResult::Ok(())
        })
        .map_err(|err| anyhow::anyhow!("generating competing intents: {err}"))?;
    let kinds = [
        TransitionKind::Replace,
        TransitionKind::Reprotect,
        TransitionKind::Flatten,
        TransitionKind::Exit,
    ];
    for (same_id, kind, price) in generated.into_inner() {
        let leg = PositiveLeg::admit(&pool).await?;
        compete(&pool, &leg, same_id, kinds[kind], price).await?;
    }
    Ok(())
}
