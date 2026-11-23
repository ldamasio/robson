//! ADR-0056 step 8l-4 proofs (PLAN-r4 §8): proxy traces of the journal's
//! incident, suspension and release writes, of the suspension clear and of
//! the projector's `monthly_state` writes on a balance-account daemon and on
//! USD-M, the lock order, generation progress through the owner's cell, a
//! stale generation's refusal kept pending, 8m's `v₁ + 2` and the runbook
//! model switch.
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Datelike, TimeZone, Utc};
use robson_eventlog::{ActorType, EventEnvelope};
use robson_exec::{
    gate::{Incident, IncidentReason},
    order_journal::{
        CommitFailure, JournalBackend, JournalBatch, OrderJournal, PendingEntry, ReleaseCounts,
        ReleaseRecord, ReleaseRefusal, StreamSuspensionRow, DEFAULT_ACCOUNT_SCOPE,
    },
    venue::{AccountKind, VenueId},
};
use robson_projector::{apply_event_to_projections_with, ProjectionError};
use robson_testkit::db_proxy::{DbProxy, ProxyMode};
use rust_decimal::Decimal;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use super::{projector_step, AccountingRefusal, BalanceAccounting, GenerationCell, BOUNDS};
use crate::{
    event_streams::{
        repair_position_stream, EventStreams, ManagedStream, RepairOutcome, Suspended,
    },
    order_journal_store::PgJournalBackend,
    process_epoch,
    spot_order_set_store::{observe_generation, ExpectedGeneration, JournalGeneration},
};

const SPOT: &str = VenueId::BINANCE_SPOT;
const BEGIN_TX: [&str; 4] = ["begin", "bounds", "bump", "cas"];

/// An accounting row of `venue` for `tenant_id`, at version 0.
async fn account(pool: &PgPool, tenant_id: Uuid, venue: &str) {
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'net_from_start_non_expanding_v1')",
    )
    .bind(tenant_id)
    .bind(venue)
    .execute(pool)
    .await
    .unwrap();
}

/// An order-set generation of `tenant_id` installed at version 0 in the
/// tenant's live epoch.
async fn install(pool: &PgPool, tenant_id: Uuid, account_scope: &str) -> JournalGeneration {
    let start = process_epoch::start_epoch(pool, tenant_id, "8l-4").await.unwrap();
    let generation = JournalGeneration {
        tenant_id,
        account_scope: account_scope.to_string(),
        generation_id: Uuid::now_v7(),
        process_epoch: start.epoch.epoch_id,
        source_audit_id: Uuid::now_v7(),
    };
    // Position hints complete, as the cold installer publishes a generation,
    // so 8k-2's pre statement can qualify it (`qualifies`).
    sqlx::query(
        "INSERT INTO spot_order_set_generation (tenant_id, venue_id, account_scope, \
         generation_id, process_epoch, source_audit_id, accounting_version, \
         position_hints_complete) VALUES ($1, $2, $3, $4, $5, $6, 0, true)",
    )
    .bind(tenant_id)
    .bind(SPOT)
    .bind(&generation.account_scope)
    .bind(generation.generation_id)
    .bind(generation.process_epoch)
    .bind(generation.source_audit_id)
    .execute(pool)
    .await
    .unwrap();
    generation
}

/// A Spot accounting row at version 0 and, when `installed`, an order-set
/// generation bound to it in the tenant's live epoch.
async fn seed(pool: &PgPool, installed: bool) -> BalanceAccounting {
    let tenant_id = Uuid::now_v7();
    account(pool, tenant_id, SPOT).await;
    let venue = VenueId::new(SPOT);
    let cell = GenerationCell::default();
    let accounting =
        BalanceAccounting::for_daemon(AccountKind::BalanceAccount, tenant_id, &venue, &cell)
            .unwrap();
    if installed {
        cell.publish(install(pool, tenant_id, &accounting.account_scope).await);
    }
    accounting
}

async fn version_of(pool: &PgPool, tenant_id: Uuid, venue: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT version FROM accounting_state WHERE (tenant_id, venue_id) = ($1, $2)",
    )
    .bind(tenant_id)
    .bind(venue)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The Spot row's version and its generation's `accounting_version`.
async fn versions(pool: &PgPool, accounting: &BalanceAccounting) -> (i64, Option<i64>) {
    sqlx::query_as(
        "SELECT a.version, g.accounting_version FROM accounting_state a \
         LEFT JOIN spot_order_set_generation g ON (g.tenant_id, g.venue_id) = (a.tenant_id, \
         a.venue_id) WHERE (a.tenant_id, a.venue_id) = ($1, $2)",
    )
    .bind(accounting.tenant_id)
    .bind(SPOT)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A writer outside 8l-4 bumps the version without the CAS.
async fn stale(pool: &PgPool, accounting: &BalanceAccounting) {
    sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id = $1")
        .bind(accounting.tenant_id)
        .execute(pool)
        .await
        .unwrap();
}

/// A cell holding `generation`, as the owner publishes it.
fn cell_of(generation: JournalGeneration) -> GenerationCell {
    let cell = GenerationCell::default();
    cell.publish(generation);
    cell
}

/// The order-set scan's qualification at pre: 8k-2's pre statement
/// (`btc_partition.sql` through `read_pre`) returns its header only while
/// the context's generation is the current one at the accounting version in
/// the live epoch. With 8k-2 composed (5a) it replaces 8l-4a's stand-in,
/// `observe_generation`, which must agree with it. The tenant's venue UID is
/// bound first, as 8k-2's rig binds it.
async fn qualifies(pool: &PgPool, accounting: &BalanceAccounting) -> bool {
    use crate::balance_reconciliation::{read_pre, DetectorScope};
    const UID: &str = "5a-scan";
    let installed = accounting.generation.current().unwrap();
    // Bound to a cutover run of the tenant (M56's foreign key).
    sqlx::query(concat!(
        include_str!("../tests/fixtures/bound_venue_account.sql"),
        " ON CONFLICT DO NOTHING"
    ))
    .bind(SPOT)
    .bind(&installed.account_scope)
    .bind(installed.tenant_id)
    .bind(UID)
    .execute(pool)
    .await
    .unwrap();
    let venue = VenueId::new(SPOT);
    let scope = DetectorScope {
        expected: ExpectedGeneration {
            tenant_id: installed.tenant_id,
            venue: &venue,
            account_scope: &installed.account_scope,
            generation_id: installed.generation_id,
            process_epoch: installed.process_epoch,
            source_audit_id: installed.source_audit_id,
        },
        uid: UID,
        operated: "BTCUSDT",
        base_asset: "BTC",
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let pre = read_pre(&pool.connect_options(), scope, deadline).await;
    let qualified = pre.is_ok();
    assert_eq!(qualified, current(pool, &installed).await, "8k-2's pre and observe_generation");
    qualified
}

async fn current(pool: &PgPool, installed: &JournalGeneration) -> bool {
    let venue = VenueId::new(SPOT);
    let mut connection = pool.acquire().await.unwrap();
    let expected = ExpectedGeneration {
        tenant_id: installed.tenant_id,
        venue: &venue,
        account_scope: &installed.account_scope,
        generation_id: installed.generation_id,
        process_epoch: installed.process_epoch,
        source_audit_id: installed.source_audit_id,
    };
    observe_generation(&mut connection, expected).await.unwrap().is_some()
}

async fn through_proxy(pool: &PgPool) -> (DbProxy, PgPool) {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    let proxy = DbProxy::start(upstream).await.unwrap();
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL").unwrap(),
        proxy.addr(),
        options.get_database().unwrap(),
    )
    .unwrap();
    // The operation's statements only: no acquisition ping.
    let proxied = PgPoolOptions::new()
        .max_connections(2)
        .test_before_acquire(false)
        .connect(&url)
        .await
        .unwrap();
    (proxy, proxied)
}

/// Each statement `work` sent through `proxy`, by what it is.
async fn traced<T>(proxy: &DbProxy, work: impl Future<Output = T>) -> (T, Vec<&'static str>) {
    proxy.enable_query_trace(64);
    let result = work.await;
    let shape = proxy
        .query_trace()
        .iter()
        .map(|sql| match sql.trim() {
            "BEGIN" => "begin",
            "COMMIT" => "commit",
            "ROLLBACK" => "rollback",
            sql if sql.starts_with("SELECT set_config('lock_timeout'") => "bounds",
            sql if sql.starts_with("WITH locked AS (") => "bump",
            sql if sql.contains("UPDATE public.spot_order_set_generation") => "cas",
            sql if sql.starts_with("UPDATE positions_current") => "positions",
            sql if sql.starts_with("INSERT INTO monthly_state") => "monthly",
            sql if sql.starts_with("UPDATE monthly_state") => "monthly",
            sql if sql.starts_with("LOCK TABLE monthly_state") => "lock",
            sql if sql.starts_with("UPDATE stream_suspensions SET cleared_at") => "clear",
            sql if sql.starts_with("INSERT INTO stream_suspensions") => "suspension",
            sql if sql.starts_with("INSERT INTO recovery_incidents") => "incident",
            sql if sql.starts_with("UPDATE order_intents") => "tombstone",
            sql if sql.starts_with("UPDATE recovery_incidents") => "released",
            sql if sql.starts_with("INSERT INTO journal_decisions") => "decision",
            _ => "other",
        })
        .collect();
    (result, shape)
}

/// A position stream suspended on record and in memory, with nothing to
/// apply: its repair goes straight to the clear.
async fn suspended(pool: &PgPool, tenant_id: Uuid) -> (EventStreams, String, Suspended) {
    let position_id = Uuid::now_v7();
    let stream_key = format!("position:{position_id}");
    let entry = Suspended {
        suspension_id: Uuid::now_v7(),
        stream: ManagedStream::Position(position_id),
        repaired_through_seq: None,
    };
    sqlx::query(
        "INSERT INTO stream_suspensions (suspension_id, tenant_id, stream_key, stream_kind, \
         failed_seq, error, suspended_at) VALUES ($1, $2, $3, 'position', 2, 'injected', NOW())",
    )
    .bind(entry.suspension_id)
    .bind(tenant_id)
    .bind(&stream_key)
    .execute(pool)
    .await
    .unwrap();
    let streams = EventStreams::default();
    streams.restore([(stream_key.clone(), entry.clone())]);
    (streams, stream_key, entry)
}

async fn cleared(pool: &PgPool, suspension: &Suspended) -> bool {
    sqlx::query_scalar(
        "SELECT cleared_at IS NOT NULL FROM stream_suspensions WHERE suspension_id = $1",
    )
    .bind(suspension.suspension_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn release_record() -> ReleaseRecord {
    ReleaseRecord {
        decision_id: Uuid::now_v7(),
        generation: 1,
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
        note: "8l-4".to_string(),
        treatment: "unbooked_accepted",
        decided_at: Utc::now(),
    }
}

fn suspension_row() -> StreamSuspensionRow {
    StreamSuspensionRow {
        suspension_id: Uuid::now_v7(),
        stream_key: format!("position:{}", Uuid::now_v7()),
        stream_kind: "position",
        failed_seq: None,
        month: None,
        error: "8l-4".to_string(),
        epoch_id: None,
        suspended_at: Utc::now(),
    }
}

/// One of the journal's three auxiliary writes.
async fn auxiliary(backend: &PgJournalBackend, write: &str) -> Result<(), String> {
    match write {
        "incident" => {
            let incident = Incident {
                reason: IncidentReason::UncertainOutcome,
                first_at: Utc::now(),
                last_at: Utc::now(),
                count: 1,
                first_generation: 1,
                last_generation: 1,
            };
            backend.upsert_incident(&incident).await.map_err(|f| format!("{f:?}"))
        },
        "suspension" => {
            backend.record_suspension(&suspension_row()).await.map_err(|f| format!("{f:?}"))
        },
        _ => backend.release(&release_record()).await.map(drop),
    }
}

/// A balance journal's incident, suspension and release each add the bump
/// and the CAS right after main's bounds (6, 6, 8), move the version once and
/// keep the generation current; with none installed only the bump is added
/// (5, 5, 7); the legacy (USD-M) journal sends exactly its statements (3, 3,
/// 5) and moves nothing. A non-trading journal's are the plain balance
/// journal's (PS-1).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_journal_writers_add_the_bump_and_the_cas(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let unopted = seed(&pool, false).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let opted = PgJournalBackend::new(&proxied, installed.tenant_id)
        .with_balance_account(true)
        .with_accounting_generation(&installed.generation);
    let plain = PgJournalBackend::new(&proxied, unopted.tenant_id).with_balance_account(true);
    let legacy = PgJournalBackend::new(&proxied, unopted.tenant_id);
    // PS-1: a non-trading Spot journal as the daemon wires it (balance, the
    // daemon's empty cell) refuses every batch, and F's writes run as before.
    let non_trading = PgJournalBackend::new(&proxied, unopted.tenant_id)
        .with_accounting_generation(&GenerationCell::default())
        .with_balance_account(true)
        .non_trading();
    let journals: [(&PgJournalBackend, &BalanceAccounting, &[&str]); 4] = [
        (&opted, &installed, &["bounds", "bump", "cas"]),
        (&plain, &unopted, &["bounds", "bump"]),
        (&legacy, &unopted, &[]),
        (&non_trading, &unopted, &["bounds", "bump"]),
    ];
    for (backend, accounting, added) in journals {
        backend.check_ready().await.unwrap();
        for write in ["incident", "suspension", "release"] {
            let body: &[&str] = match write {
                "incident" => &["incident"],
                "suspension" => &["suspension"],
                _ => &["tombstone", "released", "decision"],
            };
            let (version, generation) = versions(&pool, accounting).await;
            let (written, shape) = traced(&proxy, auxiliary(backend, write)).await;
            written.unwrap_or_else(|error| panic!("{write}: {error}"));
            assert_eq!(shape, [&["begin"][..], added, body, &["commit"][..]].concat(), "{write}");
            let moved = i64::from(!added.is_empty());
            let expected = (version + moved, generation.map(|g| g + moved));
            assert_eq!(versions(&pool, accounting).await, expected, "{write}");
        }
    }
    assert!(qualifies(&pool, &installed).await);
}

/// A balance release moves the version once and the installed generation
/// with it, so the order-set scan still qualifies. With the generation left
/// stale by a writer that bumped without it, the release refuses and nothing
/// of it lands.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_release_bumps_the_version(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let backend = PgJournalBackend::new(&pool, installed.tenant_id)
        .with_balance_account(true)
        .with_accounting_generation(&installed.generation);
    let decided = |record: ReleaseRecord| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM journal_decisions WHERE decision_id = $1",
            )
            .bind(record.decision_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let record = release_record();
    backend.release(&record).await.unwrap();
    assert_eq!(versions(&pool, &installed).await, (1, Some(1)));
    assert!(qualifies(&pool, &installed).await);
    assert_eq!(decided(record).await, 1);

    stale(&pool, &installed).await;
    let record = release_record();
    let refused = backend.release(&record).await.unwrap_err();
    assert!(refused.contains("stale order-set generation"), "{refused}");
    assert_eq!(versions(&pool, &installed).await, (2, Some(1)));
    assert_eq!(decided(record).await, 0);
}

/// On a balance daemon the startup repair clears a suspension in one
/// transaction of six (BEGIN, bounds, bump, CAS, the clear, COMMIT) and only
/// then lets the stream take appends; on USD-M the clear stays one
/// autocommit statement and moves nothing; a stale generation keeps the
/// suspension on record and in memory.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn suspension_clear_bumps_and_advances_the_generation(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    let (proxy, proxied) = through_proxy(&pool).await;

    let (streams, key, entry) = suspended(&pool, tenant).await;
    let repair = repair_position_stream(&proxied, tenant, &streams, &key, &entry, Some(&installed));
    let (outcome, shape) = traced(&proxy, repair).await;
    assert_eq!(outcome, RepairOutcome::Cleared { applied: 0 });
    let tail = ["begin", "bounds", "bump", "cas", "clear", "commit"];
    assert_eq!(shape[shape.len() - 6..], tail, "{shape:?}");
    assert_eq!(shape.iter().filter(|s| **s == "begin").count(), 1, "{shape:?}");
    assert_eq!(versions(&pool, &installed).await, (1, Some(1)));
    assert!(qualifies(&pool, &installed).await);
    assert!(cleared(&pool, &entry).await && !streams.is_suspended(&key));

    let (streams, key, entry) = suspended(&pool, tenant).await;
    let repair = repair_position_stream(&proxied, tenant, &streams, &key, &entry, None);
    let (outcome, shape) = traced(&proxy, repair).await;
    assert_eq!(outcome, RepairOutcome::Cleared { applied: 0 });
    assert_eq!(shape.last(), Some(&"clear"), "{shape:?}");
    assert_eq!(shape.iter().filter(|s| **s == "clear").count(), 1, "{shape:?}");
    assert!(
        !shape.iter().any(|s| ["begin", "bounds", "bump", "cas"].contains(s)),
        "{shape:?}"
    );
    assert_eq!(versions(&pool, &installed).await, (1, Some(1)));
    assert!(cleared(&pool, &entry).await && !streams.is_suspended(&key));

    stale(&pool, &installed).await;
    let (streams, key, entry) = suspended(&pool, tenant).await;
    let outcome = repair_position_stream(&pool, tenant, &streams, &key, &entry, Some(&installed));
    assert!(matches!(outcome.await, RepairOutcome::Failed { .. }));
    assert_eq!(versions(&pool, &installed).await, (2, Some(1)));
    assert!(!cleared(&pool, &entry).await && streams.is_suspended(&key));
}

/// A balance clear whose COMMIT goes unanswered is uncertain, never taken as
/// landed: the repair reports a failure and the stream stays suspended in
/// this process, while the transaction did commit whole (cleared, the version
/// and the generation moved together), so the next start has nothing left to
/// repair. A failure before COMMIT leaves nothing (the stale case above).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_commit_ack_on_a_balance_clear_keeps_the_stream_suspended(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    let (proxy, proxied) = through_proxy(&pool).await;
    let (streams, key, entry) = suspended(&pool, tenant).await;
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE stream_suspensions SET cleared_at"));
    let outcome =
        repair_position_stream(&proxied, tenant, &streams, &key, &entry, Some(&installed));
    let RepairOutcome::Failed { error } = outcome.await else {
        panic!("a lost COMMIT answer must not read as cleared");
    };
    assert!(error.starts_with("clearing the suspension"), "{error}");
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
    assert!(streams.is_suspended(&key));
    assert!(cleared(&pool, &entry).await);
    assert_eq!(versions(&pool, &installed).await, (1, Some(1)));
    assert!(qualifies(&pool, &installed).await);
    let open = crate::event_streams::load_open_suspensions(&pool, tenant).await.unwrap();
    assert!(open.iter().all(|(stream_key, _)| *stream_key != key), "{open:?}");
}

/// The journal's backend with every failure of a durable incident or
/// suspension write recorded, for this test only: the very `failure` the
/// journal's retry pass logs in its warn lines (`order_journal.rs`, "Incident
/// not recorded yet" and "Stream suspension not recorded yet") before it keeps
/// the entry pending. A per-test record, unlike a tracing subscriber, whose
/// callsite interest is process-global and races with concurrent tests.
struct Refusals<B> {
    backend: B,
    seen: Mutex<Vec<String>>,
}

impl<B> Refusals<B> {
    fn note<T>(&self, result: Result<T, CommitFailure>) -> Result<T, CommitFailure> {
        if let Err(failure) = &result {
            self.seen.lock().unwrap().push(format!("{failure:?}"));
        }
        result
    }
}

#[async_trait::async_trait]
impl<B: JournalBackend> JournalBackend for Refusals<B> {
    async fn commit(&self, batch: &JournalBatch) -> Result<(), CommitFailure> {
        self.backend.commit(batch).await
    }

    async fn attempt_states(
        &self,
        venue: &str,
        scope: &str,
        id: &str,
    ) -> Result<Vec<bool>, String> {
        self.backend.attempt_states(venue, scope, id).await
    }

    async fn check_ready(&self) -> Result<(), String> {
        self.backend.check_ready().await
    }

    async fn check_balance_schema(&self, venue: &str, index: bool) -> Result<(), String> {
        self.backend.check_balance_schema(venue, index).await
    }

    async fn record_suspension(&self, row: &StreamSuspensionRow) -> Result<(), CommitFailure> {
        self.note(self.backend.record_suspension(row).await)
    }

    async fn upsert_incident(&self, incident: &Incident) -> Result<(), CommitFailure> {
        self.note(self.backend.upsert_incident(incident).await)
    }

    async fn release(&self, release: &ReleaseRecord) -> Result<ReleaseCounts, String> {
        self.backend.release(release).await
    }
}

/// The incident and the suspension a stale generation refused, as rows.
async fn landed(pool: &PgPool, tenant: Uuid, stream_key: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM recovery_incidents WHERE tenant_id = $1 \
         AND reason = 'stream_suspended'), (SELECT count(*) FROM stream_suspensions \
         WHERE tenant_id = $1 AND stream_key = $2)",
    )
    .bind(tenant)
    .bind(stream_key)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A stale generation refuses the durable incident and suspension writes,
/// and nothing opens or is lost (Root's condition (iv) on design choice 4):
/// (1) the latch and the fence stay closed, so admissions stay refused
/// (`check_admissions_open` reads exactly these) and a release is refused;
/// (2) every retry pass tries both writes again and receives the backend's
/// refusal, the `failure` its warn lines print (recorded per test, see
/// `Refusals`), and the retry-set depth the `robsond_journal_retry_set_depth`
/// gauge exports stays at both; (3) the order-set owner's
/// rebuild rotates the generation's identity as `cold_install`'s
/// `publish.sql` does, the journal's pass still refuses on the identity it
/// read, and once the owner publishes the new one into the shared cell the
/// next pass lands both with the bump and the CAS.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stale_generation_keeps_durable_incident_and_suspension_writes_pending(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    let backend = Arc::new(Refusals {
        backend: PgJournalBackend::new(&pool, tenant)
            .with_balance_account(true)
            .with_accounting_generation(&installed.generation),
        seen: Mutex::default(),
    });
    let journal = Arc::new(
        OrderJournal::new(backend.clone(), &VenueId::new(SPOT)).with_balance_account(true),
    );
    stale(&pool, &installed).await;
    let suspension = suspension_row();
    let stream_key = suspension.stream_key.clone();
    journal.record_suspension(suspension);
    assert!(journal.latch_set() && journal.gate().fenced());

    // Every pass tries both writes, receives the backend's refusal (what its
    // warn lines print) and keeps both pending: the depth the
    // `robsond_journal_retry_set_depth` gauge exports.
    for pass in 1..=3 {
        assert_eq!(journal.flush_pending().await, 0);
        assert_eq!(journal.retry_set_depth(), 2);
        let seen = backend.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2 * pass, "{seen:?}");
        assert!(
            seen.iter().all(|failure| failure.contains("stale order-set generation")),
            "{seen:?}"
        );
    }
    let pending = journal.pending_entries();
    assert!(pending.contains(&PendingEntry::Incident(IncidentReason::StreamSuspended)));
    assert!(pending.contains(&PendingEntry::Suspension(stream_key.clone())));
    assert!(journal.latch_set() && journal.gate().fenced());
    let reasons: Vec<_> = journal.gate().open_incidents().iter().map(|i| i.reason).collect();
    assert!(reasons.contains(&IncidentReason::StreamSuspended), "{reasons:?}");
    let quoted = ReleaseRecord {
        generation: journal.gate().generation(),
        ..release_record()
    };
    assert_eq!(journal.release(quoted).await.unwrap_err(), ReleaseRefusal::Latched);
    assert_eq!(landed(&pool, tenant, &stream_key).await, (0, 0));
    assert_eq!(versions(&pool, &installed).await, (1, Some(0)));

    // The owner's rebuild, as publish.sql's update: a new generation_id and
    // source_audit_id at the current version in the live epoch.
    let old = installed.generation.current().unwrap();
    let rotated = JournalGeneration {
        generation_id: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
        ..old.clone()
    };
    sqlx::query(
        "UPDATE public.spot_order_set_generation g SET generation_id = $3, process_epoch = $4, \
         source_audit_id = $5, accounting_version = a.version, position_hints_complete = true \
         FROM public.accounting_state a WHERE (a.tenant_id, a.venue_id) = (g.tenant_id, \
         g.venue_id) AND (g.tenant_id, g.venue_id, g.account_scope) = ($1, 'binance:spot', $2) \
         AND g.generation_id = $6 AND g.generation_id <> $3",
    )
    .bind(tenant)
    .bind(&old.account_scope)
    .bind(rotated.generation_id)
    .bind(rotated.process_epoch)
    .bind(rotated.source_audit_id)
    .bind(old.generation_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(current(&pool, &rotated).await && !current(&pool, &old).await);
    assert_eq!(journal.flush_pending().await, 0, "the journal still reads the old identity");
    assert_eq!(journal.retry_set_depth(), 2);
    installed.generation.publish(rotated.clone());
    assert_eq!(journal.flush_pending().await, 2);
    assert_eq!(journal.retry_set_depth(), 0);
    assert_eq!(landed(&pool, tenant, &stream_key).await, (1, 1));
    assert_eq!(versions(&pool, &installed).await, (3, Some(3)));
    assert!(current(&pool, &rotated).await);
    assert!(journal.latch_set() && journal.gate().fenced());
}

#[derive(Default)]
struct Marker(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl crate::stop::StopMarker for Marker {
    async fn mark_clean(&self) -> Result<(), String> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// Condition (iv), point 4: a stop while a refused durable write is pending
/// is never clean. Its final flush is refused like every pass, so it ends at
/// its deadline with the marker never attempted, the epoch is left without
/// a clean stop, and the tenant's next start is unclean with its
/// `unclean_start` incident open: the fence a daemon seeds from it at start
/// (`daemon::tests::an_unclean_start_is_fenced_until_a_release_and_only_then_stops_clean`).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stop_with_a_refused_durable_write_pending_is_never_clean(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    let backend = PgJournalBackend::new(&pool, tenant)
        .with_balance_account(true)
        .with_accounting_generation(&installed.generation);
    let journal =
        OrderJournal::new(Arc::new(backend), &VenueId::new(SPOT)).with_balance_account(true);
    stale(&pool, &installed).await;
    journal.record_suspension(suspension_row());
    let marker = Marker::default();
    let timings = crate::stop::StopTimings {
        drain_limit: std::time::Duration::from_millis(100),
        deadline: std::time::Duration::from_secs(1),
    };
    let asked = tokio::time::Instant::now();
    let report = crate::stop::run_stop(&journal, asked, || {}, Some(&marker), timings).await;
    assert_eq!(report.result, crate::stop::StopResult::Deadline);
    assert_eq!(report.marker, crate::stop::MarkerWrite::NotAttempted);
    assert!(!marker.0.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(journal.retry_set_depth(), 2);
    let next = process_epoch::start_epoch(&pool, tenant, "8l-4 next").await.unwrap();
    assert_eq!(next.predecessor.unclean_reason(), Some("no_marker"));
    assert!(next.unclean_generation.is_some());
    let reasons: Vec<_> = next.open_incidents.iter().map(|i| i.reason).collect();
    assert!(reasons.contains(&IncidentReason::UncleanStart), "{reasons:?}");
}

/// The context's binding is checked before any SQL: a generation of another
/// tenant or scope, an invalid one, or a context venue other than Spot
/// refuses with nothing sent. An installed generation whose tenant has no
/// Spot row refuses after the bump, and its rollback leaves every row as it
/// was.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn advance_refuses_a_generation_that_is_not_its_own(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let generation = installed.generation.current().unwrap();
    let usdm = VenueId::new(VenueId::BINANCE_USDM);
    let (proxy, proxied) = through_proxy(&pool).await;
    let invalid = JournalGeneration {
        source_audit_id: Uuid::nil(),
        ..generation.clone()
    };
    let refused = [
        BalanceAccounting {
            tenant_id: Uuid::now_v7(),
            ..installed.clone()
        },
        BalanceAccounting {
            account_scope: "another-scope".to_string(),
            ..installed.clone()
        },
        BalanceAccounting { venue: usdm.clone(), ..installed.clone() },
        BalanceAccounting {
            generation: cell_of(invalid),
            ..installed.clone()
        },
    ];
    for accounting in refused {
        let mut tx = proxied.begin().await.unwrap();
        let (advanced, shape) = traced(&proxy, accounting.advance(&mut tx)).await;
        assert!(matches!(advanced, Err(AccountingRefusal::Binding)), "{advanced:?}");
        assert!(shape.is_empty(), "{shape:?}");
        tx.rollback().await.unwrap();
    }
    assert_eq!(versions(&pool, &installed).await, (0, Some(0)));

    let tenant = Uuid::now_v7();
    account(&pool, tenant, usdm.as_str()).await;
    let orphan = BalanceAccounting {
        tenant_id: tenant,
        generation: cell_of(install(&pool, tenant, DEFAULT_ACCOUNT_SCOPE).await),
        ..installed.clone()
    };
    let mut tx = pool.begin().await.unwrap();
    let advanced = orphan.advance(&mut tx).await;
    assert!(matches!(advanced, Err(AccountingRefusal::NoAccountingRow)), "{advanced:?}");
    tx.rollback().await.unwrap();
    assert_eq!(version_of(&pool, tenant, usdm.as_str()).await, 0);
}

/// Every 8l-4a writer bumps every accounting row of its tenant, and no other
/// tenant's: a balance journal's incident and a balance clear each move the
/// tenant's Spot and USD-M rows once, the CAS follows the Spot row, and
/// another tenant's row stays where it was.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_bump_moves_every_accounting_row_of_the_tenant_and_no_other(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    account(&pool, tenant, VenueId::BINANCE_USDM).await;
    let other = seed(&pool, false).await;
    let backend = PgJournalBackend::new(&pool, tenant)
        .with_balance_account(true)
        .with_accounting_generation(&installed.generation);
    auxiliary(&backend, "incident").await.unwrap();
    assert_eq!(versions(&pool, &installed).await, (1, Some(1)));
    assert_eq!(version_of(&pool, tenant, VenueId::BINANCE_USDM).await, 1);
    let (streams, key, entry) = suspended(&pool, tenant).await;
    let outcome = repair_position_stream(&pool, tenant, &streams, &key, &entry, Some(&installed));
    assert_eq!(outcome.await, RepairOutcome::Cleared { applied: 0 });
    assert_eq!(versions(&pool, &installed).await, (2, Some(2)));
    assert_eq!(version_of(&pool, tenant, VenueId::BINANCE_USDM).await, 2);
    assert_eq!(versions(&pool, &other).await, (0, None));
    assert!(qualifies(&pool, &installed).await);
}

/// Concurrent 8l-4a writers on one tenant (incidents and suspensions from
/// four balance journals, and suspension clears) lock its two accounting
/// rows in the same order: no write fails with a deadlock, every write that
/// landed moved each row exactly once, one refused (its lock or pool bound)
/// moved nothing, and the generation ends current.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn concurrent_writers_keep_the_version_and_the_generation_together(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let tenant = installed.tenant_id;
    account(&pool, tenant, VenueId::BINANCE_USDM).await;
    let backends: Vec<_> = (0..4)
        .map(|_| {
            Arc::new(
                PgJournalBackend::new(&pool, tenant)
                    .with_balance_account(true)
                    .with_accounting_generation(&installed.generation),
            )
        })
        .collect();
    for backend in &backends {
        backend.check_ready().await.unwrap();
    }
    let mut writers = tokio::task::JoinSet::new();
    for round in 0..24 {
        let backend = Arc::clone(&backends[round % backends.len()]);
        let write = if round % 3 == 0 {
            "incident"
        } else {
            "suspension"
        };
        writers.spawn(async move { auxiliary(&backend, write).await });
    }
    for _ in 0..6 {
        let (pool, accounting) = (pool.clone(), installed.clone());
        let (streams, key, entry) = suspended(&pool, tenant).await;
        writers.spawn(async move {
            let outcome =
                repair_position_stream(&pool, tenant, &streams, &key, &entry, Some(&accounting));
            match outcome.await {
                RepairOutcome::Cleared { .. } => Ok(()),
                other => Err(format!("{other:?}")),
            }
        });
    }
    let (mut landed, mut refused) = (0, Vec::new());
    while let Some(written) = writers.join_next().await {
        match written.unwrap() {
            Ok(()) => landed += 1,
            Err(error) => refused.push(error),
        }
    }
    println!("ACCOUNTING_8L4_CONCURRENT writes=30 landed={landed} refused={}", refused.len());
    assert!(!refused.iter().any(|error| error.contains("deadlock")), "{refused:?}");
    assert!(landed > 0, "{refused:?}");
    assert_eq!(versions(&pool, &installed).await, (landed, Some(landed)));
    assert_eq!(version_of(&pool, tenant, VenueId::BINANCE_USDM).await, landed);
    assert!(qualifies(&pool, &installed).await);
}

/// The runbook's balance model switch is one transaction: it bumps every
/// accounting row of the tenant with the model, then switches the month; a
/// failure in the month's update leaves the version and the model as they
/// were.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn runbook_model_switch_bumps_with_the_model_in_one_transaction(pool: PgPool) {
    let runbook = include_str!("../../docs/runbooks/activate-net-from-start-budget.md");
    let block = runbook.split("<!-- 8l-4 balance model switch -->").nth(1).unwrap();
    let line = block
        .lines()
        .find(|line| line.trim_start().starts_with("robson_psql \""))
        .unwrap();
    let sql = &line[line.find('"').unwrap() + 1..line.rfind('"').unwrap()];
    let tenant = Uuid::now_v7();
    let switch = sql
        .replace("${TENANT_ID}", &tenant.to_string())
        .replace("${TARGET_YEAR}", "2027")
        .replace("${TARGET_MONTH}", "4");
    let state = || async {
        sqlx::query_as::<_, (i64, String, String)>(
            "SELECT a.version, a.budget_model, m.monthly_budget_model FROM accounting_state a, \
             monthly_state m WHERE a.tenant_id = $1 AND (m.year, m.month) = (2027, 4)",
        )
        .bind(tenant)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    sqlx::query(
        "INSERT INTO monthly_state (year, month, capital_base, monthly_budget_model) \
         VALUES (2027, 4, 10000, 'hwm_v1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'hwm_v1')",
    )
    .bind(tenant)
    .bind(SPOT)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION refuse_month() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER refuse_month BEFORE \
         UPDATE ON monthly_state FOR EACH ROW EXECUTE FUNCTION refuse_month();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(sqlx::raw_sql(&switch).execute(&pool).await.is_err());
    let hwm = "hwm_v1".to_string();
    assert_eq!(state().await, (0, hwm.clone(), hwm));

    sqlx::query("DROP TRIGGER refuse_month ON monthly_state")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(&switch).execute(&pool).await.unwrap();
    let net = "net_from_start_non_expanding_v1".to_string();
    assert_eq!(state().await, (1, net.clone(), net));
}

/// The production text of a source: everything before its unit-test module.
fn production(source: &str) -> &str {
    source.split("\n#[cfg(test)]\nmod tests {").next().unwrap()
}

/// Only a balance-account daemon builds a context, unopted until the
/// order-set owner publishes a generation into the cell it shares with the
/// journal; the daemon builds one cell and hands it to the journal and to the
/// manager, whose startup repair passes it to the clear.
#[test]
fn only_a_balance_account_daemon_builds_a_context() {
    let tenant = Uuid::now_v7();
    let spot = VenueId::new(SPOT);
    let cell = GenerationCell::default();
    let position =
        BalanceAccounting::for_daemon(AccountKind::PositionAccount, tenant, &spot, &cell);
    assert!(position.is_none());
    let daemon =
        BalanceAccounting::for_daemon(AccountKind::BalanceAccount, tenant, &spot, &cell).unwrap();
    let journal = BalanceAccounting::for_journal(tenant, &cell);
    for context in [&daemon, &journal] {
        assert_eq!((context.tenant_id, context.venue.as_str()), (tenant, SPOT));
        assert_eq!(context.account_scope, DEFAULT_ACCOUNT_SCOPE);
        assert_eq!(context.generation.current(), None);
    }
    let generation = JournalGeneration {
        tenant_id: tenant,
        account_scope: DEFAULT_ACCOUNT_SCOPE.to_string(),
        generation_id: Uuid::now_v7(),
        process_epoch: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
    };
    cell.publish(generation.clone());
    assert_eq!(daemon.generation.current(), Some(generation.clone()));
    assert_eq!(journal.generation.current(), Some(generation));

    let daemon = production(include_str!("daemon.rs"));
    assert_eq!(daemon.matches("GenerationCell::default()").count(), 1);
    let shared = "with_durable_order_journal(executor, &venue, &pg_pool, &config, &generation)";
    assert!(daemon.contains(shared) && daemon.contains(".with_accounting_generation(generation)"));
    assert_eq!(daemon.matches("BalanceAccounting::for_daemon(").count(), 1);
    let built = daemon.split("BalanceAccounting::for_daemon(").nth(1).unwrap();
    assert!(built[..built.find(");").unwrap()].contains("&generation"), "{built}");
    assert!(daemon.contains(".with_balance_accounting(accounting)"));
    let manager = production(include_str!("position_manager.rs"));
    let repair = manager.split("repair_position_stream(").nth(1).unwrap();
    let call = &repair[..repair.find(".await").unwrap()];
    assert!(call.contains("self.balance_accounting.as_ref()"), "{call}");
}

fn envelope(event_type: &str, payload: serde_json::Value, at: DateTime<Utc>) -> EventEnvelope {
    EventEnvelope {
        event_id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        stream_key: "test:8l-4".to_string(),
        seq: 1,
        event_type: event_type.to_string(),
        payload,
        payload_schema_version: 1,
        occurred_at: at,
        ingested_at: at,
        idempotency_key: Uuid::now_v7().to_string(),
        trace_id: None,
        causation_id: None,
        command_id: None,
        workflow_id: None,
        actor_type: Some(ActorType::Daemon),
        actor_id: None,
        prev_hash: None,
        hash: None,
    }
}

fn reset(at: DateTime<Utc>, base: &str) -> EventEnvelope {
    let payload = serde_json::json!({
        "capital_base": base, "carried_positions_risk": "0",
        "month": at.month(), "year": at.year(), "timestamp": at,
    });
    envelope("month_boundary_reset", payload, at)
}

fn recalibration(at: DateTime<Utc>, base: &str) -> EventEnvelope {
    let payload = serde_json::json!({
        "previous_capital_base": "10000", "new_capital_base": base, "wallet_balance": base,
        "carried_risk": "0", "reason": "spot_cutover_basis", "evidence": "8l-4",
        "month": at.month(), "year": at.year(), "timestamp": at,
    });
    envelope("capital_base_recalibrated", payload, at)
}

fn fill(at: DateTime<Utc>) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": Uuid::now_v7(), "order_id": Uuid::now_v7(), "fill_price": "100",
        "filled_quantity": "1", "fee": "0.1", "initial_stop": "90",
        "invalidation_guard_level": null, "timestamp": at,
    });
    envelope("entry_filled", payload, at)
}

fn losing_close(at: DateTime<Utc>) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": Uuid::now_v7(), "exit_reason": "TrailingStop", "entry_price": "100",
        "exit_price": "90", "realized_pnl": "-10", "total_fees": "1", "timestamp": at,
    });
    envelope("position_closed", payload, at)
}

fn april() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap()
}

/// The month's capital base, carried risk and model.
async fn month(pool: &PgPool, at: DateTime<Utc>) -> (Decimal, Decimal, String) {
    sqlx::query_as(
        "SELECT capital_base, carried_risk, monthly_budget_model FROM monthly_state \
         WHERE year = $1 AND month = $2",
    )
    .bind(at.year() as i16)
    .bind(at.month() as i16)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Balance daemon: each autocommit `monthly_state` path is one transaction of
/// six (BEGIN, bounds, one bump, CAS, the write, COMMIT); the counter fallback
/// is 6 + 7 = 13; each transaction bumps once and keeps the generation
/// current. Without an installed generation it is one statement shorter.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_projector_sends_six_per_path_and_thirteen_on_the_fallback(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let may = Utc.with_ymd_and_hms(2027, 5, 2, 0, 0, 10).unwrap();
    let tx = [&BEGIN_TX[..], &["monthly", "commit"][..]].concat();
    let fallback = [&BEGIN_TX[..], &["lock", "monthly", "commit"][..]].concat();
    let cases = [
        (reset(april(), "10000"), tx.clone(), 1),
        (recalibration(april(), "9000"), tx.clone(), 1),
        (fill(april()), [&["positions"][..], &tx[..]].concat(), 1),
        (losing_close(april()), [&["positions"][..], &tx[..]].concat(), 1),
        (fill(may), [&["positions"][..], &tx[..], &fallback[..]].concat(), 2),
    ];
    let step = projector_step(Some(&installed));
    for (event, expected, bumps) in cases {
        let (version, generation) = versions(&pool, &installed).await;
        let applied = apply_event_to_projections_with(&proxied, &event, step);
        let (applied, shape) = traced(&proxy, applied).await;
        applied.unwrap();
        assert_eq!(shape, expected, "{}", event.event_type);
        let moved = (version + bumps, generation.map(|g| g + bumps));
        assert_eq!(versions(&pool, &installed).await, moved, "{}", event.event_type);
        assert!(qualifies(&pool, &installed).await, "{}", event.event_type);
    }

    let unopted = seed(&pool, false).await;
    let event = reset(may, "10000");
    let applied = apply_event_to_projections_with(&proxied, &event, projector_step(Some(&unopted)));
    let (applied, shape) = traced(&proxy, applied).await;
    applied.unwrap();
    assert_eq!(shape, ["begin", "bounds", "bump", "monthly", "commit"]);
    assert_eq!(versions(&pool, &unopted).await, (1, None));
}

/// USD-M (a position account) gets no context from the daemon, and every
/// `monthly_state` path sends exactly its pre-8l statements: one autocommit
/// write, or the counter fallback's five; no accounting row of the tenant
/// moves.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_usdm_projector_issues_its_pre_8l_statements(pool: PgPool) {
    let tenant_id = Uuid::now_v7();
    let usdm = VenueId::new(VenueId::BINANCE_USDM);
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'hwm_v1')",
    )
    .bind(tenant_id)
    .bind(usdm.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let cell = GenerationCell::default();
    let accounting =
        BalanceAccounting::for_daemon(AccountKind::PositionAccount, tenant_id, &usdm, &cell);
    let step = projector_step(accounting.as_ref());
    let (proxy, proxied) = through_proxy(&pool).await;
    let may = Utc.with_ymd_and_hms(2027, 5, 2, 0, 0, 10).unwrap();
    let cases: [(EventEnvelope, &[&str]); 5] = [
        (reset(april(), "10000"), &["monthly"]),
        (recalibration(april(), "9000"), &["monthly"]),
        (fill(april()), &["positions", "monthly"]),
        (losing_close(april()), &["positions", "monthly"]),
        (fill(may), &["positions", "monthly", "begin", "lock", "monthly", "commit"]),
    ];
    for (event, expected) in cases {
        let applied = apply_event_to_projections_with(&proxied, &event, step);
        let (applied, shape) = traced(&proxy, applied).await;
        applied.unwrap();
        assert_eq!(shape, expected, "{}", event.event_type);
    }
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, 0);
}

/// The projector takes the accounting rows before `monthly_state`: the bump
/// precedes the write in its trace; and an apply that finds the tenant's
/// accounting row held waits on it, is refused at its 300 ms lock bound
/// (`BOUNDS`) while the row stays held, and never reached `monthly_state`:
/// its whole trace has the bump and no `monthly_state` statement, and
/// nothing changed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn projector_takes_the_accounting_row_before_monthly_state(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let step = projector_step(Some(&installed));
    let event = reset(april(), "10000");
    let applied = apply_event_to_projections_with(&proxied, &event, step);
    let (applied, shape) = traced(&proxy, applied).await;
    applied.unwrap();
    let at = |kind: &str| shape.iter().position(|s| *s == kind).unwrap();
    assert!(at("bump") < at("monthly"), "{shape:?}");

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT version FROM accounting_state WHERE tenant_id = $1 FOR UPDATE")
        .bind(installed.tenant_id)
        .fetch_all(&mut *holder)
        .await
        .unwrap();
    let event = recalibration(april(), "9000");
    let step = projector_step(Some(&installed));
    let refused = apply_event_to_projections_with(&proxied, &event, step);
    let (refused, shape) = traced(&proxy, refused).await;
    holder.rollback().await.unwrap();
    assert!(refused.is_err(), "the apply landed past a held row: {shape:?}");
    assert!(shape.contains(&"bump"), "{shape:?}");
    assert!(
        !shape.iter().any(|s| ["monthly", "lock"].contains(s)),
        "monthly_state before the accounting row: {shape:?}"
    );
    let (base, _, _) = month(&pool, april()).await;
    assert_eq!((versions(&pool, &installed).await, base), ((1, Some(1)), 10000.into()));
}

/// Healthy progress: repeated applies on a balance daemon each advance the
/// version and the installed generation together, so the order-set scan
/// qualifies after every one; each apply ends inside PLAN-r4 §7.2's 400 ms
/// holder target (its slowest is printed). A generation left stale by a writer
/// that bumped without it refuses the next apply whole: no `monthly_state`
/// change, no version move.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn projector_apply_then_scan_qualifies(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let step = projector_step(Some(&installed));
    let mut slowest = Duration::ZERO;
    for round in 0..40 {
        let event = match round % 4 {
            0 => reset(april(), "10000"),
            1 => recalibration(april(), "9000"),
            2 => fill(april()),
            _ => losing_close(april()),
        };
        let started = Instant::now();
        apply_event_to_projections_with(&pool, &event, step).await.unwrap();
        slowest = slowest.max(started.elapsed());
        assert!(qualifies(&pool, &installed).await, "round {round}");
    }
    println!("ACCOUNTING_8L4_BOUND applies=40 slowest_us={}", slowest.as_micros());
    assert!(slowest < Duration::from_millis(400), "{slowest:?}");
    assert_eq!(versions(&pool, &installed).await, (40, Some(40)));

    stale(&pool, &installed).await;
    let before = month(&pool, april()).await;
    let refused =
        apply_event_to_projections_with(&pool, &recalibration(april(), "8000"), step).await;
    assert!(matches!(refused, Err(ProjectionError::Accounting(_))), "{refused:?}");
    assert_eq!(month(&pool, april()).await, before);
    assert_eq!(versions(&pool, &installed).await, (41, Some(40)));
    assert!(!qualifies(&pool, &installed).await);
}

/// 8m's tx2 with the real handlers (v16 §C10 certificate steps 3-6): the
/// recalibration's projection apply bumps the version once and 8m's anchor
/// and activation-basis transaction once more, so completion's conditional
/// bump finds exactly `v₁ + 2`. The month carries the new base and zero
/// carried risk, its model unchanged.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn recalibration_apply_bumps_once_for_the_cutover(pool: PgPool) {
    let installed = seed(&pool, true).await;
    let step = projector_step(Some(&installed));
    apply_event_to_projections_with(&pool, &reset(april(), "10000"), step)
        .await
        .unwrap();
    let (v1, _) = versions(&pool, &installed).await;
    let (_, _, model) = month(&pool, april()).await;

    let recalibrated = recalibration(april(), "9000.5");
    apply_event_to_projections_with(&pool, &recalibrated, step).await.unwrap();
    assert_eq!(versions(&pool, &installed).await, (v1 + 1, Some(v1 + 1)));
    // 8m's one transaction: its anchor and activation-basis writes are 8m's;
    // its accounting step is this context's.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(BOUNDS).execute(&mut *tx).await.unwrap();
    assert_eq!(installed.advance(&mut tx).await.unwrap(), Some(v1 + 2));
    tx.commit().await.unwrap();
    let completed = sqlx::query(
        "UPDATE accounting_state SET version = version + 1 \
         WHERE tenant_id = $1 AND venue_id = $2 AND version = $3",
    )
    .bind(installed.tenant_id)
    .bind(SPOT)
    .bind(v1 + 2)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(completed.rows_affected(), 1);
    let expected = ("9000.5".parse().unwrap(), Decimal::ZERO, model);
    assert_eq!(month(&pool, april()).await, expected);
}

/// Source guard (PLAN-r4 §6.7): every caller of the projector in robsond's
/// production code (the append path, the projection worker, the stream
/// repair and 8p-2's zero reset, on its own connection) passes the balance
/// context, and the daemon builds it for both the manager and the worker.
#[test]
fn every_projector_caller_on_a_balance_daemon_passes_its_context() {
    let callers = [
        ("position_manager.rs", include_str!("position_manager.rs")),
        ("projection_worker.rs", include_str!("projection_worker.rs")),
        ("event_streams.rs", include_str!("event_streams.rs")),
        ("boundary.rs", include_str!("boundary.rs")),
    ];
    // Each caller's own context reaches its one apply: the manager's, the
    // repair's (from the manager, checked in 8l-4a's wiring test), the
    // worker's (from the manager, checked below) and the zero reset's, which
    // is only ever a balance daemon's (it has no other) and reaches the
    // projector's owned-connection entry, whose step is not optional.
    let flows: [&[&str]; 4] = [
        &[
            "let accounting = self.balance_accounting.as_ref(); \
             match Self::append_and_apply(pool, tenant_id, &stream_key, event, accounting)",
            "let step = crate::balance_accounting::projector_step(accounting); \
             apply_event_to_projections_with(pool, &envelope, step)",
        ],
        &[
            "let step = projector_step(self.balance_accounting.as_ref()); \
             match apply_event_to_projections_with(&self.pool, event, step)",
        ],
        &[
            "repair_under_lock(pool, tenant_id, streams, stream_key, suspended, accounting)",
            "let step = projector_step(accounting); if let Err(error) = \
             robson_projector::apply_event_to_projections_with(pool, envelope, step)",
        ],
        &[
            "let envelope = fetch.fetch_one(held.connection()).instrument(io(1)).await?; \
             failed_seq = Some(envelope.seq); apply_month_boundary_reset_on(held.connection(), \
             &envelope, accounting).await?; held.finished = true;",
        ],
    ];
    let apply_calls = |source: &str| {
        source.matches("apply_event_to_projections_with(").count()
            + source.matches("apply_month_boundary_reset_on(").count()
    };
    let squeezed = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for ((file, source), flow) in callers.iter().zip(flows) {
        let source = squeezed(production(source));
        assert!(!source.contains("apply_event_to_projections("), "{file}");
        assert_eq!(apply_calls(&source), 1, "{file}");
        for step in flow {
            assert!(source.contains(&squeezed(step)), "{file}: {step}");
        }
    }
    let mut pending = vec![std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src"
    ))];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                pending.push(path);
            } else if name.ends_with(".rs") && !name.contains("test") {
                let known = callers.iter().any(|(caller, _)| *caller == name);
                let source = std::fs::read_to_string(&path).unwrap();
                let source = production(&source);
                let applies = source.contains("apply_event_to_projections")
                    || source.contains("apply_month_boundary_reset_on");
                assert!(known || !applies, "{} applies projections", path.display());
            }
        }
    }
    // The worker shares the manager's context, so the journal's cell.
    let daemon = production(include_str!("daemon.rs"));
    assert_eq!(daemon.matches(".with_balance_accounting(accounting)").count(), 2);
    assert!(daemon.contains("self.position_manager.read().await.balance_accounting()"));
}
