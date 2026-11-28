//! ADR-0056 step 8m-2a proofs (8m PLAN-r4 §6.7, §8). A test session holds the
//! cutover certificate's key exclusive from its grant to its `COMMIT`, as
//! completion (8m-5) will. While it does, every transactional participant, on
//! both account kinds, either waits and commits after it (a position
//! account's pool has no server bounds) or refuses at its bound with nothing
//! written (a balance account's bounds) and commits on its retry; an append
//! refuses at once with nothing written (`position_manager`'s tests show its
//! stream is not suspended); a process started during the window is ordered
//! after it; a participant waits holding no row; and generated interleavings
//! with completion never deadlock and never write inside its window.
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::Utc;
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, RngSeed, TestCaseError, TestRunner},
};
use robson_eventlog::{
    append_event,
    certificate::{EXCLUSIVE, PARTICIPATE, TRY_PARTICIPATE},
    Event, EventLogError,
};
use robson_exec::{
    gate::{Incident, IncidentReason},
    order_journal::{
        CommitFailure, IntentKind, IntentRow, JournalBackend, JournalBatch, ReleaseRecord,
        StreamSuspensionRow, DEFAULT_ACCOUNT_SCOPE,
    },
    venue::{AccountKind, VenueId},
};
use robson_testkit::db_proxy::DbProxy;
use sqlx::{postgres::PgPoolOptions, Connection, PgConnection, PgPool};
use uuid::Uuid;

use super::{AccountingRefusal, BalanceAccounting, GenerationCell, BOUNDS};
use crate::{
    order_journal_store::{record_disposition, Disposition, PgJournalBackend},
    process_epoch,
};

const SPOT: &str = VenueId::BINANCE_SPOT;
const USDM: &str = VenueId::BINANCE_USDM;

/// Completion's stand-in: one session holding the key exclusive from its grant
/// to its `COMMIT`. Its reads see what committed before its grant and, under
/// READ COMMITTED, anything that commits while it holds the key. Shared with
/// the writers' own test modules.
pub(crate) struct Completion(PgConnection);

impl Completion {
    /// Granted: every shared holder before it has committed or rolled back.
    pub(crate) async fn hold(pool: &PgPool) -> Result<Self> {
        let mut connection = PgConnection::connect_with(&pool.connect_options()).await?;
        sqlx::query("BEGIN").execute(&mut connection).await?;
        sqlx::query(EXCLUSIVE).execute(&mut connection).await?;
        Ok(Self(connection))
    }

    async fn latest_epoch(&mut self, tenant_id: Uuid) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT max(epoch_seq) FROM process_epochs WHERE tenant_id = $1")
                .bind(tenant_id)
                .fetch_one(&mut self.0)
                .await?,
        )
    }

    /// Completion's bump takes every accounting row of the tenant; a
    /// participant waiting for the key holds none, so this never waits on one.
    async fn take_rows(&mut self, tenant_id: Uuid) -> Result<()> {
        sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut self.0).await?;
        sqlx::query(
            "UPDATE accounting_state SET version = version + 1 WHERE tenant_id = $1 \
             RETURNING version",
        )
        .bind(tenant_id)
        .fetch_all(&mut self.0)
        .await
        .context("completion waited on a participant's row")?;
        Ok(())
    }

    async fn snapshot(&mut self, tenant_id: Uuid) -> Result<Snapshot> {
        snapshot(&mut self.0, tenant_id).await
    }

    /// Completion's own session, for a read inside its window (8m-2b).
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.0
    }

    pub(crate) async fn release(mut self) -> Result<()> {
        sqlx::query("COMMIT").execute(&mut self.0).await?;
        self.0.close().await?;
        Ok(())
    }
}

/// Every certification input a participant here writes, for one tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    events: i64,
    epochs: i64,
    incidents: i64,
    intents: i64,
    suspensions: i64,
    versions: i64,
}

async fn snapshot(connection: &mut PgConnection, tenant_id: Uuid) -> Result<Snapshot> {
    let (events, epochs, incidents, intents, suspensions, versions) =
        sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(
            "SELECT (SELECT count(*) FROM event_log WHERE tenant_id = $1), \
                    (SELECT count(*) FROM process_epochs WHERE tenant_id = $1), \
                    (SELECT coalesce(sum(count), 0)::BIGINT FROM recovery_incidents \
                     WHERE tenant_id = $1), \
                    (SELECT count(*) FROM order_intents WHERE tenant_id = $1), \
                    (SELECT count(*) FROM stream_suspensions WHERE tenant_id = $1), \
                    (SELECT coalesce(sum(version), 0)::BIGINT FROM accounting_state \
                     WHERE tenant_id = $1)",
        )
        .bind(tenant_id)
        .fetch_one(connection)
        .await?;
    Ok(Snapshot {
        events,
        epochs,
        incidents,
        intents,
        suspensions,
        versions,
    })
}

async fn now_of(pool: &PgPool, tenant_id: Uuid) -> Result<Snapshot> {
    snapshot(&mut *pool.acquire().await?, tenant_id).await
}

/// Sessions waiting for the key in shared mode, a participant's statement.
async fn key_waiters(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
         AND wait_event_type = 'Lock' AND wait_event = 'advisory' AND query = $1",
    )
    .bind(PARTICIPATE)
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn until_key_waiters(pool: &PgPool, waiters: i64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while key_waiters(pool).await? < waiters {
        ensure!(Instant::now() < deadline, "fewer than {waiters} participants wait for the key");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

fn lock_timeout(error: &sqlx::Error) -> bool {
    error.as_database_error().and_then(|e| e.code()).as_deref() == Some("55P03")
}

/// A balance journal sets its lock and statement timeouts from one budget,
/// so a wait for the key ends at whichever fires first.
fn bound_refusal(why: &str) -> bool {
    why.contains("lock timeout") || why.contains("statement timeout")
}

/// A Spot accounting row at version 0 and a balance daemon's context on it,
/// with no generation installed (no CAS).
async fn balance(pool: &PgPool) -> Result<BalanceAccounting> {
    let tenant_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'net_from_start_non_expanding_v1')",
    )
    .bind(tenant_id)
    .bind(SPOT)
    .execute(pool)
    .await?;
    BalanceAccounting::for_daemon(
        AccountKind::BalanceAccount,
        tenant_id,
        &VenueId::new(SPOT),
        &GenerationCell::default(),
    )
    .ok_or_else(|| anyhow!("a balance account builds a context"))
}

/// One `advance` in its own transaction; with `bounded`, the ledger's bounds
/// first, as every balance writer has them.
async fn advance_once(
    pool: &PgPool,
    accounting: &BalanceAccounting,
    bounded: bool,
) -> Result<Option<i64>, AccountingRefusal> {
    let mut tx = pool.begin().await?;
    if bounded {
        sqlx::query(BOUNDS).execute(&mut *tx).await?;
    }
    let version = accounting.advance(&mut tx).await?;
    tx.commit().await?;
    Ok(version)
}

fn incident() -> Incident {
    Incident {
        reason: IncidentReason::UncertainOutcome,
        first_at: Utc::now(),
        last_at: Utc::now(),
        count: 1,
        first_generation: 1,
        last_generation: 1,
    }
}

fn suspension() -> StreamSuspensionRow {
    StreamSuspensionRow {
        suspension_id: Uuid::now_v7(),
        stream_key: format!("position:{}", Uuid::now_v7()),
        stream_kind: "position",
        failed_seq: None,
        month: None,
        error: "8m-2a".to_string(),
        epoch_id: None,
        suspended_at: Utc::now(),
    }
}

fn release() -> ReleaseRecord {
    ReleaseRecord {
        decision_id: Uuid::now_v7(),
        generation: 1,
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
        note: "8m-2a".to_string(),
        treatment: "unbooked_accepted",
        decided_at: Utc::now(),
    }
}

/// A USD-M intent with no attempt, as the legacy journal writes it.
pub(crate) fn usdm_batch() -> JournalBatch {
    let intent_id = Uuid::now_v7();
    JournalBatch {
        intent: IntentRow {
            intent_id,
            venue_id: USDM.to_owned(),
            account_scope: DEFAULT_ACCOUNT_SCOPE.to_owned(),
            client_order_id: format!("m2a-{}", intent_id.simple()),
            position_id: Some(Uuid::now_v7()),
            kind: IntentKind::Entry,
            cancels_order_id: None,
            params: serde_json::json!({}),
            created_at: Utc::now(),
        },
        attempts: Vec::new(),
        outcome: None,
        balance: None,
    }
}

fn disposition(intent_id: Uuid) -> Disposition {
    Disposition {
        intent_id,
        terminal_evidence: None,
        treatment: "unbooked_accepted",
        note: "8m-2a".to_string(),
        actor: "operator@example.test".to_string(),
        auth_kind: "verified",
    }
}

fn probe(tenant_id: Uuid) -> (String, Event) {
    let stream_key = format!("position:{}", Uuid::now_v7());
    let event = Event::new(tenant_id, &stream_key, "probe", serde_json::json!({ "m2a": true }));
    (stream_key, event)
}

/// `advance` without the key (mutant) → this test. Every balance bump waits
/// for completion within its bounds and refuses with nothing moved: a bounded
/// `advance`, the balance journal's incident (`advance_participating` after
/// its own key), its release and the ledger's `ensure_accounting_row` of a
/// new tenant. After completion each lands on its retry, and each bump moves
/// the version once.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_balance_bump_waits_for_completion(pool: PgPool) -> Result<()> {
    let accounting = balance(&pool).await?;
    let tenant_id = accounting.tenant_id;
    let journal = PgJournalBackend::new(&pool, tenant_id).with_balance_account(true);
    journal.check_ready().await.map_err(|error| anyhow!(error))?;
    let fresh = Uuid::now_v7();
    let ledger = crate::venue_ledger_store::PgVenueLedger::new(&pool, fresh);
    let model = robson_domain::MonthlyBudgetModel::NetFromStartNonExpandingV1;
    let rows_of = |tenant: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM accounting_state WHERE tenant_id = $1",
            )
            .bind(tenant)
            .fetch_one(&pool)
            .await
        }
    };
    let before = now_of(&pool, tenant_id).await?;

    let completion = Completion::hold(&pool).await?;
    let started = Instant::now();
    let refused = advance_once(&pool, &accounting, true).await;
    ensure!(
        matches!(&refused, Err(AccountingRefusal::Database(error)) if lock_timeout(error)),
        "a bounded advance refuses at its lock bound: {refused:?}"
    );
    let incident_refused = journal.upsert_incident(&incident()).await;
    ensure!(
        matches!(&incident_refused, Err(CommitFailure::Known(why)) if bound_refusal(why)),
        "the balance incident refuses, known: {incident_refused:?}"
    );
    let release_refused = journal.release(&release()).await;
    ensure!(
        matches!(&release_refused, Err(why) if bound_refusal(why)),
        "the balance release refuses: {release_refused:?}"
    );
    let row_refused = ledger.ensure_accounting_row(&VenueId::new(SPOT), model).await;
    ensure!(row_refused.is_err(), "the accounting row's insert refuses: {row_refused:?}");
    ensure!(rows_of(fresh).await? == 0);
    ensure!(started.elapsed() < Duration::from_secs(5), "each waited only its bound");
    ensure!(
        now_of(&pool, tenant_id).await? == before,
        "nothing moved while completion held the key"
    );
    completion.release().await?;

    // Clean progress: every retry lands.
    ensure!(advance_once(&pool, &accounting, true).await? == Some(1));
    journal.upsert_incident(&incident()).await.map_err(|f| anyhow!("{f:?}"))?;
    journal.release(&release()).await.map_err(|error| anyhow!(error))?;
    ledger.ensure_accounting_row(&VenueId::new(SPOT), model).await?;
    ensure!(rows_of(fresh).await? == 1);
    let after = now_of(&pool, tenant_id).await?;
    ensure!(after.incidents == 1, "{after:?}");
    ensure!(after.versions == 3, "three bumps, one each: {after:?}");
    Ok(())
}

/// `advance_from` without its version check (mutant) → this test. On a
/// transaction that holds the key it bumps every row of the tenant and lands
/// only from the expected version; from any other it refuses, and the
/// caller's rollback leaves nothing moved (8m-4's tx2b, 8l-5's conditional
/// bump).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn advance_from_lands_only_from_the_expected_version(pool: PgPool) -> Result<()> {
    let accounting = balance(&pool).await?;
    let from = |expected: i64| {
        let (pool, accounting) = (pool.clone(), accounting.clone());
        async move {
            let mut tx = pool.begin().await?;
            let key = robson_eventlog::certificate::participate(&mut tx).await?;
            let advanced = accounting.advance_from(&mut tx, &key, expected).await;
            match advanced {
                Ok(_) => tx.commit().await?,
                Err(_) => tx.rollback().await?,
            }
            Ok::<_, anyhow::Error>(advanced)
        }
    };
    let refused = from(1).await?;
    ensure!(matches!(refused, Err(AccountingRefusal::VersionMoved)), "{refused:?}");
    ensure!(now_of(&pool, accounting.tenant_id).await?.versions == 0, "nothing moved");
    let landed = from(0).await?;
    ensure!(matches!(landed, Ok(1)), "{landed:?}");
    ensure!(now_of(&pool, accounting.tenant_id).await?.versions == 1);
    Ok(())
}

/// The append blocking instead of trying (mutant) → this test's other half
/// in `position_manager`. Here the event log itself: while completion holds
/// the key `append_event` answers `CertificateInProgress` at once with no
/// event, no idempotency claim and no stream state; after it, the same event
/// lands.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_append_refuses_at_once_with_nothing_written(pool: PgPool) -> Result<()> {
    let tenant_id = Uuid::now_v7();
    let (stream_key, event) = probe(tenant_id);
    let claims = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM event_idempotency WHERE tenant_id = $1) \
                  + (SELECT count(*) FROM stream_state WHERE stream_key = $2)",
        )
        .bind(tenant_id)
        .bind(&stream_key)
        .fetch_one(&pool)
        .await
    };
    let completion = Completion::hold(&pool).await?;
    let started = Instant::now();
    let refused = tokio::time::timeout(
        Duration::from_secs(2),
        append_event(&pool, &stream_key, None, event.clone()),
    )
    .await
    .map_err(|_| anyhow!("the append waited for completion"))?;
    ensure!(
        matches!(refused, Err(EventLogError::CertificateInProgress)),
        "certain refusal: {refused:?}"
    );
    ensure!(started.elapsed() < Duration::from_secs(1));
    ensure!(now_of(&pool, tenant_id).await?.events == 0);
    ensure!(claims().await? == 0, "no idempotency claim, no stream state");
    completion.release().await?;
    append_event(&pool, &stream_key, None, event).await?;
    ensure!(now_of(&pool, tenant_id).await?.events == 1);
    Ok(())
}

/// The epoch without the key (mutant) → this test. On both epoch paths: a
/// process that started before the grant is the latest epoch completion
/// reads (completion abandons on it); one that starts after completion's read
/// waits for the key, so nothing lands inside the window, and its epoch lands
/// after the `COMMIT`, the successor of the one completion read.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_process_started_during_completion_is_ordered_after_it(pool: PgPool) -> Result<()> {
    for balance_start in [false, true] {
        let tenant_id = match balance_start {
            true => balance(&pool).await?.tenant_id,
            false => Uuid::now_v7(),
        };
        let start = move |pool: PgPool| async move {
            match balance_start {
                true => process_epoch::start_balance_epoch(&pool, tenant_id, "8m-2a").await,
                false => process_epoch::start_epoch(&pool, tenant_id, "8m-2a").await,
            }
        };
        let before = start(pool.clone()).await?;
        let mut completion = Completion::hold(&pool).await?;
        ensure!(completion.latest_epoch(tenant_id).await? == Some(before.epoch.epoch_seq));

        let during = tokio::spawn(start(pool.clone()));
        until_key_waiters(&pool, 1).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        ensure!(
            completion.latest_epoch(tenant_id).await? == Some(before.epoch.epoch_seq),
            "an epoch landed inside completion's window ({balance_start})"
        );
        completion.release().await?;
        let during = during.await??;
        ensure!(during.epoch.epoch_seq > before.epoch.epoch_seq);
        ensure!(during.predecessor_epoch_id == Some(before.epoch.epoch_id));
    }
    Ok(())
}

/// A USD-M journal commit without the key (mutant) → this test. The legacy
/// journal's intent commit has no server bounds: it waits for completion,
/// writes nothing inside the window, and commits after it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn usdm_intent_commit_waits_for_completion(pool: PgPool) -> Result<()> {
    let tenant_id = Uuid::now_v7();
    let journal = PgJournalBackend::new(&pool, tenant_id);
    journal.check_ready().await.map_err(|error| anyhow!(error))?;
    let completion = Completion::hold(&pool).await?;
    let batch = usdm_batch();
    let commit = tokio::spawn(async move { journal.commit(&batch).await });
    until_key_waiters(&pool, 1).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ensure!(
        now_of(&pool, tenant_id).await?.intents == 0,
        "the intent landed inside the window"
    );
    completion.release().await?;
    commit.await?.map_err(|f| anyhow!("{f:?}"))?;
    ensure!(now_of(&pool, tenant_id).await?.intents == 1);
    Ok(())
}

/// A USD-M incident write without the key (mutant) → this test. The legacy
/// journal's incident, suspension, release and an operator's disposition
/// each wait for completion, write nothing inside the window, and commit
/// after it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn usdm_incident_write_waits_for_completion(pool: PgPool) -> Result<()> {
    let tenant_id = Uuid::now_v7();
    let journal = std::sync::Arc::new(PgJournalBackend::new(&pool, tenant_id));
    journal.check_ready().await.map_err(|error| anyhow!(error))?;
    let outstanding = usdm_batch();
    journal.commit(&outstanding).await.map_err(|f| anyhow!("{f:?}"))?;
    let decisions = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM journal_decisions")
            .fetch_one(&pool)
            .await
    };
    for write in ["incident", "suspension", "release", "disposition"] {
        let (before, decided) = (now_of(&pool, tenant_id).await?, decisions().await?);
        let completion = Completion::hold(&pool).await?;
        let (journal, pool_of, intent_id) =
            (journal.clone(), pool.clone(), outstanding.intent.intent_id);
        let writer = tokio::spawn(async move {
            match write {
                "incident" => {
                    journal.upsert_incident(&incident()).await.map_err(|f| format!("{f:?}"))
                },
                "suspension" => {
                    journal.record_suspension(&suspension()).await.map_err(|f| format!("{f:?}"))
                },
                "release" => journal.release(&release()).await.map(drop),
                _ => record_disposition(&pool_of, &disposition(intent_id))
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string()),
            }
        });
        until_key_waiters(&pool, 1).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        ensure!(now_of(&pool, tenant_id).await? == before, "{write} landed inside the window");
        ensure!(decisions().await? == decided, "{write} decided inside the window");
        completion.release().await?;
        writer.await?.map_err(|error| anyhow!("{write}: {error}"))?;
    }
    let after = now_of(&pool, tenant_id).await?;
    ensure!(after.suspensions == 1 && decisions().await? == 2, "{after:?}");
    Ok(())
}

/// The key after a row lock (mutant: the balance epoch locks the accounting
/// rows first) → this test. A participant waiting for the key holds no row:
/// completion, holding the key, takes every accounting row of the tenant
/// without waiting while a balance epoch start and an unbounded `advance` wait
/// for it; both then land.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn participants_take_the_key_before_any_row_lock(pool: PgPool) -> Result<()> {
    let accounting = balance(&pool).await?;
    let tenant_id = accounting.tenant_id;
    let mut completion = Completion::hold(&pool).await?;
    let epoch = {
        let pool = pool.clone();
        tokio::spawn(
            async move { process_epoch::start_balance_epoch(&pool, tenant_id, "8m-2a").await },
        )
    };
    let bump = {
        let (pool, accounting) = (pool.clone(), accounting.clone());
        tokio::spawn(async move { advance_once(&pool, &accounting, false).await })
    };
    until_key_waiters(&pool, 2).await?;
    completion.take_rows(tenant_id).await?;
    completion.release().await?;
    epoch.await??;
    ensure!(bump.await?? == Some(2), "completion's bump, then the participant's");
    Ok(())
}

/// The clean-stop marker, a participating transaction since 8m-2a: it waits
/// for completion, writes nothing inside the window, and lands after it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_clean_stop_marker_waits_for_completion(pool: PgPool) -> Result<()> {
    let tenant_id = Uuid::now_v7();
    // A clean predecessor, so this start opens no incident and may mark.
    sqlx::query(
        "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version, clean_stop_at, \
         marker_version) VALUES ($1, $2, '8m-2a', now(), $3)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(process_epoch::MARKER_VERSION)
    .execute(&pool)
    .await?;
    let start = process_epoch::start_epoch(&pool, tenant_id, "8m-2a").await?;
    ensure!(start.open_incidents.is_empty(), "{start:?}");
    let marked = || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT clean_stop_at IS NOT NULL FROM process_epochs WHERE epoch_id = $1",
        )
        .bind(start.epoch.epoch_id)
        .fetch_one(&pool)
        .await
    };
    let completion = Completion::hold(&pool).await?;
    let mark = {
        let (pool, epoch) = (pool.clone(), start.epoch);
        tokio::spawn(async move { process_epoch::mark_clean_stop(&pool, epoch).await })
    };
    until_key_waiters(&pool, 1).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ensure!(!marked().await?, "the marker landed inside completion's window");
    completion.release().await?;
    mark.await?.map_err(|error| anyhow!(error))?;
    ensure!(marked().await?);
    Ok(())
}

/// The statements a USD-M append, an epoch start and the clean-stop marker
/// send through a proxy: each gains exactly the key (v16:2227), the append's
/// as its try-lock right after `BEGIN`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn appends_epochs_and_the_stop_marker_add_exactly_the_key(pool: PgPool) -> Result<()> {
    let (proxy, proxied) = through_proxy(&pool).await?;
    let tenant_id = Uuid::now_v7();
    let shape = |trace: Vec<String>| -> Vec<&'static str> {
        trace
            .iter()
            .map(|sql| match sql.trim() {
                "BEGIN" => "begin",
                "COMMIT" => "commit",
                sql if sql == TRY_PARTICIPATE => "try",
                sql if sql == PARTICIPATE => "key",
                _ => "statement",
            })
            .collect()
    };
    proxy.enable_query_trace(64);
    let (stream_key, event) = probe(tenant_id);
    append_event(&proxied, &stream_key, None, event).await?;
    ensure!(
        shape(proxy.query_trace())
            == [
                "begin",
                "try",
                "statement",
                "statement",
                "statement",
                "statement",
                "commit"
            ],
        "append: {:?}",
        proxy.query_trace()
    );
    proxy.enable_query_trace(64);
    let first = process_epoch::start_epoch(&proxied, tenant_id, "8m-2a").await?;
    // No predecessor: the unclean start stamps and opens its incident.
    ensure!(
        shape(proxy.query_trace())
            == [
                "begin",
                "key",
                "statement",
                "statement",
                "statement",
                "statement",
                "statement",
                "commit"
            ],
        "epoch: {:?}",
        proxy.query_trace()
    );
    proxy.enable_query_trace(64);
    let open = process_epoch::mark_clean_stop(&proxied, first.epoch).await;
    ensure!(open.is_err(), "an open incident keeps the marker off");
    ensure!(
        shape(proxy.query_trace()) == ["begin", "key", "statement", "commit"],
        "marker: {:?}",
        proxy.query_trace()
    );
    Ok(())
}

async fn through_proxy(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .ok_or_else(|| anyhow!("no upstream address"))?;
    let proxy = DbProxy::start(upstream).await?;
    let url = robson_testkit::daemon_process::database_url_through(
        &std::env::var("DATABASE_URL")?,
        proxy.addr(),
        options.get_database().ok_or_else(|| anyhow!("no database"))?,
    )?;
    // The operation's statements only: no acquisition ping.
    let proxied = PgPoolOptions::new()
        .max_connections(2)
        .test_before_acquire(false)
        .connect(&url)
        .await?;
    Ok((proxy, proxied))
}

/// One participant of a generated interleaving.
#[derive(Debug, Clone, Copy)]
enum Writer {
    Append,
    Epoch,
    BalanceEpoch,
    Advance { bounded: bool },
    UsdmIncident,
    BalanceIncident,
    UsdmIntent,
}

/// What a writer did: whether it landed, and what it moved if it did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Landed {
    events: i64,
    epochs: i64,
    intents: i64,
    bumps: i64,
}

fn writer() -> impl Strategy<Value = Writer> {
    prop_oneof![
        Just(Writer::Append),
        Just(Writer::Epoch),
        Just(Writer::BalanceEpoch),
        any::<bool>().prop_map(|bounded| Writer::Advance { bounded }),
        Just(Writer::UsdmIncident),
        Just(Writer::BalanceIncident),
        Just(Writer::UsdmIntent),
    ]
}

#[derive(Debug, Clone)]
struct History {
    writers: Vec<(Writer, u64)>,
    completion_at: u64,
    completion_hold: u64,
}

fn history() -> impl Strategy<Value = History> {
    (proptest::collection::vec((writer(), 0u64..40), 2..9), 0u64..40, 0u64..60).prop_map(
        |(writers, completion_at, completion_hold)| History {
            writers,
            completion_at,
            completion_hold,
        },
    )
}

/// Runs one writer; a refusal allowed by its kind lands nothing. Any other
/// error, a deadlock (`40P01`) among them, fails the history.
async fn run_writer(pool: PgPool, accounting: BalanceAccounting, writer: Writer) -> Result<Landed> {
    let tenant_id = accounting.tenant_id;
    let none = Landed::default();
    match writer {
        Writer::Append => {
            let (stream_key, event) = probe(tenant_id);
            match append_event(&pool, &stream_key, None, event).await {
                Ok(_) => Ok(Landed { events: 1, ..none }),
                Err(EventLogError::CertificateInProgress) => Ok(none),
                Err(error) => Err(anyhow!("append: {error}")),
            }
        },
        Writer::Epoch => {
            process_epoch::start_epoch(&pool, tenant_id, "8m-2a").await?;
            Ok(Landed { epochs: 1, ..none })
        },
        Writer::BalanceEpoch => {
            process_epoch::start_balance_epoch(&pool, tenant_id, "8m-2a").await?;
            Ok(Landed { epochs: 1, ..none })
        },
        Writer::Advance { bounded } => match advance_once(&pool, &accounting, bounded).await {
            Ok(_) => Ok(Landed { bumps: 1, ..none }),
            Err(AccountingRefusal::Database(error)) if bounded && lock_timeout(&error) => Ok(none),
            Err(refusal) => Err(anyhow!("advance: {refusal}")),
        },
        Writer::UsdmIncident => {
            let journal = PgJournalBackend::new(&pool, tenant_id);
            journal.upsert_incident(&incident()).await.map_err(|f| anyhow!("{f:?}"))?;
            Ok(none)
        },
        Writer::BalanceIncident => {
            let journal = PgJournalBackend::new(&pool, tenant_id).with_balance_account(true);
            match journal.upsert_incident(&incident()).await {
                Ok(()) => Ok(Landed { bumps: 1, ..none }),
                Err(CommitFailure::Known(why)) if bound_refusal(&why) => Ok(none),
                Err(failure) => Err(anyhow!("balance incident: {failure:?}")),
            }
        },
        Writer::UsdmIntent => {
            let journal = PgJournalBackend::new(&pool, tenant_id);
            journal.commit(&usdm_batch()).await.map_err(|f| anyhow!("{f:?}"))?;
            Ok(Landed { intents: 1, ..none })
        },
    }
}

/// One history on its own tenant: every writer and completion concurrently.
/// Completion takes the rows without waiting, sees the same inputs at its
/// grant and before its `COMMIT` (nothing interleaves), and every writer ends
/// landed or refused as its kind allows, never deadlocked; what landed is
/// exactly what the tables hold.
async fn run_history(pool: &PgPool, history: History) -> Result<()> {
    let accounting = balance(pool).await?;
    let tenant_id = accounting.tenant_id;
    let writers: Vec<_> = history
        .writers
        .iter()
        .map(|&(writer, at)| {
            let (pool, accounting) = (pool.clone(), accounting.clone());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(at)).await;
                run_writer(pool, accounting, writer)
                    .await
                    .with_context(|| format!("{writer:?}"))
            })
        })
        .collect();
    let completion = {
        let pool = pool.clone();
        let (at, hold) = (history.completion_at, history.completion_hold);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(at)).await;
            let mut completion = Completion::hold(&pool).await?;
            let granted = completion.snapshot(tenant_id).await?;
            completion.take_rows(tenant_id).await?;
            tokio::time::sleep(Duration::from_millis(hold)).await;
            let before_commit = completion.snapshot(tenant_id).await?;
            completion.release().await?;
            let moved = Snapshot {
                versions: granted.versions + 1,
                ..granted
            };
            ensure!(before_commit == moved, "a write landed inside completion's window");
            Ok::<_, anyhow::Error>(())
        })
    };
    let mut landed = Landed::default();
    for writer in writers {
        let done = tokio::time::timeout(Duration::from_secs(30), writer)
            .await
            .map_err(|_| anyhow!("a writer never finished"))???;
        landed.events += done.events;
        landed.epochs += done.epochs;
        landed.intents += done.intents;
        landed.bumps += done.bumps;
    }
    tokio::time::timeout(Duration::from_secs(30), completion)
        .await
        .map_err(|_| anyhow!("completion never finished"))???;
    let after = now_of(pool, tenant_id).await?;
    ensure!(
        (after.events, after.epochs, after.intents, after.versions)
            == (landed.events, landed.epochs, landed.intents, landed.bumps + 1),
        "{after:?} {landed:?}"
    );
    Ok(())
}

/// No deadlock across generated interleavings with completion, on a
/// multithreaded runtime (guardrails §1.2): appends, both epoch paths,
/// bounded and unbounded bumps, both journals' incidents and USD-M intents
/// race a completion stand-in that holds the key and takes the rows.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_interleavings_with_completion_never_deadlock(pool: PgPool) -> Result<()> {
    let options = (*pool.connect_options()).clone();
    std::thread::spawn(move || -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        let pool =
            runtime.block_on(PgPoolOptions::new().max_connections(24).connect_with(options))?;
        let mut runner = TestRunner::new(ProptestConfig {
            cases: 16,
            max_shrink_iters: 8,
            rng_seed: RngSeed::Fixed(0x8_2a_c3_7e),
            failure_persistence: None,
            ..ProptestConfig::default()
        });
        let result = runner
            .run(&history(), |history| {
                runtime
                    .block_on(run_history(&pool, history))
                    .map_err(|error| TestCaseError::fail(format!("{error:#}")))
            })
            .map_err(|error| anyhow!("{error}"));
        runtime.block_on(pool.close());
        result
    })
    .join()
    .map_err(|_| anyhow!("the history thread panicked"))?
}
